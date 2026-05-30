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
        bulk_insert_records(&mut db, &model.records, "records", log_progress, options)?;
    let boundary_fns_written = bulk_insert_records(
        &mut db,
        &model.boundary_fns,
        "boundary functions",
        log_progress,
        options,
    )?;
    let hyperedges_written =
        bulk_insert_hyperedges(&mut db, &model.hyperedges, log_progress, options)?;

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

/// Encode records and bulk-import them grouped by space (one bulk session per space).
fn bulk_insert_records(
    db: &mut InfiniteDb,
    records: &[DbRecord],
    label: &str,
    log_progress: bool,
    options: WriteOptions,
) -> Result<usize> {
    if records.is_empty() {
        return Ok(0);
    }

    let total = records.len();
    let mut by_space: BTreeMap<u64, Vec<(DimensionVector, Vec<u8>)>> = BTreeMap::new();
    let mut record_indices: BTreeMap<u64, Vec<usize>> = BTreeMap::new();

    for (i, record) in records.iter().enumerate() {
        let data = bincode::serde::encode_to_vec(&record.payload, standard())
            .with_context(|| format!("failed to bincode-encode {} record", record.entity_type))?;
        let point = DimensionVector::new(record.address.coords.clone());
        by_space
            .entry(record.address.space_id)
            .or_default()
            .push((point, data));
        record_indices
            .entry(record.address.space_id)
            .or_default()
            .push(i);
    }

    let mut written = 0usize;
    let mut global_done = 0usize;

    for (space_id, rows) in by_space {
        let space = SpaceId(space_id);
        let indices = record_indices.get(&space_id).cloned().unwrap_or_default();

        if log_progress && total >= 100 {
            let mut import = db
                .begin_record_import(space)
                .with_context(|| format!("failed to begin bulk record import for space {space_id}"))?;
            for (local_i, (point, data)) in rows.into_iter().enumerate() {
                import
                    .push(point, data)
                    .with_context(|| format!("failed to bulk-insert record in space {space_id}"))?;
                if let Some(&record_i) = indices.get(local_i) {
                    log_record_write(&records[record_i], options);
                }
                global_done += 1;
                log_insert_progress(label, global_done, total);
            }
            let result = import
                .finish()
                .with_context(|| format!("failed to finish bulk record import for space {space_id}"))?;
            written += result.count;
        } else {
            for (local_i, record) in indices.iter().enumerate() {
                log_record_write(&records[*record], options);
                let _ = local_i;
            }
            let result = db
                .insert_records_bulk(space, rows)
                .with_context(|| format!("failed to bulk-insert records in space {space_id}"))?;
            written += result.count;
            global_done += result.count;
        }
    }

    Ok(written)
}

/// Bulk-import hyperedges via the official hyperedge import session (with endpoint indexing).
fn bulk_insert_hyperedges(
    db: &mut InfiniteDb,
    hyperedges: &[DbHyperedge],
    log_progress: bool,
    options: WriteOptions,
) -> Result<usize> {
    if hyperedges.is_empty() {
        return Ok(0);
    }

    let total = hyperedges.len();
    let space = SpaceId(ids::TOPOLOGY);
    let mut import = db
        .begin_hyperedge_import(space)
        .context("failed to begin bulk hyperedge import")?;

    for (i, he) in hyperedges.iter().enumerate() {
        import
            .push(to_hyperedge(he))
            .with_context(|| format!("failed to bulk-insert hyperedge {} ({})", he.id, he.kind))?;
        log_hyperedge_write(he, options);
        if log_progress {
            log_insert_progress("hyperedges", i + 1, total);
        }
    }

    let result = import
        .finish()
        .context("failed to finish bulk hyperedge import")?;
    Ok(result.count)
}

fn log_insert_progress(label: &str, done: usize, total: usize) {
    #[cfg(feature = "log")]
    if done == 1 || done == total || done % 250 == 0 {
        log::info!("{label}: {done}/{total}");
    }
    #[cfg(not(feature = "log"))]
    let _ = (label, done, total);
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
