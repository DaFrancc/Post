#!/usr/bin/env bash
# Every static check CI runs: formatting (changes nothing), clippy with
# warnings as errors, and flutter analyze. Tests are scripts/test.sh.
#
#   scripts/check.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

echo "== formatting"
"$root/scripts/fmt.sh" --check

echo "== clippy"
cargo clippy -q --manifest-path "$root/Cargo.toml" --workspace --all-targets -- -D warnings

echo "== flutter analyze"
cd "$root/app"
flutter analyze
