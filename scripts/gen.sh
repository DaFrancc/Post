#!/usr/bin/env bash
# Regenerate every generated file. Generated files are committed and never
# edited or formatted by hand (docs/architecture.md, "Generated code").
#
#   scripts/gen.sh
#
# The local API (post-apigen, from post-core's #[api] annotations): the
# dispatch and OpenRPC file in crates/post-api, the Dart client in
# app/lib/api, and the fixtures and their tests.
#
# The flutter_rust_bridge bridge (crates/post-ffi/src/frb_generated.rs,
# app/lib/src/rust/), which needs flutter_rust_bridge_codegen 2.13.0:
#   cargo install flutter_rust_bridge_codegen --version 2.13.0 --locked
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)

echo "== local API"
cargo run -q --manifest-path "$root/Cargo.toml" -p post-apigen

echo "== flutter_rust_bridge"
cd "$root/app"
flutter_rust_bridge_codegen generate
