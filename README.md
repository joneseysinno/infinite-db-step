# infinite_db_step

Import [STEP](https://en.wikipedia.org/wiki/ISO_10303-21) (ISO 10303-21) CAD files into [InfiniteDB](https://crates.io/crates/infinite-db) spatial records: BREP topology as hyperedges, faces with signed-distance boundary functions, and Hilbert-friendly spatial addressing.

## Installation

```toml
[dependencies]
infinite_db_step = "0.2"
infinite-db = "0.2"
```

Optional progress and per-entity logging:

```toml
infinite_db_step = { version = "0.2", features = ["log"] }
```

Parallel STEP parsing and encoding are enabled by default. To disable:

```toml
infinite_db_step = { version = "0.2", default-features = false }
```

Initialize a logger in your binary (for example `env_logger`) when using the `log` feature.

## Command-line import

Build and run the `import-step` binary:

```text
cargo run --bin import-step -- model.step ./my_database
cargo run --bin import-step -- model.step ./my_database --json dump.json
```

Or run an ignored integration test with environment variables:

```text
STEP_FILE=model.step DB_DIR=./my_database cargo test --test import_step_file -- --ignored --nocapture
```

Optional `JSON_DUMP=./dump.json` writes a JSON sidecar.

## Example

```rust
use infinite_db_step::{import_step_file, EncoderConfig, WriteOptions};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let stats = import_step_file(
        Path::new("model.step"),
        Path::new("my_database"),
        &EncoderConfig::default(),
        WriteOptions::default(),
    )?;
    println!("wrote {} records (revision {})", stats.records_written, stats.final_revision);
    Ok(())
}
```

## Logging

With the `log` feature enabled:

- Large writes (`>= 100` inserts) emit `info!` milestones (start, progress every 250 items, sealing).
- `WriteOptions { verbose: true, .. }` adds `debug!` lines for every entity and hyperedge.

Without the feature, the library is fully silent on stdout/stderr.

## License

MIT
