//! Encoder: GeometryModel → InfiniteDB records + hyperedges.
//!
//! This is the heart of the importer. It translates each geometric entity
//! into the spatial address system used by InfiniteDB.
//!
//! ## Coordinate Encoding
//!
//! World coordinates (floats) are normalized to u32 via:
//!   coord_u32 = (val + coord_max) / (2 * coord_max) * u32::MAX
//!
//! Then the 3 coordinates [x, y, z] are fed to the Hilbert encoder to produce
//! a single u128 that preserves spatial locality. In InfiniteDB, the
//! DimensionVector stores the [x, y, z] u32 values directly; the Hilbert
//! encoding is done internally when building the B-tree index.
//!
//! ## Record Format
//!
//! Each InfiniteDB record has:
//!   - address: SpaceId + DimensionVector (the spatial key)
//!   - data:    bincode-serialized payload (the domain struct)
//!
//! Our payload types are serialized to JSON here for human readability in
//! the output dump. Production use would switch to bincode.
//!
//! ## Hyperedge Topology
//!
//! Containment (solid → shell, shell → face, face → edge, edge → vertex)
//! and adjacency (face A shares edge with face B) are stored as typed
//! Hyperedge records using InfiniteDB's existing Hyperedge model.
//!
//! ### Hyperedge kinds:
//! - "solid.contains_shell"       solid → shell
//! - "shell.contains_face"        shell → face
//! - "face.has_boundary_edge"     face  → edge
//! - "edge.has_start_vertex"      edge  → vertex (start)
//! - "edge.has_end_vertex"        edge  → vertex (end)
//! - "face.is_adjacent_to_face"   face  → face (sharing an edge)
//! - "face.has_boundary_fn"       face  → boundary_fn record
//! - "solid.has_bbox_signal"      solid → bbox signal
 
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use anyhow::Result;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::geometry::*;
use crate::boundary::{boundary_fn_from_surface};
use crate::spaces::{ids, dims, encode_point_3d, encode_point_6d};
 
/// Configuration for the encoder.
pub struct EncoderConfig {
    pub bits_per_dim: u32,
    pub coord_max: f64,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            bits_per_dim: 8,
            coord_max: 1000.0,
        }
    }
}
 
/// An InfiniteDB address (space + 3D coordinate vector).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbAddress {
    pub space_id: u64,
    pub coords: Vec<u32>,
}
 
/// A spatial record ready for InfiniteDB insertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRecord {
    pub address: DbAddress,
    /// JSON-serialized domain payload (production: bincode).
    pub payload: serde_json::Value,
    /// Human-readable entity type label.
    pub entity_type: String,
}
 
/// A relationship record (maps to InfiniteDB Hyperedge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbHyperedge {
    pub id: u64,
    pub kind: String,
    /// (space_id, coords, role) for each endpoint.
    pub endpoints: Vec<DbEndpoint>,
    /// Optional weight in milli-units.
    pub weight_milli: Option<i64>,
    pub metadata: HashMap<String, String>,
}
 
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbEndpoint {
    pub role: String,
    pub space_id: u64,
    pub coords: Vec<u32>,
}
 
/// The complete output of the encoder.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct EncodedModel {
    /// All spatial records (solids, shells, faces, edges, vertices).
    pub records: Vec<DbRecord>,
    /// All relationship hyperedges.
    pub hyperedges: Vec<DbHyperedge>,
    /// All boundary function records (6D space).
    pub boundary_fns: Vec<DbRecord>,
    /// Space registry entries.
    pub spaces: Vec<SpaceRegistration>,
}
 
/// Space registration info emitted alongside the records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceRegistration {
    pub id: u64,
    pub name: String,
    pub dims: usize,
    pub description: String,
}
 
pub fn encode_model(model: &GeometryModel, config: &EncoderConfig) -> Result<EncodedModel> {
    let mut out = EncodedModel::default();

    out.spaces = vec![
        SpaceRegistration { id: ids::SOLIDS,      name: "solids".into(),       dims: dims::SPATIAL_3D,  description: "Closed solid bodies".into() },
        SpaceRegistration { id: ids::SHELLS,      name: "shells".into(),       dims: dims::SPATIAL_3D,  description: "Open/closed shells".into() },
        SpaceRegistration { id: ids::FACES,       name: "faces".into(),        dims: dims::SPATIAL_3D,  description: "Individual faces with surface type".into() },
        SpaceRegistration { id: ids::EDGES,       name: "edges".into(),        dims: dims::SPATIAL_3D,  description: "Curve edges between vertices".into() },
        SpaceRegistration { id: ids::VERTICES,    name: "vertices".into(),     dims: dims::SPATIAL_3D,  description: "Point vertices".into() },
        SpaceRegistration { id: ids::BOUNDARY_FN, name: "boundary_fn".into(),  dims: dims::BOUNDARY_6D, description: "SDF boundary functions (centroid + normal)".into() },
        SpaceRegistration { id: ids::TOPOLOGY,    name: "topology".into(),     dims: dims::TOPOLOGY_2D, description: "Topological relationship hyperedges".into() },
    ];

    let edge_to_faces = build_edge_to_faces(model);

    let edge_he_base = 100_001u64;
    let edge_he_count = model.edges.len() * 2;
    let face_he_base = edge_he_base + edge_he_count as u64;
    let face_he_counts: Vec<usize> = model
        .faces
        .iter()
        .map(|f| 1 + f.edge_loop.len())
        .collect();
    let face_he_count: usize = face_he_counts.iter().sum();
    let adjacency_he_base = face_he_base + face_he_count as u64;
    let adjacency_he_count = count_adjacency_hyperedges(&edge_to_faces);
    let shell_he_base = adjacency_he_base + adjacency_he_count as u64;
    let shell_he_counts: Vec<usize> = model.shells.iter().map(|s| s.face_ids.len()).collect();
    let shell_he_count: usize = shell_he_counts.iter().sum();
    let solid_he_base = shell_he_base + shell_he_count as u64;

    let face_he_offsets: Vec<usize> = {
        let mut off = 0usize;
        face_he_counts
            .iter()
            .map(|&c| {
                let start = off;
                off += c;
                start
            })
            .collect()
    };

    let shell_he_offsets: Vec<usize> = {
        let mut off = 0usize;
        shell_he_counts
            .iter()
            .map(|&c| {
                let start = off;
                off += c;
                start
            })
            .collect()
    };

    // --- Vertices ---
    encode_vertices(model, config, &mut out);

    // --- Edges ---
    encode_edges(model, config, edge_he_base, &mut out);

    // --- Faces + boundary functions ---
    encode_faces(model, config, face_he_base, &face_he_offsets, &mut out)?;

    // --- Face adjacency ---
    encode_adjacency(model, config, &edge_to_faces, adjacency_he_base, &mut out);

    // --- Shells ---
    encode_shells(model, config, shell_he_base, &shell_he_offsets, &mut out);

    // --- Solids ---
    encode_solids(model, config, solid_he_base, &mut out);

    Ok(out)
}

fn build_edge_to_faces(model: &GeometryModel) -> HashMap<u64, Vec<u64>> {
    let mut edge_to_faces: HashMap<u64, Vec<u64>> = HashMap::new();
    for face in &model.faces {
        for eid in &face.edge_loop {
            edge_to_faces.entry(*eid).or_default().push(face.id);
        }
    }
    edge_to_faces
}

fn count_adjacency_hyperedges(edge_to_faces: &HashMap<u64, Vec<u64>>) -> usize {
    edge_to_faces
        .values()
        .filter(|ids| ids.len() >= 2)
        .map(|ids| ids.len() * (ids.len() - 1) / 2)
        .sum()
}

fn encode_vertices(model: &GeometryModel, config: &EncoderConfig, out: &mut EncodedModel) {
    let map_vertex = |v: &Vertex| {
        let coords = encode_point_3d(
            v.position.x,
            v.position.y,
            v.position.z,
            config.coord_max,
            config.bits_per_dim,
        );
        DbRecord {
            address: DbAddress {
                space_id: ids::VERTICES,
                coords: coords.to_vec(),
            },
            payload: serde_json::json!({
                "id": v.id,
                "name": v.name,
                "x": v.position.x,
                "y": v.position.y,
                "z": v.position.z,
            }),
            entity_type: "vertex".to_string(),
        }
    };

    #[cfg(feature = "parallel")]
    {
        out.records.extend(
            model
                .vertices
                .par_iter()
                .map(map_vertex)
                .collect::<Vec<_>>(),
        );
    }
    #[cfg(not(feature = "parallel"))]
    {
        out.records.extend(model.vertices.iter().map(map_vertex));
    }
}

fn encode_edges(model: &GeometryModel, config: &EncoderConfig, he_base: u64, out: &mut EncodedModel) {
    let encode_one = |i: usize, e: &Edge| {
        let coords = encode_point_3d(
            e.midpoint.x,
            e.midpoint.y,
            e.midpoint.z,
            config.coord_max,
            config.bits_per_dim,
        );
        let record = DbRecord {
            address: DbAddress {
                space_id: ids::EDGES,
                coords: coords.to_vec(),
            },
            payload: serde_json::json!({
                "id": e.id,
                "name": e.name,
                "start_vertex_id": e.start_vertex_id,
                "end_vertex_id": e.end_vertex_id,
                "curve_type": curve_type_name(&e.curve),
                "midpoint": { "x": e.midpoint.x, "y": e.midpoint.y, "z": e.midpoint.z },
                "length_estimate": e.length_estimate,
            }),
            entity_type: "edge".to_string(),
        };

        let v_start = vertex_coords_for(e.start_vertex_id, model, config);
        let v_end = vertex_coords_for(e.end_vertex_id, model, config);
        let he_id = he_base + (i * 2) as u64;
        let hyperedges = vec![
            DbHyperedge {
                id: he_id,
                kind: "edge.has_start_vertex".to_string(),
                endpoints: vec![
                    DbEndpoint {
                        role: "edge".to_string(),
                        space_id: ids::EDGES,
                        coords: coords.to_vec(),
                    },
                    DbEndpoint {
                        role: "vertex".to_string(),
                        space_id: ids::VERTICES,
                        coords: v_start,
                    },
                ],
                weight_milli: Some((e.length_estimate * 1000.0) as i64),
                metadata: HashMap::new(),
            },
            DbHyperedge {
                id: he_id + 1,
                kind: "edge.has_end_vertex".to_string(),
                endpoints: vec![
                    DbEndpoint {
                        role: "edge".to_string(),
                        space_id: ids::EDGES,
                        coords: coords.to_vec(),
                    },
                    DbEndpoint {
                        role: "vertex".to_string(),
                        space_id: ids::VERTICES,
                        coords: v_end,
                    },
                ],
                weight_milli: None,
                metadata: HashMap::new(),
            },
        ];
        (record, hyperedges)
    };

    #[cfg(feature = "parallel")]
    let edge_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .edges
        .par_iter()
        .enumerate()
        .map(|(i, e)| encode_one(i, e))
        .collect();
    #[cfg(not(feature = "parallel"))]
    let edge_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| encode_one(i, e))
        .collect();

    for (record, hyperedges) in edge_outputs {
        out.records.push(record);
        out.hyperedges.extend(hyperedges);
    }
}

fn encode_faces(
    model: &GeometryModel,
    config: &EncoderConfig,
    he_base: u64,
    he_offsets: &[usize],
    out: &mut EncodedModel,
) -> Result<()> {
    let encode_one = |i: usize, face: &Face| -> Result<(DbRecord, DbRecord, Vec<DbHyperedge>)> {
        let coords = encode_point_3d(
            face.centroid.x,
            face.centroid.y,
            face.centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );

        let record = DbRecord {
            address: DbAddress {
                space_id: ids::FACES,
                coords: coords.to_vec(),
            },
            payload: serde_json::json!({
                "id": face.id,
                "name": face.name,
                "surface_type": surface_type_name(&face.surface),
                "same_sense": face.same_sense,
                "centroid": { "x": face.centroid.x, "y": face.centroid.y, "z": face.centroid.z },
                "normal": { "x": face.normal_at_centroid.x, "y": face.normal_at_centroid.y, "z": face.normal_at_centroid.z },
                "area_estimate": face.area_estimate,
                "edge_count": face.edge_loop.len(),
            }),
            entity_type: "face".to_string(),
        };

        let bfn = boundary_fn_from_surface(
            face.id,
            &face.surface,
            face.centroid,
            face.normal_at_centroid,
            face.area_estimate,
        );

        let bfn_coords = encode_point_6d(
            face.centroid.x,
            face.centroid.y,
            face.centroid.z,
            face.normal_at_centroid.x,
            face.normal_at_centroid.y,
            face.normal_at_centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );

        let (sdf_at_centroid, sdf_is_approximate) = bfn.function.evaluate(face.centroid);
        let contains_centroid = bfn.function.contains(face.centroid);
        let mut bfn_json = serde_json::to_value(&bfn)?;
        if let serde_json::Value::Object(ref mut map) = bfn_json {
            map.insert("sdf_at_centroid".into(), serde_json::json!(sdf_at_centroid));
            map.insert(
                "sdf_is_approximate".into(),
                serde_json::json!(sdf_is_approximate),
            );
            map.insert("contains_centroid".into(), serde_json::json!(contains_centroid));
        }

        let bfn_record = DbRecord {
            address: DbAddress {
                space_id: ids::BOUNDARY_FN,
                coords: bfn_coords.to_vec(),
            },
            payload: bfn_json,
            entity_type: format!("boundary_fn::{}", bfn.function.type_name()),
        };

        let mut he_id = he_base + he_offsets[i] as u64;
        let mut hyperedges = Vec::new();

        hyperedges.push(DbHyperedge {
            id: he_id,
            kind: "face.has_boundary_fn".to_string(),
            endpoints: vec![
                DbEndpoint {
                    role: "face".to_string(),
                    space_id: ids::FACES,
                    coords: coords.to_vec(),
                },
                DbEndpoint {
                    role: "boundary_fn".to_string(),
                    space_id: ids::BOUNDARY_FN,
                    coords: bfn_coords.to_vec(),
                },
            ],
            weight_milli: Some((face.area_estimate * 1000.0) as i64),
            metadata: {
                let mut m = HashMap::new();
                m.insert(
                    "surface_type".to_string(),
                    surface_type_name(&face.surface).to_string(),
                );
                m.insert("is_exact_sdf".to_string(), bfn.is_exact.to_string());
                m
            },
        });
        he_id += 1;

        for eid in &face.edge_loop {
            if let Some(e) = model.edge(*eid) {
                let e_coords = encode_point_3d(
                    e.midpoint.x,
                    e.midpoint.y,
                    e.midpoint.z,
                    config.coord_max,
                    config.bits_per_dim,
                );
                hyperedges.push(DbHyperedge {
                    id: he_id,
                    kind: "face.has_boundary_edge".to_string(),
                    endpoints: vec![
                        DbEndpoint {
                            role: "face".to_string(),
                            space_id: ids::FACES,
                            coords: coords.to_vec(),
                        },
                        DbEndpoint {
                            role: "edge".to_string(),
                            space_id: ids::EDGES,
                            coords: e_coords.to_vec(),
                        },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
                he_id += 1;
            }
        }

        Ok((record, bfn_record, hyperedges))
    };

    #[cfg(feature = "parallel")]
    let face_outputs: Result<Vec<_>> = model
        .faces
        .par_iter()
        .enumerate()
        .map(|(i, face)| encode_one(i, face))
        .collect();
    #[cfg(not(feature = "parallel"))]
    let face_outputs: Result<Vec<_>> = model
        .faces
        .iter()
        .enumerate()
        .map(|(i, face)| encode_one(i, face))
        .collect();

    for (record, bfn_record, hyperedges) in face_outputs? {
        out.records.push(record);
        out.boundary_fns.push(bfn_record);
        out.hyperedges.extend(hyperedges);
    }

    Ok(())
}

fn encode_adjacency(
    model: &GeometryModel,
    config: &EncoderConfig,
    edge_to_faces: &HashMap<u64, Vec<u64>>,
    he_base: u64,
    out: &mut EncodedModel,
) {
    let face_coord_map: HashMap<u64, Vec<u32>> = model
        .faces
        .iter()
        .map(|f| {
            let c = encode_point_3d(
                f.centroid.x,
                f.centroid.y,
                f.centroid.z,
                config.coord_max,
                config.bits_per_dim,
            );
            (f.id, c.to_vec())
        })
        .collect();

    let mut adjacency_items: Vec<(u64, u64, u64)> = Vec::new();
    for (edge_id, face_ids) in edge_to_faces {
        if face_ids.len() >= 2 {
            for i in 0..face_ids.len() {
                for j in i + 1..face_ids.len() {
                    adjacency_items.push((*edge_id, face_ids[i], face_ids[j]));
                }
            }
        }
    }

    let encode_one = |i: usize, (edge_id, fa_id, fb_id): (u64, u64, u64)| {
        if let (Some(ca), Some(cb)) = (face_coord_map.get(&fa_id), face_coord_map.get(&fb_id)) {
            let mut meta = HashMap::new();
            meta.insert("shared_edge_id".to_string(), edge_id.to_string());
            Some(DbHyperedge {
                id: he_base + i as u64,
                kind: "face.is_adjacent_to_face".to_string(),
                endpoints: vec![
                    DbEndpoint {
                        role: "face_a".to_string(),
                        space_id: ids::FACES,
                        coords: ca.clone(),
                    },
                    DbEndpoint {
                        role: "face_b".to_string(),
                        space_id: ids::FACES,
                        coords: cb.clone(),
                    },
                ],
                weight_milli: None,
                metadata: meta,
            })
        } else {
            None
        }
    };

    #[cfg(feature = "parallel")]
    let hyperedges: Vec<DbHyperedge> = adjacency_items
        .par_iter()
        .enumerate()
        .filter_map(|(i, item)| encode_one(i, *item))
        .collect();
    #[cfg(not(feature = "parallel"))]
    let hyperedges: Vec<DbHyperedge> = adjacency_items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| encode_one(i, *item))
        .collect();

    out.hyperedges.extend(hyperedges);
}

fn encode_shells(
    model: &GeometryModel,
    config: &EncoderConfig,
    he_base: u64,
    he_offsets: &[usize],
    out: &mut EncodedModel,
) {
    let encode_one = |i: usize, shell: &Shell| {
        let coords = encode_point_3d(
            shell.centroid.x,
            shell.centroid.y,
            shell.centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );

        let record = DbRecord {
            address: DbAddress {
                space_id: ids::SHELLS,
                coords: coords.to_vec(),
            },
            payload: serde_json::json!({
                "id": shell.id,
                "name": shell.name,
                "is_closed": shell.is_closed,
                "face_count": shell.face_ids.len(),
                "centroid": { "x": shell.centroid.x, "y": shell.centroid.y, "z": shell.centroid.z },
            }),
            entity_type: "shell".to_string(),
        };

        let mut he_id = he_base + he_offsets[i] as u64;
        let mut hyperedges = Vec::new();
        for fid in &shell.face_ids {
            if let Some(face) = model.face(*fid) {
                let f_coords = encode_point_3d(
                    face.centroid.x,
                    face.centroid.y,
                    face.centroid.z,
                    config.coord_max,
                    config.bits_per_dim,
                );
                hyperedges.push(DbHyperedge {
                    id: he_id,
                    kind: "shell.contains_face".to_string(),
                    endpoints: vec![
                        DbEndpoint {
                            role: "shell".to_string(),
                            space_id: ids::SHELLS,
                            coords: coords.to_vec(),
                        },
                        DbEndpoint {
                            role: "face".to_string(),
                            space_id: ids::FACES,
                            coords: f_coords.to_vec(),
                        },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
                he_id += 1;
            }
        }

        (record, hyperedges)
    };

    #[cfg(feature = "parallel")]
    let shell_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .shells
        .par_iter()
        .enumerate()
        .map(|(i, shell)| encode_one(i, shell))
        .collect();
    #[cfg(not(feature = "parallel"))]
    let shell_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .shells
        .iter()
        .enumerate()
        .map(|(i, shell)| encode_one(i, shell))
        .collect();

    for (record, hyperedges) in shell_outputs {
        out.records.push(record);
        out.hyperedges.extend(hyperedges);
    }
}

fn encode_solids(model: &GeometryModel, config: &EncoderConfig, he_base: u64, out: &mut EncodedModel) {
    let solid_he_offsets: Vec<usize> = {
        let mut off = 0usize;
        model
            .solids
            .iter()
            .map(|s| {
                let start = off;
                off += s.shell_ids.len();
                start
            })
            .collect()
    };

    let encode_one = |i: usize, solid: &Solid| {
        let coords = encode_point_3d(
            solid.centroid.x,
            solid.centroid.y,
            solid.centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );

        let record = DbRecord {
            address: DbAddress {
                space_id: ids::SOLIDS,
                coords: coords.to_vec(),
            },
            payload: serde_json::json!({
                "id": solid.id,
                "name": solid.name,
                "shell_count": solid.shell_ids.len(),
                "centroid": { "x": solid.centroid.x, "y": solid.centroid.y, "z": solid.centroid.z },
                "bbox_min": { "x": solid.bounding_box_min.x, "y": solid.bounding_box_min.y, "z": solid.bounding_box_min.z },
                "bbox_max": { "x": solid.bounding_box_max.x, "y": solid.bounding_box_max.y, "z": solid.bounding_box_max.z },
            }),
            entity_type: "solid".to_string(),
        };

        let mut he_id = he_base + solid_he_offsets[i] as u64;
        let mut hyperedges = Vec::new();
        for sid in &solid.shell_ids {
            if let Some(shell) = model.shell(*sid) {
                let s_coords = encode_point_3d(
                    shell.centroid.x,
                    shell.centroid.y,
                    shell.centroid.z,
                    config.coord_max,
                    config.bits_per_dim,
                );
                hyperedges.push(DbHyperedge {
                    id: he_id,
                    kind: "solid.contains_shell".to_string(),
                    endpoints: vec![
                        DbEndpoint {
                            role: "solid".to_string(),
                            space_id: ids::SOLIDS,
                            coords: coords.to_vec(),
                        },
                        DbEndpoint {
                            role: "shell".to_string(),
                            space_id: ids::SHELLS,
                            coords: s_coords.to_vec(),
                        },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
                he_id += 1;
            }
        }

        (record, hyperedges)
    };

    #[cfg(feature = "parallel")]
    let solid_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .solids
        .par_iter()
        .enumerate()
        .map(|(i, solid)| encode_one(i, solid))
        .collect();
    #[cfg(not(feature = "parallel"))]
    let solid_outputs: Vec<(DbRecord, Vec<DbHyperedge>)> = model
        .solids
        .iter()
        .enumerate()
        .map(|(i, solid)| encode_one(i, solid))
        .collect();

    for (record, hyperedges) in solid_outputs {
        out.records.push(record);
        out.hyperedges.extend(hyperedges);
    }
}
 
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
 
fn vertex_coords_for(vertex_id: u64, model: &GeometryModel, config: &EncoderConfig) -> Vec<u32> {
    model
        .vertex(vertex_id)
        .map(|v| {
            encode_point_3d(
                v.position.x,
                v.position.y,
                v.position.z,
                config.coord_max,
                config.bits_per_dim,
            )
            .to_vec()
        })
        .unwrap_or_else(|| vec![0, 0, 0])
}
 
fn surface_type_name(s: &SurfaceKind) -> &str {
    match s {
        SurfaceKind::Plane { .. }           => "PLANE",
        SurfaceKind::Cylinder { .. }        => "CYLINDRICAL_SURFACE",
        SurfaceKind::Sphere { .. }          => "SPHERICAL_SURFACE",
        SurfaceKind::Cone { .. }            => "CONICAL_SURFACE",
        SurfaceKind::Torus { .. }           => "TOROIDAL_SURFACE",
        SurfaceKind::BSplineSurface { .. }  => "B_SPLINE_SURFACE",
        SurfaceKind::Unknown { type_name }  => type_name.as_str(),
    }
}
 
fn curve_type_name(c: &CurveKind) -> &str {
    match c {
        CurveKind::Line { .. }         => "LINE",
        CurveKind::Circle { .. }       => "CIRCLE",
        CurveKind::Ellipse { .. }      => "ELLIPSE",
        CurveKind::BSplineCurve { .. } => "B_SPLINE_CURVE",
        CurveKind::Unknown { type_name } => type_name.as_str(),
    }
}