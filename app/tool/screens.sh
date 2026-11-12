#!/usr/bin/env bash
# Screenshots of the app's main screens, from a throwaway server filled with
# test accounts (integration_test/screens_test.dart). A window opens while
# it runs. Leaves PNGs in app/build/screens/ (or the directory given).
#
#   app/tool/screens.sh [OUTPUT_DIR]          everything
#   app/tool/screens.sh calls [OUTPUT_DIR]    calls only (calls_test.dart)
set -euo pipefail

test=integration_test/screens_test.dart
if [[ ${1:-} == calls ]]; then
  test=integration_test/calls_test.dart
  shift
fi
app=$(cd "$(dirname "$0")/.." && pwd)
out=$(realpath -m "${1:-$app/build/screens}")
# Fedora's ccache puts wrappers first in PATH; Flutter's native-asset builds
# (flutter_soloud) follow the clang link to ccache itself and fail. Use the
# compilers directly.
PATH=$(tr ':' '\n' <<<"$PATH" | grep -v '^/usr/lib64/ccache$' | paste -sd:)
cargo build -q --release --manifest-path "$app/../Cargo.toml" -p post-server -p post-cli
rm -rf "$out"
cd "$app"
# The test lists the server and accounts it starts; stop them however the
# test ends, even if it crashed before tearing down.
cleanup() {
  for pids in "$app"/build/*.pids; do
    [[ -f $pids ]] && xargs -r kill 2>/dev/null <"$pids" || true
    rm -f "$pids"
  done
  podman stop post-test-livekit >/dev/null 2>&1 || true
}
trap cleanup EXIT
# Spellcheck dictionaries download once and are kept between runs.
export POST_DICTIONARIES="$app/build/dictionaries"
# Calls: LiveKit in a container, when Podman is there.
if command -v podman >/dev/null; then
  keys="$app/build/livekit-keys"
  [[ -f $keys ]] || (umask 077; printf 'post: %s\n' "$(head -c 36 /dev/urandom | base64 | tr -d '/+=')" > "$keys")
  podman rm -f post-test-livekit >/dev/null 2>&1 || true
  podman run -d --rm --name post-test-livekit --network host -v "$keys:/keys.yaml:ro,Z" \
    docker.io/livekit/livekit-server:v1.13.11 --bind 127.0.0.1 --node-ip 127.0.0.1 --key-file /keys.yaml >/dev/null
  export POST_TEST_LIVEKIT_URL=ws://127.0.0.1:7880 POST_TEST_LIVEKIT_KEYS="$keys"
fi
flutter test "$test" -d linux --dart-define=SHOTS="$out" "${@:2}"
echo "screenshots in $out"
