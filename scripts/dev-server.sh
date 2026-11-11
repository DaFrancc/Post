#!/usr/bin/env bash
# A local Postern server for development, at ws://127.0.0.1:8080/v1/ws.
# Its data and database key live in .dev/server/ in the repository (ignored
# by git). Plain ws:// is for this local server only; real servers use
# wss:// behind Caddy (deploy/README.md).
#
#   scripts/dev-server.sh            run the server (Ctrl-C stops it)
#   scripts/dev-server.sh invite     print a new invite code
#   scripts/dev-server.sh reset      delete the development server's data
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/.dev/server"
addr=127.0.0.1:8080
bin="$root/target/debug/posternd"
export POSTERN_DB="$dir/postern.db" POSTERN_DB_KEY_FILE="$dir/db.key"

build() { cargo build -q --manifest-path "$root/Cargo.toml" -p postern-server; }
prepare() {
  install -d -m 700 "$dir"
  [[ -f $POSTERN_DB_KEY_FILE ]] || "$bin" keygen "$POSTERN_DB_KEY_FILE" >/dev/null
}

case ${1:-serve} in
  serve)
    build
    prepare
    echo "Postern development server: ws://$addr/v1/ws  (scripts/dev-server.sh invite for an invite)"
    exec "$bin" serve --domain "$addr" --listen "$addr"
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
