# infinite_db_step

Import [STEP](https://en.wikipedia.org/wiki/ISO_10303-21) (ISO 10303-21) CAD files into [InfiniteDB](https://crates.io/crates/infinite-db) spatial records: BREP topology as hyperedges, faces with signed-distance boundary functions, and Hilbert-friendly spatial addressing.

## Installation

```toml
[dependencies]
infinite_db_step = "0.1"
infinite-db = "0.1"
```

Optional progress and per-entity logging:

```toml
infinite_db_step = { version = "0.1", features = ["log"] }
```

Initialize a logger in your binary (for example `env_logger`) when using the `log` feature.

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
