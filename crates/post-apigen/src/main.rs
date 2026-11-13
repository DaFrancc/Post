//! Write the local API's generated files from post-core's `#[api]`
//! annotations (docs/phase-1-api.md).
//!
//! ```sh
//! cargo run -p post-apigen             # write every generated file
//! cargo run -p post-apigen -- --check  # write nothing; exit 1 and name any stale file
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(_) => {
            eprintln!("usage: post-apigen [--check]");
            return ExitCode::from(2);
        }
    };
    let root = post_apigen::workspace_root();
    let outputs = match post_apigen::generate(&root) {
        Ok(outputs) => outputs,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if check {
        let stale = post_apigen::stale(&root, &outputs);
        if stale.is_empty() {
            return ExitCode::SUCCESS;
        }
        for path in stale {
            eprintln!("stale: {path}");
        }
        eprintln!("run `cargo run -p post-apigen`");
        return ExitCode::FAILURE;
    }
    match post_apigen::write(&root, &outputs) {
        Ok(()) => {
            for o in &outputs {
                println!("wrote {}", o.path);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
