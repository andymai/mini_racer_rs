//! Writes `crates/mini_racer_wire/tests/golden/<name>.bin`: the bytes V8
//! serializes for each expression in that directory's `corpus.tsv`.

use std::error::Error;
use std::path::Path;

use mini_racer_engine::{ContextConfig, ContextHandle, Host, WaitHandle, WaitOutcome};

struct Parking;

impl Host for Parking {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        self.wait_blocking(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        while !wait.ready() {
            wait.park();
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mini_racer_wire/tests/golden");
    let corpus = std::fs::read_to_string(golden.join("corpus.tsv"))?;
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking)?;
    for line in corpus.lines().filter(|line| !line.starts_with('#')) {
        let (name, source) = line.split_once('\t').ok_or("corpus line without a tab")?;
        let bytes = context
            .eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
            .map_err(|error| format!("{name}: {error}"))?;
        std::fs::write(golden.join(format!("{name}.bin")), bytes)?;
    }
    Ok(())
}
