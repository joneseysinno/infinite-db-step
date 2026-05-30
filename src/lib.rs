//! STEP (ISO 10303-21) to InfiniteDB importer library.
//!
//! Converts CAD STEP files into InfiniteDB spatial records using:
//!
//! 1. **Spatial nodes** — geometric entities (solid, face, edge, vertex) as typed
//!    addresses with Hilbert-friendly coordinates.
//! 2. **Boundary functions (SDF)** — signed distance representations for surfaces.
//! 3. **Hyperedges** — shell→face→edge→vertex topology and adjacency.
//!
//! ## Space layout
//!
//! | Space ID | Name           | Dims | Contents                         |
//! |----------|----------------|------|----------------------------------|
//! | 1        | solids         | 3    | Closed solid bodies (BREP roots) |
//! | 2        | shells         | 3    | Open/closed shells               |
//! | 3        | faces          | 3    | Faces with SDF parameters        |
//! | 4        | edges          | 3    | Curve edges between faces        |
//! | 5        | vertices       | 3    | Point vertices                   |
//! | 6        | boundary_fn    | 6    | SDF records                      |
//! | 10       | topology_edges | 2    | Containment/adjacency hyperedges |
//!
//! ## Features
//!
//! - **`log`** (optional): emit progress via `log::info!` and per-entity detail via
//!   `log::debug!` during [`write_model`]. Use [`WriteOptions::verbose`] for full
//!   entity/hyperedge traces. Without this feature, the library writes silently.

pub mod boundary;
pub mod encoder;
pub mod emitter;
pub mod geometry;
pub mod parser;
pub mod spaces;
pub mod store;

pub use boundary::{BoundaryFunction, BoundaryFunctionRecord};
pub use encoder::{
    encode_model, DbAddress, DbEndpoint, DbHyperedge, DbRecord, EncodedModel,
    EncoderConfig, SpaceRegistration,
};
pub use emitter::emit_json;
pub use geometry::{
    Axis2Placement, CurveKind, Dir3, Edge, Face, GeometryModel, Point3, Shell, Solid,
    SurfaceKind, Vertex,
};
pub use parser::{parse_raw, parse_step, RawEntity, StepFile};
pub use store::{write_model, WriteOptions, WriteStats};

use std::path::Path;

use anyhow::{Context, Result};

/// Parse STEP text in memory, encode, and write to an InfiniteDB directory.
pub fn import_step(
    step_text: &str,
    db_dir: &Path,
    config: &EncoderConfig,
    options: WriteOptions,
) -> Result<WriteStats> {
    let model = parse_step(step_text)?;
    let encoded = encode_model(&model, config)?;
    write_model(&encoded, db_dir, config, options)
}

/// Read a STEP file from disk, encode, and write to an InfiniteDB directory.
pub fn import_step_file(
    step_path: &Path,
    db_dir: &Path,
    config: &EncoderConfig,
    options: WriteOptions,
) -> Result<WriteStats> {
    import_step_file_with_json(step_path, db_dir, config, options, None)
}

/// Full pipeline: read STEP file, optional JSON dump, write InfiniteDB.
pub fn import_step_file_with_json(
    step_path: &Path,
    db_dir: &Path,
    config: &EncoderConfig,
    options: WriteOptions,
    json_path: Option<&Path>,
) -> Result<WriteStats> {
    let step_text = std::fs::read_to_string(step_path)
        .with_context(|| format!("failed to read STEP file: {}", step_path.display()))?;

    let model = parse_step(&step_text)?;
    let encoded = encode_model(&model, config)?;

    if let Some(path) = json_path {
        emit_json(&encoded, path)
            .with_context(|| format!("failed to write JSON dump: {}", path.display()))?;
    }

    write_model(&encoded, db_dir, config, options)
        .with_context(|| format!("failed to write InfiniteDb at {}", db_dir.display()))
}
