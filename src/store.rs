//! Persist an encoded STEP model into an on-disk InfiniteDb instance.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use bincode::config::standard;
use infinite_db::infinitedb_core::address::{DimensionVector, RevisionId, SpaceId};
use infinite_db::infinitedb_core::hyperedge::{
    EndpointRef, EndpointRole, Hyperedge, HyperedgeId, HyperedgeKind,
};
use infinite_db::infinitedb_core::space::SpaceConfig;
use infinite_db::InfiniteDb;

use crate::encoder::{DbHyperedge, DbRecord, EncodedModel, EncoderConfig};
use crate::spaces::ids;

macro_rules! write_log {
    ($level:ident, $($t:tt)*) => {
        #[cfg(feature = "log")]
        log::$level!($($t)*);
    };
}

/// Controls optional logging while writing to InfiniteDB.
///
/// Only has an effect when this crate is built with the `log` feature.
/// Progress milestones use `info!`; per-record detail uses `debug!` when `verbose` is true.
#[derive(Debug, Clone, Copy)]
pub struct WriteOptions {
    /// Emit `debug!` for every entity and hyperedge (requires the `log` feature).
    pub verbose: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self { verbose: false }
    }
}

/// Summary returned after a successful write.
#[derive(Debug)]
pub struct WriteStats {
    pub records_written: usize,
    pub boundary_fns_written: usize,
    pub hyperedges_written: usize,
    pub final_revision: u64,
}

/// Open (or create) a database at `db_dir`, insert all records and hyperedges, and flush.
pub fn write_model(
    model: &EncodedModel,
    db_dir: &Path,
    config: &EncoderConfig,
    options: WriteOptions,
) -> Result<WriteStats> {
    let total_records = model.records.len() + model.boundary_fns.len();
    let total_hyperedges = model.hyperedges.len();
    let total_ops = total_records + total_hyperedges;
    let log_progress = total_ops >= 100;

    if log_progress {
        write_log!(
            info,
            "writing {total_records} spatial records + {total_hyperedges} hyperedges to {}",
            db_dir.display()
        );
    } else {
        write_log!(
            debug,
            "writing {total_records} spatial records + {total_hyperedges} hyperedges to {}",
            db_dir.display()
        );
    }

    let mut db = InfiniteDb::open(db_dir)
        .with_context(|| format!("failed to open InfiniteDb at {}", db_dir.display()))?;

    register_spaces(&mut db, model, config)?;

    let records_written =
        insert_records(&mut db, &model.records, "records", log_progress, options)?;
    let boundary_fns_written = insert_records(
        &mut db,
        &model.boundary_fns,
        "boundary functions",
        log_progress,
        options,
    )?;
    let hyperedges_written =
        insert_hyperedges(&mut db, &model.hyperedges, log_progress, options)?;

    if log_progress {
        write_log!(info, "sealing database blocks");
    }

    flush_all_spaces(&mut db)?;

    Ok(WriteStats {
        records_written,
        boundary_fns_written,
        hyperedges_written,
        final_revision: db.revision(),
    })
}

fn register_spaces(
    db: &mut InfiniteDb,
    model: &EncodedModel,
    config: &EncoderConfig,
) -> Result<()> {
    for space in &model.spaces {
        let cfg = SpaceConfig::new(SpaceId(space.id), &space.name, space.dims)
            .with_bits_per_dim(config.bits_per_dim);
        if let Err(err) = db.register_space(cfg) {
            if !is_duplicate_space_error(&err) {
                return Err(anyhow::anyhow!("failed to register space {}: {err}", space.id));
            }
        }
    }
    Ok(())
}

fn is_duplicate_space_error(err: &str) -> bool {
    err.contains("Duplicate")
}

fn insert_records(
    db: &mut InfiniteDb,
    records: &[DbRecord],
    label: &str,
    log_progress: bool,
    options: WriteOptions,
) -> Result<usize> {
    let total = records.len();
    for (i, record) in records.iter().enumerate() {
        let data = bincode::serde::encode_to_vec(&record.payload, standard())
            .with_context(|| format!("failed to bincode-encode {} record", record.entity_type))?;
        db.insert(
            SpaceId(record.address.space_id),
            DimensionVector::new(record.address.coords.clone()),
            data,
        )
        .with_context(|| {
            format!(
                "failed to insert {} record in space {}",
                record.entity_type, record.address.space_id
            )
        })?;
        log_record_write(record, options);
        if log_progress {
            log_insert_progress(label, i + 1, total);
        }
    }
    Ok(total)
}

/// Insert hyperedges as plain records (not via `insert_hyperedge`).
///
/// Skipping `insert_hyperedge` avoids the per-endpoint reverse index, which
/// triples disk syncs. Full hyperedge scans still work; endpoint lookup queries
/// are not indexed until a future re-index pass.
fn insert_hyperedges(
    db: &mut InfiniteDb,
    hyperedges: &[DbHyperedge],
    log_progress: bool,
    options: WriteOptions,
) -> Result<usize> {
    let total = hyperedges.len();
    let space = SpaceId(ids::TOPOLOGY);
    for (i, he) in hyperedges.iter().enumerate() {
        let edge = to_hyperedge(he);
        let point = hyperedge_storage_point(he.id);
        let data = bincode::encode_to_vec(&edge, standard())
            .with_context(|| format!("failed to bincode-encode hyperedge {}", he.id))?;
        db.insert(space, point, data)
            .with_context(|| format!("failed to insert hyperedge {} ({})", he.id, he.kind))?;
        log_hyperedge_write(he, options);
        if log_progress {
            log_insert_progress("hyperedges", i + 1, total);
        }
    }
    Ok(total)
}

fn log_insert_progress(label: &str, done: usize, total: usize) {
    #[cfg(feature = "log")]
    if done == 1 || done == total || done % 250 == 0 {
        log::info!("{label}: {done}/{total}");
    }
    #[cfg(not(feature = "log"))]
    let _ = (label, done, total);
}

fn hyperedge_storage_point(id: u64) -> DimensionVector {
    DimensionVector::new(vec![(id >> 32) as u32, (id & 0xFFFF_FFFF) as u32])
}

fn to_hyperedge(he: &DbHyperedge) -> Hyperedge {
    Hyperedge {
        id: HyperedgeId(he.id),
        kind: HyperedgeKind::new(&he.kind),
        endpoints: he
            .endpoints
            .iter()
            .map(|ep| EndpointRef {
                role: EndpointRole::new(&ep.role),
                space: SpaceId(ep.space_id),
                node: DimensionVector::new(ep.coords.clone()),
            })
            .collect(),
        weight_milli: he.weight_milli,
        metadata: he
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        valid_from: RevisionId::ZERO,
        valid_to: None,
    }
}

#[cfg(feature = "log")]
fn payload_field(payload: &serde_json::Value, key: &str) -> String {
    match payload.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(v) => v.to_string(),
        None => "?".to_string(),
    }
}

#[cfg(feature = "log")]
fn payload_point3(payload: &serde_json::Value, key: &str) -> (f64, f64, f64) {
    payload
        .get(key)
        .and_then(|p| {
            Some((
                p.get("x")?.as_f64()?,
                p.get("y")?.as_f64()?,
                p.get("z")?.as_f64()?,
            ))
        })
        .unwrap_or((0.0, 0.0, 0.0))
}

#[cfg(feature = "log")]
fn log_record_write(record: &DbRecord, options: WriteOptions) {
    match record.entity_type.as_str() {
            "face" => {
                let (cx, cy, cz) = payload_point3(&record.payload, "centroid");
                let (nx, ny, nz) = payload_point3(&record.payload, "normal");
                write_log!(
                    debug,
                    "face id={} name={:?} surface={} area={:.1} edges={} centroid=({:.1}, {:.1}, {:.1}) normal=({:.2}, {:.2}, {:.2})",
                    payload_field(&record.payload, "id"),
                    payload_field(&record.payload, "name"),
                    payload_field(&record.payload, "surface_type"),
                    payload_field(&record.payload, "area_estimate")
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    payload_field(&record.payload, "edge_count"),
                    cx,
                    cy,
                    cz,
                    nx,
                    ny,
                    nz,
                );
            }
            et if et.starts_with("boundary_fn::") => {
                let (cx, cy, cz) = payload_point3(&record.payload, "centroid");
                let fn_type = et.strip_prefix("boundary_fn::").unwrap_or(et);
                write_log!(
                    debug,
                    "boundary_fn face_id={} fn={} surface={} exact={} area={:.1} centroid=({:.1}, {:.1}, {:.1}) sdf@centroid={}",
                    payload_field(&record.payload, "entity_id"),
                    fn_type,
                    payload_field(&record.payload, "surface_type_name"),
                    payload_field(&record.payload, "is_exact"),
                    payload_field(&record.payload, "area_estimate")
                        .parse::<f64>()
                        .unwrap_or(0.0),
                    cx,
                    cy,
                    cz,
                    payload_field(&record.payload, "sdf_at_centroid"),
                );
            }
            other if options.verbose => {
                let (cx, cy, cz) = payload_point3(&record.payload, "centroid");
                let id = payload_field(&record.payload, "id");
                let name = payload_field(&record.payload, "name");
                if record.payload.get("centroid").is_some() {
                    write_log!(
                        debug,
                        "{other} id={id} name={name:?} centroid=({cx:.1}, {cy:.1}, {cz:.1}) space={}",
                        record.address.space_id
                    );
                } else if record.payload.get("x").is_some() {
                    write_log!(
                        debug,
                        "{other} id={id} name={name:?} pos=({:.1}, {:.1}, {:.1}) space={}",
                        record
                            .payload
                            .get("x")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                        record
                            .payload
                            .get("y")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                        record
                            .payload
                            .get("z")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0),
                        record.address.space_id,
                    );
                } else {
                    write_log!(
                        debug,
                        "{other} id={id} name={name:?} space={}",
                        record.address.space_id
                    );
                }
            }
            _ => {}
        }
}

#[cfg(not(feature = "log"))]
fn log_record_write(_record: &DbRecord, _options: WriteOptions) {}

#[cfg(feature = "log")]
fn log_hyperedge_write(he: &DbHyperedge, options: WriteOptions) {
    let is_face_related = he.kind.starts_with("face.")
        || he.endpoints.iter().any(|ep| ep.role.contains("face"));
    if !options.verbose && !is_face_related {
        return;
    }

    let endpoints: Vec<String> = he
        .endpoints
        .iter()
        .map(|ep| format!("{}@space{}", ep.role, ep.space_id))
        .collect();
    let meta = if he.metadata.is_empty() {
        String::new()
    } else {
        format!(
            " meta={}",
            he.metadata
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let weight = he
        .weight_milli
        .map(|w| format!(" weight_milli={w}"))
        .unwrap_or_default();
    write_log!(
        debug,
        "topology {} #{}{} endpoints=[{}]{meta}",
        he.kind,
        he.id,
        weight,
        endpoints.join(", "),
    );
}

#[cfg(not(feature = "log"))]
fn log_hyperedge_write(_he: &DbHyperedge, _options: WriteOptions) {}

fn flush_all_spaces(db: &mut InfiniteDb) -> Result<()> {
    for space_id in [
        ids::SOLIDS,
        ids::SHELLS,
        ids::FACES,
        ids::EDGES,
        ids::VERTICES,
        ids::BOUNDARY_FN,
        ids::TOPOLOGY,
    ] {
        db.flush(SpaceId(space_id))
            .with_context(|| format!("failed to flush space {space_id}"))?;
    }
    Ok(())
}
