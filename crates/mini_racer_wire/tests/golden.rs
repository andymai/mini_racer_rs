//! Decodes what V8 itself wrote for each corpus expression and compares the
//! rendering with the reviewed `.txt` beside it. `UPDATE_GOLDEN=1` rewrites
//! the `.txt` files; regenerate the `.bin` files with
//! `cargo run -p mini_racer_engine --example gen_wire_golden`.

#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use common::render;
use mini_racer_wire::Limits;

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn corpus() -> Vec<String> {
    std::fs::read_to_string(golden().join("corpus.tsv"))
        .expect("corpus.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split_once('\t').expect("tab").0.to_owned())
        .collect()
}

#[test]
fn v8_output_decodes_as_reviewed() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut stale = Vec::new();
    for name in corpus() {
        let bytes = std::fs::read(golden().join(format!("{name}.bin"))).expect("bin");
        let rendered = render(&bytes, Limits::default()) + "\n";
        let expected = golden().join(format!("{name}.txt"));
        if update {
            std::fs::write(&expected, &rendered).expect("write");
        } else if std::fs::read_to_string(&expected).ok().as_deref() != Some(&rendered) {
            stale.push(format!("{name}: {rendered}"));
        }
    }
    assert!(stale.is_empty(), "renderings differ:\n{}", stale.join(""));
}

#[test]
fn every_golden_file_has_a_corpus_entry() {
    let names: HashSet<String> = corpus().into_iter().collect();
    let orphans: Vec<PathBuf> = std::fs::read_dir(golden())
        .expect("golden dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            let generated = matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("bin" | "txt")
            );
            let stem = path.file_stem().and_then(|stem| stem.to_str());
            generated && !stem.is_some_and(|stem| names.contains(stem))
        })
        .collect();
    assert!(orphans.is_empty(), "no corpus entry for {orphans:?}");
}

#[test]
fn every_prefix_of_v8_output_decodes_or_fails_without_panicking() {
    for name in corpus() {
        let bytes = std::fs::read(golden().join(format!("{name}.bin"))).expect("bin");
        for end in 0..bytes.len() {
            let _ = render(&bytes[..end], Limits::default());
        }
    }
}
