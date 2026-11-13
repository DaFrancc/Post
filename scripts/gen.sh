#!/usr/bin/env bash
# Regenerate every generated file. Generated files are committed and never
# edited or formatted by hand (docs/architecture.md, "Generated code").
#
#   scripts/gen.sh
#
# Today that is the flutter_rust_bridge bridge (crates/post-ffi/src/
# frb_generated.rs, app/lib/src/rust/), which needs
# flutter_rust_bridge_codegen 2.13.0:
#   cargo install flutter_rust_bridge_codegen --version 2.13.0 --locked
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

echo "== flutter_rust_bridge"
cd "$root/app"
flutter_rust_bridge_codegen generate
