//! Decodes what V8 itself wrote for each corpus expression and compares the
//! rendering with the reviewed `.txt` beside it. `UPDATE_GOLDEN=1` rewrites
//! the `.txt` files; regenerate the `.bin` files with
//! `cargo run -p mini_racer_engine --example gen_wire_golden`.

mod common;

use std::path::{Path, PathBuf};

use common::Tree;
use mini_racer_wire::{Limits, decode_value};

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

fn render(bytes: &[u8]) -> String {
    let mut tree = Tree::default();
    match decode_value(bytes, Limits::default(), &mut tree) {
        Ok(root) => tree.render(root),
        Err(error) => format!("error: {error}"),
    }
}

#[test]
fn v8_output_decodes_as_reviewed() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut stale = Vec::new();
    for name in corpus() {
        let bytes = std::fs::read(golden().join(format!("{name}.bin"))).expect("bin");
        let rendered = render(&bytes) + "\n";
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
fn every_prefix_of_v8_output_decodes_or_fails_without_panicking() {
    for name in corpus() {
        let bytes = std::fs::read(golden().join(format!("{name}.bin"))).expect("bin");
        for end in 0..bytes.len() {
            let _ = render(&bytes[..end]);
        }
    }
}
