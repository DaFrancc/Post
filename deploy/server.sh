#!/usr/bin/env bash
# Set up and look after a Post server on Debian 13, as root, from a clone of
# the repository (deploy/README.md).
#
#   deploy/server.sh install DOMAIN [--no-calls] [--image SOURCE] [--yes]
#       Set up the server, or set it up again: safe to re-run. Installs
#       Podman, makes way on ports 80 and 443, opens the firewall, makes the
#       secrets, starts everything, checks HTTPS and prints the first invite.
#   deploy/server.sh update [--image SOURCE] [--no-pull]
#       Newer code: pulls this clone, then a new server image, and restarts.
#   deploy/server.sh invite [--bot] [--days N]
#       A code for someone to create an account with.
#   deploy/server.sh host add USER | host remove USER | host list
#       Hosts post announcements to everyone, and plan maintenance, from
#       the app.
#   deploy/server.sh announce TEXT
#       Post an announcement to everyone on the server; - reads it from
#       standard input. It is shown as from the server and never deleted.
#   deploy/server.sh maintenance plan [--at TIME | --in LENGTH] --minutes N
#                                     [--reason TEXT] [--quiet] [--yes]
#       Tell everyone, in their own time zone, that the server will be down,
#       and remind them a day, an hour, 30 and 5 minutes before. TIME is
#       anything `date -d` reads, in this machine's time zone ("2026-11-14
#       02:00", "tomorrow 02:00"); LENGTH like "2 hours". Neither starts it
#       now. --quiet skips telling everyone now; the reminders still go.
#   deploy/server.sh maintenance end
#       Cancel the planned maintenance, or say the one underway is over.
#   deploy/server.sh maintenance [status]
#       What is planned.
#   deploy/server.sh status
#       What is running, the certificate, and the release apps are offered.
#   deploy/server.sh push SSH_HOST
#       On your own computer: build the server image here and load it on a
#       small server, which then runs `update --image local`.
#   deploy/server.sh backup [FILE] [--to AGE_PUBLIC_KEY] [--stop]
#       Everything the server needs, in one encrypted file: its database,
#       secrets and settings. Encrypted with a passphrase you type, or to an
#       age public key (--to), so it can run unattended and this server
#       cannot open its own backups. postd stops for a few seconds while its
#       database is copied. --stop leaves the server off afterwards, for
#       moving it: messages sent to it after the backup would be lost.
#   deploy/server.sh restore FILE [--identity AGE_KEY_FILE] [--image SOURCE]
#       Bring a backup up on a new server, then set it up as install does.
#       Only where there is no Post server yet: it never overwrites one.
#
# SOURCE is where the server image comes from:
#   ci       downloaded: the image CI built for this checkout's commit (the
#            default). CI builds one for every push, in a few minutes.
#   build    built here from this checkout (slow on a small server)
#   local    the one already loaded (from `push`)
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
units=/etc/containers/systemd
# Quadlet's %h for system units: root's home.
conf=$(getent passwd root | cut -d: -f6)/.config/post
image=localhost/postd:latest
backups=$(getent passwd root | cut -d: -f6)/post-backups
registry=ghcr.io/dafrancc/post-server
caddy_image=docker.io/library/caddy:2
livekit_image=docker.io/livekit/livekit-server:v1.13.11

yes=false
# Whether the domain points here, as far as this machine can tell.
dns_ok=true

# Undone however the script ends: Ctrl-C, a dropped SSH connection, kill.
# A folder holding the database key, a backup half written, and postd
# stopped for a backup.
cleanup_dir=''
partial_out=''
restart_postd=false
# shellcheck disable=SC2329 # run by the trap below
on_exit() {
  if $restart_postd; then
    systemctl start post-server || printf 'postd did not start again: systemctl start post-server\n' >&2
  fi
  if [[ -n $partial_out ]]; then rm -f -- "$partial_out"; fi
  if [[ -n $cleanup_dir ]]; then rm -rf -- "$cleanup_dir"; fi
}
trap on_exit EXIT
# Signals end the script through exit, so the cleanup above runs.
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# A private folder on disk for a backup's or restore's files. Not /tmp,
# which Debian 13 keeps in memory: a database copy there could fill it.
work_dir() {
  cleanup_dir=$(mktemp -d -p /var/tmp post-backup.XXXXXX)
}

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

# Ask a yes/no question; --yes answers yes.
ask() {
  $yes && return 0
  [[ -t 0 ]] || die "$1 (run with --yes to answer yes)"
  local answer
  read -r -p "$1 [y/N] " answer
  [[ $answer == [yY]* ]]
}

usage() {
  sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'
  exit "${1:-0}"
}

need_root() {
  [[ $EUID -eq 0 ]] || die "run this as root (sudo $0 ...)"
}

check_system() {
  # shellcheck source=/dev/null
  . /etc/os-release
  if [[ ${ID:-} != debian || ${VERSION_ID:-} != 13 ]]; then
    warn "this script is written for Debian 13; this is ${PRETTY_NAME:-unknown}"
    ask "Carry on anyway?" || exit 1
  fi
}

# --------------------------------------------------------------- settings

# The domain of an existing install: from post.env, or from the units of an
# install made before post.env existed.
installed_domain() {
  local d=''
  [[ -f $conf/post.env ]] && d=$(sed -n 's/^POST_DOMAIN=//p' "$conf/post.env" | tail -1)
  [[ -z $d && -f $units/post-server.container ]] && d=$(sed -n 's/^Environment=POST_DOMAIN=//p' "$units/post-server.container" | tail -1)
  printf '%s' "$d"
}

calls_on() {
  grep -q '^POST_LIVEKIT_URL=' "$conf/post.env"
}

check_domain() {
  local domain=$1
  [[ $domain =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$ ]] ||
    die "$domain is not a domain name (lowercase, like chat.example.org)"
  local before
  before=$(installed_domain)
  if [[ -n $before && $before != "$domain" ]]; then
    die "this server is already set up for $before. The domain is part of every account on it and cannot change."
  fi
}

# Where the domain points, against this machine's addresses.
check_dns() {
  local domain=$1
  say "Checking DNS for $domain"
  local v4 v6 mine
  v4=$(getent ahostsv4 "$domain" | awk '{print $1}' | sort -u | tr '\n' ' ' || true)
  # Real IPv6 addresses only, not IPv4 ones mapped into IPv6.
  v6=$(getent ahostsv6 "$domain" | awk '{print $1}' | grep -v '^::ffff:' | sort -u | tr '\n' ' ' || true)
  mine=$(ip -4 -o addr show scope global | awk '{sub("/.*", "", $4); print $4}' | tr '\n' ' ')
  if [[ -z $v4 ]]; then
    dns_ok=false
    warn "$domain has no IPv4 address (A record). Point it at this server first."
    ask "Carry on anyway? Getting the HTTPS certificate will fail until it does." || exit 1
    return
  fi
  local hit=false a
  for a in $v4; do [[ " $mine " == *" $a "* ]] && hit=true; done
  if $hit; then
    note "$domain points here ($v4)"
  else
    warn "$domain points at $v4, but this machine's addresses are $mine"
    note "That is fine if this machine is behind a router that forwards ports to it."
    ask "Carry on?" || exit 1
  fi
  if [[ -n $v6 ]]; then
    warn "$domain also has an IPv6 address ($v6). The server answers on IPv4 only,"
    note "and Let's Encrypt tries IPv6 first, so the certificate would fail. Remove the AAAA record."
    ask "Carry on anyway?" || exit 1
  fi
}

# --------------------------------------------------------------- the host

# Everything this script and the server use, named rather than left to
# Podman's recommendations: minimal server images may not install those.
#   podman, netavark, aardvark-dns   containers, their network, and DNS in it
#   catatonit                        the init in each pod's infra container
#   ca-certificates, curl            HTTPS: pulling images, checking the site
#   iproute2 (ip, ss), procps        addresses and ports; sysctl
#   git, gzip                        updating this clone; loading pushed images
#   age                              encrypting backups
packages=(podman netavark aardvark-dns catatonit ca-certificates curl iproute2 procps git gzip age)

install_packages() {
  local missing=() p
  for p in "${packages[@]}"; do
    dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'ok installed' || missing+=("$p")
  done
  if ((${#missing[@]})); then
    say "Installing ${missing[*]}"
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${missing[@]}" >/dev/null
  fi
  local v
  v=$(podman version --format '{{.Client.Version}}')
  [[ $(printf '%s\n4.4\n' "$v" | sort -V | head -1) == 4.4 ]] || die "Podman $v is too old: Post needs 4.4 or later"
}

# The programs listening on a port, by name; Podman's own are left out.
listeners() {
  local proto=$1 port=$2 flag=-Hltnp
  [[ $proto == udp ]] && flag=-Hlunp
  ss "$flag" "sport = :$port" | grep -o '(("[^"]*"' | cut -d'"' -f2 | grep -v -x -E 'conmon|rootlessport|podman|netavark|aardvark-dns' | sort -u || true
}

free_ports() {
  local calls=$1
  say "Checking ports"
  local svc
  for svc in nginx apache2 caddy lighttpd haproxy traefik; do
    if systemctl is-active --quiet "$svc" 2>/dev/null; then
      warn "$svc is running, and Post needs ports 80 and 443 for itself"
      if ask "Stop $svc and keep it off? (Its configuration stays.)"; then
        systemctl disable --now "$svc"
        # certbot renews through nginx or Apache, which are now off.
        if systemctl is-enabled --quiet certbot.timer 2>/dev/null; then systemctl disable --now certbot.timer; fi
      else
        die "ports 80 and 443 are needed: stop $svc, or use another machine"
      fi
    fi
  done
  local busy=() p who
  local wanted=(tcp:80 tcp:443 udp:443)
  if [[ $calls == yes ]]; then wanted+=(tcp:7881 udp:7882); fi
  for p in "${wanted[@]}"; do
    who=$(listeners "${p%%:*}" "${p#*:}")
    if [[ -n $who ]]; then busy+=("${p#*:}/${p%%:*} ($who)"); fi
  done
  ((${#busy[@]} == 0)) || die "these ports are taken: ${busy[*]}. Stop what uses them, then run this again."
}

# Containers' traffic is forwarded through the host, so a firewall that
# drops forwarded traffic cuts them off: no certificate, no releases, and
# nobody reaches the server.
open_firewall() {
  local calls=$1
  local tcp=80,443 udp=443
  if [[ $calls == yes ]]; then
    tcp+=,7881
    udp+=,7882
  fi
  if command -v ufw >/dev/null && ufw status | grep -q '^Status: active'; then
    say "Opening ufw for Post"
    ufw allow proto tcp to any port "$tcp" comment 'Post' >/dev/null
    ufw allow proto udp to any port "$udp" comment 'Post' >/dev/null
    ufw route allow in on podman0 comment 'Post containers: outgoing' >/dev/null
    ufw route allow out on podman0 proto tcp to any port "$tcp" comment 'Post: published TCP ports' >/dev/null
    ufw route allow out on podman0 proto udp to any port "$udp" comment 'Post: published UDP ports' >/dev/null
  elif systemctl is-active --quiet firewalld 2>/dev/null; then
    say "Opening firewalld for Post"
    local p
    for p in ${tcp//,/ }; do firewall-cmd -q --permanent --add-port="$p/tcp"; done
    for p in ${udp//,/ }; do firewall-cmd -q --permanent --add-port="$p/udp"; done
    firewall-cmd -q --reload
  elif command -v nft >/dev/null && nft list ruleset 2>/dev/null | grep -q 'hook forward.*policy drop'; then
    warn "a firewall drops forwarded traffic; let traffic to and from podman0 through, and open TCP $tcp and UDP $udp"
  fi
  note "If your provider has a firewall in its control panel, open TCP $tcp and UDP $udp there too."
}

tune_kernel() {
  local calls=$1
  [[ $calls == yes ]] || return 0
  # LiveKit asks for a larger UDP receive buffer than Debian's default.
  local file=/etc/sysctl.d/50-post.conf
  printf '# For Post calls: LiveKit wants a larger UDP receive buffer.\nnet.core.rmem_max=5000000\n' >"$file"
  # Only this file: others' settings are not this script's to apply.
  sysctl -q -p "$file" >/dev/null 2>&1 || warn "could not raise the UDP receive buffer for calls (fine for a few people)"
}

# --------------------------------------------------------------- Post

# Made once, and never replaced: a new database key could not open the data.
make_secrets() {
  if ! podman secret exists post-db-key; then
    say "Making the database key"
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | podman secret create post-db-key - >/dev/null
  fi
  if ! podman secret exists post-livekit-keys; then
    printf 'post: %s\n' "$(head -c 36 /dev/urandom | base64 | tr -d '/+=')" | podman secret create post-livekit-keys - >/dev/null
  fi
}

write_settings() {
  local domain=$1 calls=$2
  mkdir -p "$conf"
  if [[ ! -f $conf/post.env ]]; then
    sed "s/chat\.example\.org/$domain/g" "$repo/deploy/post.env" >"$conf/post.env"
    [[ $calls == yes ]] || sed -i '/^POST_LIVEKIT_URL=/d' "$conf/post.env"
    chmod 600 "$conf/post.env"
    note "Settings: $conf/post.env"
  fi
  cp "$repo/deploy/Caddyfile" "$repo/deploy/livekit.yaml" "$conf/"
}

install_units() {
  local calls=$1
  say "Installing the services"
  mkdir -p "$units"
  local f
  for f in "$repo"/deploy/quadlet/*; do
    case $(basename "$f") in
      post-livekit.container) if [[ $calls == yes ]]; then cp "$f" "$units/"; fi ;;
      # Without calls, the pod publishes no call ports.
      post.pod) if [[ $calls == yes ]]; then cp "$f" "$units/"; else grep -v -E '7881|7882|Calls' "$f" >"$units/post.pod"; fi ;;
      *) cp "$f" "$units/" ;;
    esac
  done
  if [[ $calls != yes && -f $units/post-livekit.container ]]; then
    systemctl stop post-livekit 2>/dev/null || true
    rm -f "$units/post-livekit.container"
  fi
  systemctl daemon-reload
}

get_image() {
  local source=$1
  case $source in
    local)
      podman image exists "$image" || die "no server image is loaded: run \`deploy/server.sh push THIS_HOST\` on your computer first"
      note "Using the server image already loaded"
      ;;
    ci) download_image ;;
    build) build_image ;;
    *) die "--image is ci, build or local, not $source" ;;
  esac
}

# The image CI built from this checkout's commit, so it is exactly this code.
download_image() {
  local sha
  sha=$(git -C "$repo" rev-parse HEAD 2>/dev/null) ||
    die "git cannot read $repo (if another user owns it: git config --global --add safe.directory $repo)"
  if [[ -n $(git -C "$repo" status --porcelain --untracked-files=no -- Cargo.toml Cargo.lock crates deploy/Containerfile) ]]; then
    warn "this clone has changes to the server's code; the downloaded image is the commit without them (--image build uses them)"
  fi
  say "Downloading the server image for commit ${sha:0:12}"
  if podman pull -q "$registry:sha-$sha" >/dev/null 2>&1; then
    podman tag "$registry:sha-$sha" "$image"
    return
  fi
  warn "GitHub has no server image for commit ${sha:0:12}"
  note "CI builds one a few minutes after each push: if this commit is new, try again shortly."
  note "If it never appears, the post-server package may be private: make it public on GitHub."
  note "Or build it on your own computer and send it: deploy/server.sh push THIS_HOST, then --image local."
  ask "Build it here instead? (Slow on a small server.)" || exit 1
  build_image
}

build_image() {
  local mem
  mem=$(awk '/MemTotal|SwapTotal/ {kb += $2} END {print int(kb / 1024)}' /proc/meminfo)
  say "Building the server image from this checkout"
  if ((mem < 3000)); then
    note "With ${mem} MB of memory and swap this is slow and may run out. Building on your own"
    note "computer is faster: there, run \`deploy/server.sh push THIS_HOST\`."
  fi
  # The host's network: a firewall that blocks containers' forwarded
  # traffic would otherwise stop the build fetching crates.
  podman build --network host -q -f "$repo/deploy/Containerfile" -t "$image" "$repo" >/dev/null
}

check_container_network() {
  say "Checking that containers can reach the internet"
  podman pull -q "$caddy_image" >/dev/null
  if ! podman run --rm "$caddy_image" wget -q --spider -T 15 https://acme-v02.api.letsencrypt.org/directory 2>/dev/null; then
    die "containers cannot reach the internet, so the certificate and app releases cannot be fetched. A firewall dropping forwarded traffic is the usual cause."
  fi
}

start() {
  local calls=$1
  if [[ $calls == yes ]]; then podman pull -q "$livekit_image" >/dev/null; fi
  say "Starting Post"
  systemctl restart post-pod
  # Up means every service active and postd listening; otherwise, why not.
  local services=(post-server post-caddy)
  if [[ $calls == yes ]]; then services+=(post-livekit); fi
  for _ in $(seq 1 30); do
    if systemctl is-active --quiet "${services[@]}" && podman logs post-server 2>&1 | grep -q 'postd listening'; then
      return 0
    fi
    sleep 1
  done
  local s
  for s in "${services[@]}"; do
    systemctl is-active --quiet "$s" && continue
    warn "$s did not start. Its last words:"
    journalctl -u "$s" -n 15 --no-pager >&2 || true
  done
  die "Post did not start (see above; also podman logs post-server)"
}

# Wait for HTTPS with a valid certificate.
check_https() {
  local domain=$1
  if ! $dns_ok; then
    note "Not waiting for the HTTPS certificate: $domain does not point here yet."
    note "Caddy keeps trying, and gets it once it does."
    return 0
  fi
  say "Waiting for the HTTPS certificate"
  for _ in $(seq 1 60); do
    if curl -s -o /dev/null --max-time 5 "https://$domain/"; then
      note "https://$domain is up"
      return 0
    fi
    sleep 2
  done
  warn "https://$domain did not answer with a valid certificate within two minutes"
  note "See what Caddy says: podman logs post-caddy"
  return 1
}

# --------------------------------------------------------------- commands

cmd_install() {
  local domain='' calls=yes source=ci
  while (($#)); do
    case $1 in
      --no-calls) calls=no ;;
      --image) source=${2:?--image needs a value}; shift ;;
      --yes) yes=true ;;
      -h | --help) usage ;;
      -*) die "unknown option $1" ;;
      *) [[ -z $domain ]] || die "one domain only"; domain=$1 ;;
    esac
    shift
  done
  need_root
  [[ -n $domain ]] || domain=$(installed_domain)
  [[ -n $domain ]] || die "give the server's domain: deploy/server.sh install chat.example.org"
  check_system
  check_domain "$domain"
  # An existing install keeps its choice about calls.
  if [[ -f $conf/post.env ]]; then
    if calls_on; then calls=yes; else calls=no; fi
  fi
  install_packages
  check_dns "$domain"
  free_ports "$calls"
  open_firewall "$calls"
  tune_kernel "$calls"
  make_secrets
  write_settings "$domain" "$calls"
  get_image "$source"
  check_container_network
  install_units "$calls"
  start "$calls"
  check_https "$domain" || true

  echo
  say "Post is running at $domain"
  note "Apps connect to: wss://$domain/v1/ws"
  # Once, on whichever run first gets this far.
  if [[ ! -f $conf/first-invite-shown ]]; then
    note "Your first invite code: $(podman exec post-server postd invite)"
    echo
    note "Keep a copy of the database key somewhere other than this server:"
    note "  podman secret inspect --showsecret --format '{{.SecretData}}' post-db-key"
    note "Without it, the server's data cannot be read."
    touch "$conf/first-invite-shown"
  fi
  echo
  note "More invites:   deploy/server.sh invite"
  note "Newer code:     deploy/server.sh update"
  note "How it is:      deploy/server.sh status"
  note "Announcements:  deploy/server.sh host add YOUR_USERNAME, then post from the app"
  note "Settings:       $conf/post.env (then: systemctl restart post-pod)"
  maintenance_hint
}

cmd_update() {
  local pull=true pass=()
  while (($#)); do
    case $1 in
      --no-pull) pull=false ;;
      --image) pass+=(--image "${2:?--image needs a value}"); shift ;;
      --yes) pass+=(--yes) ;;
      -h | --help) usage ;;
      *) die "unknown option $1" ;;
    esac
    shift
  done
  need_root
  [[ -n $(installed_domain) ]] || die "no Post server is set up here yet: deploy/server.sh install DOMAIN"
  if $pull; then
    say "Pulling the newest code"
    git -C "$repo" pull --ff-only
    # The pull may have changed this script: carry on with the new one.
    exec "$repo/deploy/server.sh" update --no-pull "${pass[@]}"
  fi
  cmd_install "${pass[@]}"
}

cmd_invite() {
  need_root
  podman exec post-server postd invite "$@"
}

# postd's admin commands run inside its container, against its database.
postd_admin() {
  need_root
  [[ $(systemctl is-active post-server || true) == active ]] || die "postd is not running: systemctl start post-server"
  podman exec -i post-server postd "$@"
}

cmd_host() {
  case ${1:-} in
    add | remove) [[ -n ${2:-} ]] || die "which account? deploy/server.sh host $1 USERNAME" ;;
    list) ;;
    *) die "deploy/server.sh host add USER | host remove USER | host list" ;;
  esac
  postd_admin host "$@"
}

cmd_announce() {
  (($#)) || die "what to announce? deploy/server.sh announce TEXT (or - to read it from standard input)"
  if [[ $1 == - ]]; then
    postd_admin announce -
  else
    postd_admin announce "$*" </dev/null
  fi
}

cmd_maintenance() {
  local action=${1:-status}
  (($#)) && shift
  case $action in
    status) postd_admin maintenance status </dev/null ;;
    end) postd_admin maintenance end </dev/null ;;
    plan) maintenance_plan "$@" ;;
    -h | --help) usage ;;
    *) die "deploy/server.sh maintenance plan|end|status" ;;
  esac
}

maintenance_plan() {
  local at='' in='' minutes='' reason='' quiet=()
  while (($#)); do
    case $1 in
      --at) at=${2:?--at needs a time}; shift ;;
      --in) in=${2:?--in needs a length, like "2 hours"}; shift ;;
      --minutes) minutes=${2:?--minutes needs a number}; shift ;;
      --reason) reason=${2?--reason needs text}; shift ;;
      --quiet) quiet=(--quiet) ;;
      --yes) yes=true ;;
      -h | --help) usage ;;
      *) die "unknown option $1" ;;
    esac
    shift
  done
  [[ $minutes =~ ^[0-9]+$ ]] || die "how long will it take? --minutes N"
  [[ -z $at || -z $in ]] || die "give --at or --in, not both"
  local when=()
  if [[ -n $at || -n $in ]]; then
    local secs
    if [[ -n $at ]]; then
      secs=$(date -d "$at" +%s 2>/dev/null) || die "date cannot read the time \"$at\""
    else
      secs=$(date -d "now + $in" +%s 2>/dev/null) || die "date cannot read the length \"$in\""
    fi
    # On the minute: no one plans for 02:00:29.
    secs=$(((secs + 59) / 60 * 60))
    ((secs > $(date +%s))) || die "$(date -d "@$secs" '+%A %-d %B %Y %H:%M %Z') has passed"
    # Said back in words, in this machine's time zone: everyone else sees it
    # in theirs.
    ask "Maintenance $(date -d "@$secs" '+%A %-d %B %Y at %H:%M %Z (UTC%:z)'), for about $minutes minutes?" || exit 1
    when=(--at "@$secs")
  else
    ask "Maintenance starting now, for about $minutes minutes?" || exit 1
  fi
  postd_admin maintenance plan "${when[@]}" --minutes "$minutes" --reason "$reason" "${quiet[@]}" </dev/null
  note "Everyone sees it in their own time zone. To call it off, or say it is over:"
  note "  deploy/server.sh maintenance end"
}

# After an update or a backup during maintenance: a reminder that ending it
# is the operator's call, since it may not be over.
maintenance_hint() {
  if podman exec post-server postd maintenance status 2>/dev/null </dev/null | grep -q '^maintenance underway'; then
    note "Maintenance is underway. When it is over: deploy/server.sh maintenance end"
  fi
}

cmd_status() {
  need_root
  local domain
  domain=$(installed_domain)
  [[ -n $domain ]] || die "no Post server is set up here yet: deploy/server.sh install DOMAIN"
  say "Post at $domain"
  local u
  for u in post-server post-caddy post-livekit; do
    [[ -f $units/${u}.container ]] || continue
    printf '    %-14s %s\n' "$u" "$(systemctl is-active "$u" || true)"
  done
  local created
  created=$(podman image inspect --format '{{.Created}}' "$image" 2>/dev/null | cut -d. -f1 || true)
  note "Server image:  built ${created:-?}"
  if curl -s -o /dev/null --max-time 5 "https://$domain/"; then
    note "HTTPS:         up, certificate valid"
  else
    note "HTTPS:         not answering (podman logs post-caddy)"
  fi
  local live
  live=$(curl -s --max-time 5 "https://$domain/v1/releases/linux-x86_64" | sed -n 's/.*"version":"\([^"]*\)".*/\1/p' || true)
  note "App release:   ${live:-none offered yet}"
}

cmd_push() {
  local host=${1:-}
  [[ -n $host ]] || die "give the server to load the image on: deploy/server.sh push SSH_HOST"
  command -v podman >/dev/null || die "push needs Podman on this computer"
  say "Building the server image"
  podman build -q -f "$repo/deploy/Containerfile" -t "$image" "$repo" >/dev/null
  say "Loading it on $host"
  podman save "$image" | gzip | ssh "$host" 'gunzip | podman load -q'
  echo
  note "Now, on $host, from its clone of the repository:"
  note "  deploy/server.sh update --image local"
}

# --------------------------------------------------------------- backups
#
# A backup is a gzipped tar, encrypted with age, of one folder:
#   backup-info          format, domain, when, which commit
#   post.env             the settings
#   post-db-key          the database key, exactly as stored
#   post-livekit-keys    the secret LiveKit and postd share
#   post-data.tar        the post-data volume: the database and releases
# Caddy's certificates are left out: a new server gets its own.

# A secret's exact bytes. Podman prints them with one newline added.
save_secret() {
  (umask 077 && podman secret inspect --showsecret --format '{{.SecretData}}' "$1" >"$2")
  if [[ $(tail -c1 "$2" | od -An -c | tr -d ' ') == '\n' ]]; then truncate -s -1 "$2"; fi
}

cmd_backup() {
  local out='' to='' stop=false
  while (($#)); do
    case $1 in
      --to) to=${2:?--to needs an age public key}; shift ;;
      --stop) stop=true ;;
      -h | --help) usage ;;
      -*) die "unknown option $1" ;;
      *) out=$1 ;;
    esac
    shift
  done
  need_root
  local domain
  domain=$(installed_domain)
  [[ -n $domain ]] || die "no Post server is set up here"
  [[ -f $conf/post.env ]] || die "this server was set up before post.env: run deploy/server.sh update first"
  [[ -n $to || -t 0 ]] || die "no terminal to type a passphrase in: give an age public key with --to"
  install_packages
  if [[ -z $out ]]; then
    install -d -m 700 "$backups"
    out=$backups/$domain-$(date -u +%Y%m%d-%H%M%S).age
  fi
  [[ ! -e $out && ! -e $out.part ]] || die "$out (or $out.part) exists already"

  local volume
  volume=$(podman volume inspect post-data --format '{{.Mountpoint}}')
  # Room for the copy and for the backup beside it, before anything stops.
  local need free_tmp free_out
  need=$(($(du -sb "$volume" | cut -f1) * 2 + 100 * 1024 * 1024))
  free_tmp=$(df -B1 --output=avail /var/tmp | tail -1)
  free_out=$(df -B1 --output=avail "$(dirname "$out")" | tail -1)
  ((free_tmp >= need && free_out >= need)) || die "not enough free disk space for a backup: it needs about $((need / 1024 / 1024)) MB in /var/tmp and beside $out"

  # It holds the database key: removed however this ends (on_exit).
  work_dir
  local dir=$cleanup_dir/post-backup
  mkdir -m 700 "$dir"
  # The volume's own folder is not in its export, and postd must own it to
  # write its database's journal there.
  printf 'format=1\ndomain=%s\ncreated=%s\ncommit=%s\nvolume_owner=%s\n' \
    "$domain" "$(date -u +%FT%TZ)" "$(git -C "$repo" rev-parse HEAD 2>/dev/null || echo unknown)" "$(stat -c %u:%g "$volume")" >"$dir/backup-info"
  cp "$conf/post.env" "$dir/"
  save_secret post-db-key "$dir/post-db-key"
  save_secret post-livekit-keys "$dir/post-livekit-keys"

  # A consistent copy: the database is written to while postd runs. From
  # here until the backup is whole, any way out starts postd again (on_exit),
  # --stop or not: a failed backup must not leave the server off.
  say "Copying the database (postd stops for a moment)"
  restart_postd=true
  systemctl stop post-server
  podman volume export post-data -o "$dir/post-data.tar" || die "copying the post-data volume failed"
  if ! $stop; then
    # Back at once: the copy is taken.
    systemctl start post-server
    restart_postd=false
  fi

  say "Encrypting the backup"
  local enc=(-r "$to")
  [[ -n $to ]] || enc=(-p)
  # umask: the file is never readable by others, even for a moment. If this
  # fails or is interrupted, the part written is removed with the folder.
  partial_out=$out.part
  (umask 077 && tar -C "$cleanup_dir" -cf - post-backup | gzip | age "${enc[@]}" -o "$partial_out") || die "encrypting the backup failed"
  # Whole: renamed in one step, so FILE is never half written.
  mv "$partial_out" "$out"
  partial_out=''
  if $stop; then
    restart_postd=false
    # Masked, so it stays off through reboots too.
    systemctl mask --now post-server post-caddy post-livekit >/dev/null 2>&1 || true
    systemctl stop post-pod
    note "Post is stopped here and stays off. To start it again instead:"
    note "  systemctl unmask post-server post-caddy post-livekit && systemctl start post-pod"
  fi
  echo
  say "Backed up to $out ($(du -h "$out" | cut -f1))"
  note "It holds the database key, encrypted. Copy it off this server, for example:"
  note "  scp root@$domain:$out ."
  note "To bring it up on a new server: deploy/server.sh restore FILE"
  $stop || maintenance_hint
}

cmd_restore() {
  local file='' identity='' pass=()
  while (($#)); do
    case $1 in
      --identity) identity=${2:?--identity needs an age key file}; shift ;;
      --image) pass+=(--image "${2:?--image needs a value}"); shift ;;
      --yes) yes=true; pass+=(--yes) ;;
      -h | --help) usage ;;
      -*) die "unknown option $1" ;;
      *) file=$1 ;;
    esac
    shift
  done
  need_root
  [[ -n $file ]] || die "give the backup file: deploy/server.sh restore FILE"
  [[ -f $file ]] || die "no such file: $file"
  check_system
  install_packages
  # Never over a server: its key and data would be replaced.
  if [[ -n $(installed_domain) ]] || podman secret exists post-db-key || podman volume exists post-data; then
    die "this machine already has a Post server (or what is left of one). Restore onto a fresh machine."
  fi

  # Unpacked, it holds the database key: removed however this ends.
  work_dir
  say "Decrypting the backup"
  local dec=(-d)
  if [[ -n $identity ]]; then dec+=(-i "$identity"); fi
  age "${dec[@]}" -o "$cleanup_dir/backup.tar.gz" "$file" || die "could not decrypt $file"
  tar -C "$cleanup_dir" -xzf "$cleanup_dir/backup.tar.gz" || die "$file is not a Post backup"
  local dir=$cleanup_dir/post-backup
  grep -qx 'format=1' "$dir/backup-info" 2>/dev/null || die "$file is not a Post backup this script knows"
  local domain
  domain=$(sed -n 's/^POST_DOMAIN=//p' "$dir/post.env" | tail -1)
  [[ -n $domain ]] || die "the backup has no domain in its post.env"
  say "Restoring $domain, backed up $(sed -n 's/^created=//p' "$dir/backup-info")"

  podman secret create post-db-key "$dir/post-db-key" >/dev/null
  podman secret create post-livekit-keys "$dir/post-livekit-keys" >/dev/null
  mkdir -p "$conf"
  install -m 600 "$dir/post.env" "$conf/post.env"
  # Its people have invites already.
  touch "$conf/first-invite-shown"
  podman volume create post-data >/dev/null
  podman volume import post-data "$dir/post-data.tar"
  # Podman gives a new volume to the container's user when it first mounts
  # it empty; this one is not empty, so hand it over as it was. (65532 is
  # postd's user, for a backup that did not record it.)
  local owner
  owner=$(sed -n 's/^volume_owner=//p' "$dir/backup-info")
  chown "${owner:-65532:65532}" "$(podman volume inspect post-data --format '{{.Mountpoint}}')"
  rm -rf -- "$cleanup_dir"
  cleanup_dir=''

  note "Point $domain at this machine if you have not yet: until then the certificate waits."
  cmd_install "$domain" "${pass[@]}"
}

main() {
  local cmd=${1:-}
  (($#)) && shift
  case $cmd in
    install) cmd_install "$@" ;;
    update) cmd_update "$@" ;;
    invite) cmd_invite "$@" ;;
    host) cmd_host "$@" ;;
    announce) cmd_announce "$@" ;;
    maintenance) cmd_maintenance "$@" ;;
    status) cmd_status "$@" ;;
    push) cmd_push "$@" ;;
    backup) cmd_backup "$@" ;;
    restore) cmd_restore "$@" ;;
    -h | --help | help) usage ;;
    *) usage 1 ;;
  esac
}

# All in functions, read before anything runs: `update` changes this file.
main "$@"
exit
