#!/usr/bin/env bash
# The tests CI runs: every Rust test (end to end included) on debug builds,
# whose dependencies are optimised (root Cargo.toml), and the app's unit tests.
#
#   scripts/test.sh
#
# The app's integration tests open windows and run a server; they are run
# through app/tool/screens.sh, or one at a time with
#   cd app && flutter test integration_test/NAME_test.dart -d linux
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

echo "== cargo test"
cargo test -q --manifest-path "$root/Cargo.toml" --workspace

echo "== flutter test"
cd "$root/app"
flutter test
