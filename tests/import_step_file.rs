//! Manual STEP file import from the command line.
//!
//! ## Via environment variables
//!
//! ```text
//! STEP_FILE=C:\models\part.step DB_DIR=C:\out\my_db cargo test --test import_step_file -- --ignored --nocapture
//! ```
//!
//! Optional: `JSON_DUMP=C:\out\dump.json` for a JSON sidecar.
//!
//! ## Via the CLI binary
//!
//! ```text
//! cargo run --bin import-step -- C:\models\part.step C:\out\my_db
//! cargo run --bin import-step -- model.step ./my_database --json dump.json
//! ```

use std::path::Path;
use std::time::Instant;

use infinite_db_step::{import_step_file_with_json, EncoderConfig, WriteOptions};

#[test]
#[ignore = "manual: set STEP_FILE and DB_DIR env vars, or use `cargo run --bin import-step`"]
fn import_step_from_env() {
    let step_file = std::env::var("STEP_FILE").expect(
        "set STEP_FILE to the path of a .step/.stp file, e.g.\n  \
         STEP_FILE=model.step DB_DIR=out_db cargo test --test import_step_file -- --ignored --nocapture",
    );
    let db_dir = std::env::var("DB_DIR").expect(
        "set DB_DIR to the output InfiniteDB directory, e.g.\n  \
         STEP_FILE=model.step DB_DIR=out_db cargo test --test import_step_file -- --ignored --nocapture",
    );

    let step_path = Path::new(&step_file);
    assert!(
        step_path.is_file(),
        "STEP file not found: {}",
        step_path.display()
    );

    let json_dump = std::env::var("JSON_DUMP").ok().map(|p| Path::new(p.as_str()).to_path_buf());

    eprintln!("importing {} -> {}", step_path.display(), db_dir);
    if let Some(ref j) = json_dump {
        eprintln!("json dump: {}", j.display());
    }

    let start = Instant::now();
    let stats = import_step_file_with_json(
        step_path,
        Path::new(&db_dir),
        &EncoderConfig::default(),
        WriteOptions::default(),
        json_dump.as_deref(),
    )
    .unwrap_or_else(|e| panic!("import failed: {e:#}"));

    eprintln!("done in {:.3?}", start.elapsed());
    eprintln!("  spatial records:    {}", stats.records_written);
    eprintln!("  boundary functions: {}", stats.boundary_fns_written);
    eprintln!("  hyperedges:         {}", stats.hyperedges_written);
    eprintln!("  final revision:     {}", stats.final_revision);

    assert!(stats.records_written + stats.boundary_fns_written + stats.hyperedges_written > 0);
}
