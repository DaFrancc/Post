//! The committed generated files are exactly what the generator writes from
//! the core today, so `cargo test --workspace` fails on a stale one.

#[test]
fn generated_files_are_up_to_date() {
    let root = post_apigen::workspace_root();
    let outputs = post_apigen::generate(&root).unwrap_or_else(|e| panic!("the core's #[api] annotations have problems:\n{e}"));
    let stale = post_apigen::stale(&root, &outputs);
    assert!(stale.is_empty(), "{stale:?} are stale: run `cargo run -p post-apigen` (or scripts/gen.sh)");
}
