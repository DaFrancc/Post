# Deploying Post

A Post server is one pod with two containers (design section 12):

- **Caddy**: terminates TLS on port 443 with a Let's Encrypt certificate and
  forwards to postd. Writes no access log, so client addresses are not
  recorded.
- **postd**: the Post server, listening on localhost inside the pod. Its
  database is encrypted at rest with a key held as a Podman secret.
- **LiveKit** (optional): carries calls' audio and video. The media is
  encrypted end to end with keys only the people in the call derive, so it
  relays frames it cannot read. See "Calls" below.

You need a host with a public IP, a DNS name pointing at it (here
`chat.example.org`), ports 80 and 443 open, and Podman 4.4 or later. For
calls, also ports 7881 (TCP) and 7882 (UDP).

## Quick setup (Debian 13)

On a Debian 13 server, as root, with the domain already pointing at it:

```sh
apt update && apt install -y git
git clone https://github.com/DaFrancc/Post.git && cd Post
deploy/server.sh install chat.example.org
```

The server runs releases: the script switches the clone to the newest server
release (a `server-vX.Y.Z` tag) before it starts.

It installs what it needs (Podman and a few tools), offers to stop a web
server already on ports 80 and 443, opens ufw or firewalld, makes the
secrets, writes the settings to `~/.config/post/post.env`, starts
everything, waits for the HTTPS certificate and prints your first invite.
Run it again any time: it keeps your secrets and settings. Add `--no-calls`
to offer no calls.

Afterwards:

```sh
deploy/server.sh invite        # an invite code (--bot, --days N)
deploy/server.sh url           # the address apps sign in with: wss://DOMAIN/v1/ws
deploy/server.sh status        # what is running, HTTPS, the app release offered
deploy/server.sh update        # the newest server release: check it out, its image, restart
deploy/server.sh update --version 0.3.0   # a particular one, newer or older
```

**Versions.** The server and the app have their own versions: the server's
releases are `server-vX.Y.Z` tags, the app's `app-vX.Y.Z`, and either is
released only when it changed. They agree through the protocol, not their
numbers, so any recent app works with any recent server.

**The server image.** The script downloads the image CI built for the
server release (`ghcr.io/dafrancc/post-server:X.Y.Z`), so the server never
compiles anything. CI builds one when a server release is tagged, a few
minutes after. If there is none yet, the script says so and asks before
building here instead, which takes a long time on a small server. Two other
ways, which use whatever this clone has checked out:

```sh
deploy/server.sh install chat.example.org --image build   # build it on the server
deploy/server.sh push root@chat.example.org               # build on your computer and send it,
deploy/server.sh install chat.example.org --image local   # then use it on the server
```

The steps below are what the script does, for doing it by hand or on another
system.

## 1. Build the server image

From the repository root:

```sh
podman build -f deploy/Containerfile -t localhost/postd:latest .
```

## 2. Create the database key

```sh
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' | podman secret create post-db-key -
```

Back the key up somewhere safe and separate from the data. Without it the
database cannot be read; with it and a copy of the data, the usernames, device
lists and registration dates can be. Messages are never readable from server
data either way.

## 3. Install the pod (Quadlet, managed by systemd)

```sh
mkdir -p ~/.config/containers/systemd ~/.config/post
cp deploy/quadlet/* ~/.config/containers/systemd/
cp deploy/Caddyfile deploy/livekit.yaml deploy/post.env ~/.config/post/
```

In `~/.config/post/post.env`, put your domain in place of `chat.example.org`
(twice). Every other setting is there too, commented out at its default.
**The domain is bound into every account and cannot change later.**

Create the secret postd and LiveKit share for calls. postd mounts it even
with calls off, so make it either way:

```sh
printf 'post: %s\n' "$(head -c 36 /dev/urandom | base64 | tr -d '/+=')" | podman secret create post-livekit-keys -
```

To offer no calls, delete the `POST_LIVEKIT_URL` line in `post.env`,
`post-livekit.container`, and the two call ports in `post.pod`.

To bind ports 80 and 443 as an unprivileged user, allow it once:

```sh
sudo sysctl -w net.ipv4.ip_unprivileged_port_start=80
echo 'net.ipv4.ip_unprivileged_port_start=80' | sudo tee /etc/sysctl.d/50-post.conf
loginctl enable-linger "$USER"   # keep it running after you log out
```

Then start it:

```sh
systemctl --user daemon-reload
systemctl --user start post-pod
```

## 4. Invite people

```sh
podman exec post-server postd invite            # a person
podman exec post-server postd invite --bot      # a bot account
podman exec post-server postd invite --days 1   # expires sooner
```

Each code works once. People register with:

```sh
post keygen ~/.config/post/db.key
post --db-key-file ~/.config/post/db.key register \
    --server wss://chat.example.org/v1/ws --invite CODE --username NAME
```

## 5. Check post-quantum TLS

The design requires the hybrid `X25519MLKEM768` key exchange. With OpenSSL 3.5
or later:

```sh
openssl s_client -connect chat.example.org:443 -groups X25519MLKEM768 </dev/null 2>/dev/null \
  | grep -E 'Negotiated TLS1.3 group|Server Temp Key'
```

It should name `X25519MLKEM768`. The Post client prefers this group, so it
is what clients use whenever the server offers it.

## Calls

Calls run through LiveKit in the same pod. postd hands out a short-lived
token to anyone who proves they are in a conversation, and LiveKit relays the
call's media, which is encrypted end to end. LiveKit only ever sees a random
name per call for each participant, never who they are.

You set the limits, so calls cannot outgrow your bandwidth:

| Setting (in `post.env`) | Default | What it does |
|---|---|---|
| `POST_CALL_MAX_PARTICIPANTS` | `25` | The most people in one call |
| `POST_CALL_MAX_BITRATE_KBPS` | `2500` | The most each person may send. Apps step their quality down to fit |

A rough guide to bandwidth: a voice call uses about 50 kbit/s per person
each way, and video up to the bitrate limit. Each person also receives
everyone else's streams, so a call with N people uses about N × (N − 1)
streams through the server.

## App updates

Apps get updates from their own server, so nobody else sees who runs Post.
The server never updates itself. It hands new versions of the app to the
people who use it, and each person's app still decides when to install.
You decide how much the server does by itself:

| Setting | Default | What it does |
|---|---|---|
| `POST_RELEASES_CHECK` | `true` | Ask GitHub for a new release every hour |
| `POST_RELEASES_DOWNLOAD` | `true` | Download it into `/var/lib/post/releases` and check its signature and files |
| `POST_RELEASES_LIVE_WHILE_RUNNING` | `false` | Offer it to apps as soon as it has downloaded |
| `POST_RELEASES_LIVE_ON_START` | `true` | When the server starts, offer apps the newest release: downloaded before, or found as it starts |

With the defaults, a release that comes out while the server runs waits on the
server until it next restarts. A new server offers the latest release from the
start.

- To put it live now instead, run `systemctl reload post-server` (with
  `--user` for a rootless install).
  Without systemd, send `postd` the HUP signal
  (`podman kill --signal HUP post-server`, or `kill -HUP <pid>`). A reload is
  you asking, so it does everything whatever the settings say: it re-reads
  the releases folder, asks GitHub, downloads, and puts the newest release
  live. Nobody's connection drops.
- With `POST_RELEASES_LIVE_ON_START=false`, a restart keeps offering the
  release that was live before, and only a reload changes it.
- Set these in `~/.config/post/post.env`, then `systemctl restart post-pod`.
- `POST_RELEASES_FROM=owner/repo` fetches from another repository. Leave it
  empty to fetch nothing.
- To skip GitHub altogether, copy a release in by hand: a folder named after
  its version, holding its files, `manifest-<platform>.json` and the `.sig`
  file. Then reload.

## Backups

Losing the server's data loses only messages not yet delivered and the
account registry: people keep their own history. But without the registry
nobody can log in, so keep backups.

```sh
deploy/server.sh backup                      # asks for a passphrase
deploy/server.sh backup --to age1...         # to an age public key, for cron
```

A backup is one file in `/root/post-backups/` holding the database, both
secrets and `post.env`, encrypted with [age](https://age-encryption.org). It
holds the database key, so it is always encrypted: with a passphrase you type,
or to a public key (`age-keygen -o key.txt` on your own computer prints it),
which lets it run unattended without this server being able to open its own
backups. Copy it off the server. postd stops for a few seconds while the
database is copied: messages wait in apps' outboxes meanwhile, and calls in
progress carry on.

Bring it up on a new machine, where it never overwrites an existing server:

```sh
deploy/server.sh restore FILE                         # passphrase
deploy/server.sh restore FILE --identity key.txt      # age key
```

**Moving to another machine.** Back up with `--stop`, which leaves the old
server off for good, so nothing reaches it after the copy: apps keep their
messages until they reach the new one. Then restore on the new machine and
point the domain at it. Apps carry on by themselves once the name resolves to
the new address.

By hand: stop `post-server`, `podman volume export post-data`, and keep the
`post-db-key` secret separately.

## Announcements and maintenance

Everyone on the server sees one channel of announcements, pinned at the top of
their conversation list. They show as from the server's domain and are never
deleted. Choose who may post from the app:

```sh
deploy/server.sh host add YOUR_USERNAME     # host remove USER, host list
deploy/server.sh announce "New: calls work in groups."   # or post from here
```

Before taking the server down, plan it: everyone is told in their own time
zone, and reminded a day, an hour, 30 minutes and 5 minutes before. Hosts can
do the same from the app, by typing `/maintenance` in the channel.

```sh
deploy/server.sh maintenance plan --at "2026-11-14 02:00" --minutes 15 --reason "moving to a new machine"
deploy/server.sh maintenance plan --in "2 hours" --minutes 10
deploy/server.sh maintenance plan --minutes 10   # starting now
deploy/server.sh maintenance end                 # cancel it, or say it is over
```

`--at` is in this machine's time zone, and is said back in words before
anything is sent. The server announces the start itself, and that it is over
when the time is up; end it sooner with `maintenance end`. A routine backup
needs none of this: postd is down for seconds, and messages wait in apps.

## Alternative: compose

`deploy/compose.yaml` runs the same containers with `podman compose` or
`docker compose`. Put the database key in `deploy/post-db-key` and the calls
secret (`post: <secret>`) in `deploy/post-livekit-keys`, both mode 0600, and
set `POST_DOMAIN`.
