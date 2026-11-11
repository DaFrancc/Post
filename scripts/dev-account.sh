#!/usr/bin/env bash
# Act as a test account on the local development server (scripts/dev-server.sh),
# through the CLI. Profiles and keys live in .dev/accounts/ (ignored by git);
# an account that does not exist yet is registered with a fresh invite.
#
#   scripts/dev-account.sh alice send --to sam "hello"     DM sam
#   scripts/dev-account.sh alice group create climbing sam bob
#   scripts/dev-account.sh alice conversations              IDs for --conversation
#
# Any postern command works after the name; see `postern --help`.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
name=${1:?usage: dev-account.sh NAME COMMAND...}
shift
dir="$root/.dev/accounts"
bin="$root/target/debug/postern"
url=ws://127.0.0.1:8080/v1/ws

cargo build -q --manifest-path "$root/Cargo.toml" -p postern-cli
install -d -m 700 "$dir"
[[ -f $dir/$name.key ]] || "$bin" keygen "$dir/$name.key" >/dev/null
p() { "$bin" --profile "$dir/$name" --db-key-file "$dir/$name.key" "$@"; }
if [[ ! -f $dir/$name/profile.db ]]; then
  p register --server "$url" --invite "$("$root/scripts/dev-server.sh" invite)" --username "$name" >/dev/null
  echo "registered $name" >&2
fi
p "$@"
