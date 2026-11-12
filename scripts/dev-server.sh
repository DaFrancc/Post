#!/usr/bin/env bash
# A local Post server for development, at ws://127.0.0.1:8080/v1/ws.
# Its data and database key live in .dev/server/ in the repository (ignored
# by git). Plain ws:// is for this local server only; real servers use
# wss:// behind Caddy (deploy/README.md).
#
#   scripts/dev-server.sh            run the server (Ctrl-C stops it)
#   POST_RELEASES_FROM=DaFrancc/Post scripts/dev-server.sh
#                                    ...also fetching app releases from GitHub;
#                                    they go live at the next start, or now
#                                    with: kill -HUP $(pgrep postd)
#   scripts/dev-server.sh invite     print a new invite code
#   scripts/dev-server.sh reset      delete the development server's data
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/.dev/server"
addr=127.0.0.1:8080
bin="$root/target/debug/postd"
export POST_DB="$dir/post.db" POST_DB_KEY_FILE="$dir/db.key"

build() { cargo build -q --manifest-path "$root/Cargo.toml" -p post-server; }
prepare() {
  install -d -m 700 "$dir"
  [[ -f $POST_DB_KEY_FILE ]] || "$bin" keygen "$POST_DB_KEY_FILE" >/dev/null
}

livekit_image=docker.io/livekit/livekit-server:v1.13.11

# Calls: LiveKit in a container beside postd, sharing one key file, when
# Podman is there. Its media ports are 7881 (TCP) and 7882 (UDP).
start_livekit() {
  command -v podman >/dev/null || { echo "(no podman: calls are off)"; return 1; }
  local keys="$dir/livekit-keys"
  [[ -f $keys ]] || (umask 077; printf 'post: %s\n' "$(head -c 36 /dev/urandom | base64 | tr -d '/+=')" > "$keys")
  podman rm -f post-dev-livekit >/dev/null 2>&1 || true
  podman run -d --rm --name post-dev-livekit --network host \
    -v "$keys:/keys.yaml:ro,Z" "$livekit_image" \
    --bind 127.0.0.1 --node-ip 127.0.0.1 --key-file /keys.yaml >/dev/null
  echo "LiveKit for calls: ws://127.0.0.1:7880"
}

case ${1:-serve} in
  serve)
    build
    prepare
    calls=()
    if start_livekit; then
      trap 'podman stop post-dev-livekit >/dev/null 2>&1 || true' EXIT
      calls=(--livekit-url ws://127.0.0.1:7880 --livekit-secret-file "$dir/livekit-keys")
    fi
    echo "Post development server: ws://$addr/v1/ws  (scripts/dev-server.sh invite for an invite)"
    # Releases from its own folder; fetched from GitHub only when asked
    # (POST_RELEASES_FROM=DaFrancc/Post scripts/dev-server.sh).
    "$bin" serve --domain "$addr" --listen "$addr" --releases-dir "$dir/releases" --releases-from "${POST_RELEASES_FROM:-}" "${calls[@]}"
    ;;
  invite)
    build
    prepare
    "$bin" invite --days 7
    ;;
  reset)
    rm -rf "$dir"
    echo "deleted $dir"
    ;;
  *)
    echo "usage: $0 [serve|invite|reset]" >&2
    exit 2
    ;;
esac
