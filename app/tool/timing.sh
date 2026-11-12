#!/usr/bin/env bash
# Frame timings around the welcome screen's buttons, in a profile build
# (integration_test/signup_timing_test.dart). Each scenario runs in its own
# launch, so neither warms the app up for the other. A window opens while it
# runs. Reports land in app/build/timing-*.txt.
#
#   app/tool/timing.sh [create|link ...]
set -euo pipefail

app=$(cd "$(dirname "$0")/.." && pwd)
cargo build -q --release --manifest-path "$app/../Cargo.toml" -p post-server -p post-cli
cd "$app"
pids="$app/build/timing.pids"
cleanup() {
  [[ -f $pids ]] && xargs -r kill 2>/dev/null <"$pids" || true
  rm -f "$pids"
}
trap cleanup EXIT
for scenario in "${@:-create link}"; do
  for s in $scenario; do
    flutter drive --profile -d linux \
      --driver=test_driver/integration_test.dart \
      --target=integration_test/signup_timing_test.dart \
      --dart-define=SCENARIO="$s" >/dev/null
    cleanup
    cat "build/timing-$s.txt"
  done
done
