//! Import a STEP file into an InfiniteDB directory from the command line.
//!
//! ```text
//! cargo run --bin import-step -- model.step ./my_database
//! cargo run --bin import-step -- model.step ./my_database --json dump.json
//! cargo run --features log --bin import-step -- model.step ./my_database --verbose
//! ```

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use infinite_db_step::{import_step_file_with_json, EncoderConfig, WriteOptions};

fn main() -> ExitCode {
    #[cfg(feature = "log")]
    let _ = env_logger::try_init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let positional: Vec<String> = std::env::args().skip(1).collect();
    if positional.is_empty() || positional.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        if positional.is_empty() {
            return Err(usage_error("missing STEP file path"));
        }
        return Ok(());
    }

    let mut json_path: Option<String> = None;
    let mut verbose = false;
    let mut paths: Vec<String> = Vec::new();

    let mut i = 0;
    while i < positional.len() {
        match positional[i].as_str() {
            "--json" => {
                i += 1;
                json_path = Some(
                    positional
                        .get(i)
                        .ok_or_else(|| usage_error("--json requires a file path"))?
                        .clone(),
                );
            }
            "--verbose" | "-v" => verbose = true,
            other if other.starts_with('-') => anyhow::bail!("unknown argument: {other}"),
            path => paths.push(path.to_string()),
        }
        i += 1;
    }

    let step_path = paths
        .first()
        .ok_or_else(|| usage_error("missing STEP file path"))?;
    let db_dir = paths
        .get(1)
        .ok_or_else(|| usage_error("missing output database directory"))?;
    let step_path = Path::new(step_path.as_str());
    let db_dir = Path::new(db_dir.as_str());
    let json = json_path.as_deref().map(Path::new);

    if !step_path.is_file() {
        anyhow::bail!("STEP file not found: {}", step_path.display());
    }

    eprintln!("importing {} -> {}", step_path.display(), db_dir.display());
    if let Some(j) = json {
        eprintln!("json dump: {}", j.display());
    }

    let start = Instant::now();
    let stats = import_step_file_with_json(
        step_path,
        db_dir,
        &EncoderConfig::default(),
        WriteOptions { verbose },
        json,
    )?;
    let elapsed = start.elapsed();

    eprintln!("done in {elapsed:.3?}");
    eprintln!("  spatial records:    {}", stats.records_written);
    eprintln!("  boundary functions: {}", stats.boundary_fns_written);
    eprintln!("  hyperedges:         {}", stats.hyperedges_written);
    eprintln!("  final revision:     {}", stats.final_revision);

    Ok(())
}

fn usage_error(msg: &str) -> anyhow::Error {
    print_usage();
    anyhow::anyhow!("{msg}")
}

fn print_usage() {
    eprintln!(
        "Usage: import-step <step-file> <db-dir> [--json dump.json] [--verbose]\n\
         \n\
         Import an ISO 10303-21 STEP file into an InfiniteDB directory.\n\
         \n\
         Options:\n\
           --json PATH   Write an encoded JSON dump alongside the database\n\
           --verbose     Log every entity and hyperedge (requires `log` feature)\n\
           -h, --help    Show this help"
    );
}
