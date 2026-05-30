//! Emitter: write the encoded model to disk as JSON.
//!
//! The JSON format is structured to match InfiniteDB's bulk-insert API:
//!
//! ```json
//! {
//!   "spaces": [ { "id": 1, "name": "solids", "dims": 3, ... } ],
//!   "records": [
//!     {
//!       "address": { "space_id": 3, "coords": [12345678, 98765432, 11223344] },
//!       "entity_type": "face",
//!       "payload": { ... }
//!     }
//!   ],
//!   "boundary_fns": [ ... ],
//!   "hyperedges": [
//!     {
//!       "id": 100001,
//!       "kind": "face.has_boundary_fn",
//!       "endpoints": [
//!         { "role": "face",        "space_id": 3, "coords": [...] },
//!         { "role": "boundary_fn", "space_id": 6, "coords": [...] }
//!       ],
//!       "weight_milli": 42000,
//!       "metadata": { "surface_type": "PLANE", "is_exact_sdf": "true" }
//!     }
//!   ],
//!   "stats": { ... }
//! }
//! ```
//!
//! The `stats` block gives a human-readable summary useful for debugging
//! and validating the import before feeding it to the live database.
 
use std::collections::HashMap;
use std::path::Path;
use anyhow::Result;
use serde_json::json;
 
use crate::encoder::EncodedModel;
 
pub fn emit_json(model: &EncodedModel, path: &Path) -> Result<()> {
    // Compute summary statistics
    let mut entity_type_counts: HashMap<String, usize> = HashMap::new();
    for r in &model.records {
        *entity_type_counts.entry(r.entity_type.clone()).or_insert(0) += 1;
    }
    for r in &model.boundary_fns {
        *entity_type_counts.entry(r.entity_type.clone()).or_insert(0) += 1;
    }
 
    let mut hyperedge_kind_counts: HashMap<String, usize> = HashMap::new();
    for h in &model.hyperedges {
        *hyperedge_kind_counts.entry(h.kind.clone()).or_insert(0) += 1;
    }
 
    let stats = json!({
        "total_records": model.records.len() + model.boundary_fns.len(),
        "total_hyperedges": model.hyperedges.len(),
        "entity_types": entity_type_counts,
        "hyperedge_kinds": hyperedge_kind_counts,
        "spaces": model.spaces.len(),
    });
 
    let output = json!({
        "format_version": "step_infinitedb_v1",
        "spaces": model.spaces,
        "records": model.records,
        "boundary_fns": model.boundary_fns,
        "hyperedges": model.hyperedges,
        "stats": stats,
    });
 
    let json_str = serde_json::to_string_pretty(&output)?;
    std::fs::write(path, json_str)?;
    Ok(())
}
 