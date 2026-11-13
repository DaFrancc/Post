//! The generator's output for a fixture covering every row of the type table
//! and every annotation, against committed files in `tests/golden/`.
//! `APIGEN_UPDATE_GOLDEN=1 cargo test -p post-apigen` rewrites them; review
//! the diff.

use std::fs;
use std::path::Path;

use post_apigen::{Config, Source};

#[test]
fn outputs_match_the_golden_files() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let files = [("tests/fixtures/sample.rs", ""), ("tests/fixtures/sample_kinds.rs", "kinds")];
    let texts: Vec<String> = files.iter().map(|(f, _)| fs::read_to_string(tests.join("..").join(f)).expect("fixture source")).collect();
    let sources: Vec<Source<'_>> = files.iter().zip(&texts).map(|((path, module), text)| Source { path, module, text }).collect();
    let model = post_apigen::parse(&sources).unwrap_or_else(|e| panic!("the fixture should parse:\n{e}"));
    let update = std::env::var_os("APIGEN_UPDATE_GOLDEN").is_some();
    let mut differ = Vec::new();
    for out in post_apigen::emit(&model, &Config::new("post.v1")) {
        let name = Path::new(out.path).file_name().expect("a file name");
        let golden = tests.join("golden").join(name);
        if update {
            fs::create_dir_all(golden.parent().expect("a folder")).expect("golden folder");
            fs::write(&golden, &out.contents).expect("write golden");
        } else if fs::read_to_string(&golden).ok().as_deref() != Some(out.contents.as_str()) {
            differ.push(name.to_string_lossy().into_owned());
        }
    }
    assert!(
        differ.is_empty(),
        "{differ:?} differ from tests/golden/; if the change is intended, rerun with APIGEN_UPDATE_GOLDEN=1 and review the diff"
    );
}
