#!/usr/bin/env bash
# Screenshots of the app's main screens, from a throwaway server filled with
# test accounts (integration_test/screens_test.dart). A window opens while
# it runs. Leaves PNGs in app/build/screens/ (or the directory given).
#
#   app/tool/screens.sh [OUTPUT_DIR]
set -euo pipefail

app=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "${1:-$app/build/screens}")
cargo build -q --release --manifest-path "$app/../Cargo.toml" -p post-server -p post-cli
rm -rf "$out"
cd "$app"
# The test lists the server and accounts it starts; stop them however the
# test ends, even if it crashed before tearing down.
pids="$app/build/screens.pids"
cleanup() {
  [[ -f $pids ]] && xargs -r kill 2>/dev/null <"$pids" || true
  rm -f "$pids"
}
trap cleanup EXIT
# Spellcheck dictionaries download once and are kept between runs.
export POST_DICTIONARIES="$app/build/dictionaries"
flutter test integration_test/screens_test.dart -d linux --dart-define=SHOTS="$out" "${@:2}"
echo "screenshots in $out"
