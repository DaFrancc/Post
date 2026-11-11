#!/usr/bin/env bash
# Screenshots of the app's main screens, from a throwaway server filled with
# test accounts (integration_test/screens_test.dart). A window opens while
# it runs. Leaves PNGs in app/build/screens/ (or the directory given).
#
#   app/tool/screens.sh [OUTPUT_DIR]
set -euo pipefail

app=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "${1:-$app/build/screens}")
cargo build -q --release --manifest-path "$app/../Cargo.toml" -p postern-server -p postern-cli
rm -rf "$out"
cd "$app"
flutter test integration_test/screens_test.dart -d linux --dart-define=SHOTS="$out" "${@:2}"
echo "screenshots in $out"
