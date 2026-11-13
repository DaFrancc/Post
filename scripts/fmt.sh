#!/usr/bin/env bash
# Format the Rust and the app's hand-written Dart with the committed settings
# (rustfmt.toml, app/analysis_options.yaml). Generated code keeps its
# generator's format: frb_generated.rs opts out with #[rustfmt::skip], and the
# Dart under app/lib/src/rust/ is never passed to the formatter.
#
#   scripts/fmt.sh            format in place
#   scripts/fmt.sh --check    change nothing; fail and name what is unformatted
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
check=false
case ${1:-} in
  --check) check=true ;;
  '') ;;
  *) echo "usage: fmt.sh [--check]" >&2; exit 2 ;;
esac

cd "$root"
if $check; then cargo fmt --all -- --check; else cargo fmt --all; fi

cd "$root/app"
mapfile -t dart < <(find lib test integration_test tool -name '*.dart' -not -path 'lib/src/rust/*' | sort)
if $check; then
  dart format --output=none --set-exit-if-changed "${dart[@]}"
else
  dart format "${dart[@]}"
fi
