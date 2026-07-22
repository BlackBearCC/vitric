//! Every example under `examples/` must pass `vitric check` — the README promises each example
//! is validated this way, and this test is what keeps that promise true. Runs the same
//! `runtime::check` the CLI uses; headless, no GPU / window / display required.

use std::fs;
use std::path::PathBuf;

use vitric_cli::runtime;

fn examples_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

#[test]
fn every_example_passes_check() {
    let root = examples_root();
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir() && p.join("vitric.json").exists())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no examples found under {}", root.display());

    // Collect all failures before asserting so one bad example does not hide the others.
    let mut failures = Vec::new();
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        match runtime::check(dir) {
            Ok(_) => eprintln!("check PASS: {name}"),
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "examples failing `vitric check`:\n{}",
        failures.join("\n")
    );
}
