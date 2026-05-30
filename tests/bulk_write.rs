//! Bulk write integration tests.

use std::collections::HashMap;
use std::time::Instant;

use infinite_db::infinitedb_core::address::SpaceId;
use infinite_db::InfiniteDb;
use infinite_db_step::{
    write_model, DbAddress, DbEndpoint, DbHyperedge, DbRecord, EncodedModel, EncoderConfig,
    SpaceRegistration, WriteOptions,
};
use infinite_db_step::spaces::{dims, ids};
use tempfile::TempDir;

fn minimal_spaces() -> Vec<SpaceRegistration> {
    vec![
        SpaceRegistration {
            id: ids::SOLIDS,
            name: "solids".into(),
            dims: dims::SPATIAL_3D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::SHELLS,
            name: "shells".into(),
            dims: dims::SPATIAL_3D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::FACES,
            name: "faces".into(),
            dims: dims::SPATIAL_3D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::EDGES,
            name: "edges".into(),
            dims: dims::SPATIAL_3D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::VERTICES,
            name: "vertices".into(),
            dims: dims::SPATIAL_3D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::BOUNDARY_FN,
            name: "boundary_fn".into(),
            dims: dims::BOUNDARY_6D,
            description: String::new(),
        },
        SpaceRegistration {
            id: ids::TOPOLOGY,
            name: "topology".into(),
            dims: dims::TOPOLOGY_2D,
            description: String::new(),
        },
    ]
}

fn synthetic_model(record_count: usize, hyperedge_count: usize) -> EncodedModel {
    let mut records = Vec::with_capacity(record_count);
    for i in 0..record_count {
        let space_id = match i % 5 {
            0 => ids::VERTICES,
            1 => ids::EDGES,
            2 => ids::FACES,
            3 => ids::SHELLS,
            _ => ids::SOLIDS,
        };
        records.push(DbRecord {
            address: DbAddress {
                space_id,
                coords: vec![i as u32, (i >> 16) as u32, 0],
            },
            payload: serde_json::json!({ "id": i, "name": format!("entity_{i}") }),
            entity_type: "test".to_string(),
        });
    }

    let boundary_fns: Vec<DbRecord> = (0..5)
        .map(|i| DbRecord {
            address: DbAddress {
                space_id: ids::BOUNDARY_FN,
                coords: vec![i, 0, 0, 0, 0, 0],
            },
            payload: serde_json::json!({ "entity_id": i }),
            entity_type: "boundary_fn::plane".to_string(),
        })
        .collect();

    let hyperedges: Vec<DbHyperedge> = (0..hyperedge_count)
        .map(|i| {
            let id = 100_001 + i as u64;
            DbHyperedge {
                id,
                kind: "face.is_adjacent_to_face".to_string(),
                endpoints: vec![
                    DbEndpoint {
                        role: "face_a".to_string(),
                        space_id: ids::FACES,
                        coords: vec![i as u32, 0, 0],
                    },
                    DbEndpoint {
                        role: "face_b".to_string(),
                        space_id: ids::FACES,
                        coords: vec![i as u32, 1, 0],
                    },
                ],
                weight_milli: Some(1000),
                metadata: HashMap::new(),
            }
        })
        .collect();

    EncodedModel {
        records,
        boundary_fns,
        hyperedges,
        spaces: minimal_spaces(),
    }
}

#[test]
fn bulk_write_records_and_hyperedges_roundtrip() {
    let dir = TempDir::new().unwrap();
    let model = synthetic_model(15, 50);
    let config = EncoderConfig::default();

    let stats = write_model(&model, dir.path(), &config, WriteOptions::default()).unwrap();
    assert_eq!(stats.records_written, 15);
    assert_eq!(stats.boundary_fns_written, 5);
    assert_eq!(stats.hyperedges_written, 50);

    let mut db = InfiniteDb::open(dir.path()).unwrap();
    assert_eq!(db.query(SpaceId(ids::VERTICES), None).unwrap().len(), 3);
    assert_eq!(db.query(SpaceId(ids::BOUNDARY_FN), None).unwrap().len(), 5);
    assert_eq!(
        db.query_hyperedges(SpaceId(ids::TOPOLOGY), None)
            .unwrap()
            .len(),
        50
    );
}

#[test]
#[ignore = "bulk perf: run with cargo test --test bulk_write -- --ignored --nocapture"]
fn write_one_million_plus_entities_reports_throughput() {
    const RECORDS: usize = 100_000;
    const HYPEREDGES: usize = 1_000_001;

    let dir = TempDir::new().unwrap();
    let model = synthetic_model(RECORDS, HYPEREDGES);
    let config = EncoderConfig::default();

    let start = Instant::now();
    let stats = write_model(&model, dir.path(), &config, WriteOptions::default()).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(stats.records_written, RECORDS);
    assert_eq!(stats.hyperedges_written, HYPEREDGES);

    let total = stats.records_written + stats.boundary_fns_written + stats.hyperedges_written;
    let secs = elapsed.as_secs_f64().max(f64::EPSILON);

    eprintln!("=== bulk write throughput ===");
    eprintln!("spatial records:     {}", stats.records_written);
    eprintln!("boundary functions:  {}", stats.boundary_fns_written);
    eprintln!("hyperedges:          {}", stats.hyperedges_written);
    eprintln!("total entities:      {total}");
    eprintln!("wall time:           {elapsed:.3?}");
    eprintln!(
        "throughput:          {:.0} entities/sec",
        total as f64 / secs
    );
    eprintln!(
        "hyperedge rate:      {:.0} hyperedges/sec",
        stats.hyperedges_written as f64 / secs
    );
    eprintln!("final revision:      {}", stats.final_revision);

    let mut db = InfiniteDb::open(dir.path()).unwrap();
    assert_eq!(
        db.query_hyperedges(SpaceId(ids::TOPOLOGY), None)
            .unwrap()
            .len(),
        HYPEREDGES
    );
}
