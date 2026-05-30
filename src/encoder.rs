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
 
    // Register spaces
    out.spaces = vec![
        SpaceRegistration { id: ids::SOLIDS,      name: "solids".into(),       dims: dims::SPATIAL_3D,  description: "Closed solid bodies".into() },
        SpaceRegistration { id: ids::SHELLS,      name: "shells".into(),       dims: dims::SPATIAL_3D,  description: "Open/closed shells".into() },
        SpaceRegistration { id: ids::FACES,       name: "faces".into(),        dims: dims::SPATIAL_3D,  description: "Individual faces with surface type".into() },
        SpaceRegistration { id: ids::EDGES,       name: "edges".into(),        dims: dims::SPATIAL_3D,  description: "Curve edges between vertices".into() },
        SpaceRegistration { id: ids::VERTICES,    name: "vertices".into(),     dims: dims::SPATIAL_3D,  description: "Point vertices".into() },
        SpaceRegistration { id: ids::BOUNDARY_FN, name: "boundary_fn".into(),  dims: dims::BOUNDARY_6D, description: "SDF boundary functions (centroid + normal)".into() },
        SpaceRegistration { id: ids::TOPOLOGY,    name: "topology".into(),     dims: dims::TOPOLOGY_2D, description: "Topological relationship hyperedges".into() },
    ];
 
    // Build a map of edge_id → face_ids for adjacency hyperedges
    let mut edge_to_faces: HashMap<u64, Vec<u64>> = HashMap::new();
    for face in &model.faces {
        for eid in &face.edge_loop {
            edge_to_faces.entry(*eid).or_default().push(face.id);
        }
    }
 
    let mut edge_id_counter = 100_000u64; // Hyperedge IDs (separate from entity IDs)
 
    // --- Encode Vertices ---
    for v in &model.vertices {
        let coords = encode_point_3d(v.position.x, v.position.y, v.position.z, config.coord_max, config.bits_per_dim);
        out.records.push(DbRecord {
            address: DbAddress { space_id: ids::VERTICES, coords: coords.to_vec() },
            payload: serde_json::json!({
                "id": v.id,
                "name": v.name,
                "x": v.position.x,
                "y": v.position.y,
                "z": v.position.z,
            }),
            entity_type: "vertex".to_string(),
        });
    }
 
    // --- Encode Edges ---
    for e in &model.edges {
        let coords = encode_point_3d(e.midpoint.x, e.midpoint.y, e.midpoint.z, config.coord_max, config.bits_per_dim);
        out.records.push(DbRecord {
            address: DbAddress { space_id: ids::EDGES, coords: coords.to_vec() },
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
        });
 
        // Edge → start vertex
        let v_start = vertex_coords_for(e.start_vertex_id, model, config);
        edge_id_counter += 1;
        out.hyperedges.push(DbHyperedge {
            id: edge_id_counter,
            kind: "edge.has_start_vertex".to_string(),
            endpoints: vec![
                DbEndpoint { role: "edge".to_string(),   space_id: ids::EDGES,    coords: coords.to_vec() },
                DbEndpoint { role: "vertex".to_string(), space_id: ids::VERTICES, coords: v_start },
            ],
            weight_milli: Some((e.length_estimate * 1000.0) as i64),
            metadata: HashMap::new(),
        });
 
        // Edge → end vertex
        let v_end = vertex_coords_for(e.end_vertex_id, model, config);
        edge_id_counter += 1;
        out.hyperedges.push(DbHyperedge {
            id: edge_id_counter,
            kind: "edge.has_end_vertex".to_string(),
            endpoints: vec![
                DbEndpoint { role: "edge".to_string(),   space_id: ids::EDGES,    coords: coords.to_vec() },
                DbEndpoint { role: "vertex".to_string(), space_id: ids::VERTICES, coords: v_end },
            ],
            weight_milli: None,
            metadata: HashMap::new(),
        });
    }
 
    // --- Encode Faces + Boundary Functions ---
    for face in &model.faces {
        let coords = encode_point_3d(
            face.centroid.x, face.centroid.y, face.centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );
 
        out.records.push(DbRecord {
            address: DbAddress { space_id: ids::FACES, coords: coords.to_vec() },
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
        });
 
        // Build and store the boundary function
        let bfn = boundary_fn_from_surface(
            face.id,
            &face.surface,
            face.centroid,
            face.normal_at_centroid,
            face.area_estimate,
        );
 
        let bfn_coords = encode_point_6d(
            face.centroid.x, face.centroid.y, face.centroid.z,
            face.normal_at_centroid.x, face.normal_at_centroid.y, face.normal_at_centroid.z,
            config.coord_max,
            config.bits_per_dim,
        );

        let (sdf_at_centroid, sdf_is_approximate) = bfn.function.evaluate(face.centroid);
        let contains_centroid = bfn.function.contains(face.centroid);
        let mut bfn_json = serde_json::to_value(&bfn)?;
        if let serde_json::Value::Object(ref mut map) = bfn_json {
            map.insert("sdf_at_centroid".into(), serde_json::json!(sdf_at_centroid));
            map.insert("sdf_is_approximate".into(), serde_json::json!(sdf_is_approximate));
            map.insert("contains_centroid".into(), serde_json::json!(contains_centroid));
        }
        out.boundary_fns.push(DbRecord {
            address: DbAddress { space_id: ids::BOUNDARY_FN, coords: bfn_coords.to_vec() },
            payload: bfn_json,
            entity_type: format!("boundary_fn::{}", bfn.function.type_name()),
        });
 
        // Hyperedge: face → boundary function
        edge_id_counter += 1;
        out.hyperedges.push(DbHyperedge {
            id: edge_id_counter,
            kind: "face.has_boundary_fn".to_string(),
            endpoints: vec![
                DbEndpoint { role: "face".to_string(),        space_id: ids::FACES,       coords: coords.to_vec() },
                DbEndpoint { role: "boundary_fn".to_string(), space_id: ids::BOUNDARY_FN, coords: bfn_coords.to_vec() },
            ],
            weight_milli: Some((face.area_estimate * 1000.0) as i64),
            metadata: {
                let mut m = HashMap::new();
                m.insert("surface_type".to_string(), surface_type_name(&face.surface).to_string());
                m.insert("is_exact_sdf".to_string(), bfn.is_exact.to_string());
                m
            },
        });
 
        // Hyperedges: face → each boundary edge
        for eid in &face.edge_loop {
            if let Some(e) = model.edges.iter().find(|e| e.id == *eid) {
                let e_coords = encode_point_3d(e.midpoint.x, e.midpoint.y, e.midpoint.z, config.coord_max, config.bits_per_dim);
                edge_id_counter += 1;
                out.hyperedges.push(DbHyperedge {
                    id: edge_id_counter,
                    kind: "face.has_boundary_edge".to_string(),
                    endpoints: vec![
                        DbEndpoint { role: "face".to_string(), space_id: ids::FACES, coords: coords.to_vec() },
                        DbEndpoint { role: "edge".to_string(), space_id: ids::EDGES, coords: e_coords.to_vec() },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
            }
        }
    }
 
    // --- Face Adjacency Hyperedges ---
    // Two faces sharing an edge are adjacent. This creates the "topology graph"
    // that powers questions like "find all faces adjacent to face X".
    let face_coord_map: HashMap<u64, Vec<u32>> = model.faces.iter().map(|f| {
        let c = encode_point_3d(f.centroid.x, f.centroid.y, f.centroid.z, config.coord_max, config.bits_per_dim);
        (f.id, c.to_vec())
    }).collect();
 
    for (edge_id, face_ids) in &edge_to_faces {
        if face_ids.len() >= 2 {
            // Emit adjacency for every pair of faces sharing this edge
            for i in 0..face_ids.len() {
                for j in i+1..face_ids.len() {
                    let fa_id = face_ids[i];
                    let fb_id = face_ids[j];
                    if let (Some(ca), Some(cb)) = (face_coord_map.get(&fa_id), face_coord_map.get(&fb_id)) {
                        edge_id_counter += 1;
                        let mut meta = HashMap::new();
                        meta.insert("shared_edge_id".to_string(), edge_id.to_string());
                        out.hyperedges.push(DbHyperedge {
                            id: edge_id_counter,
                            kind: "face.is_adjacent_to_face".to_string(),
                            endpoints: vec![
                                DbEndpoint { role: "face_a".to_string(), space_id: ids::FACES, coords: ca.clone() },
                                DbEndpoint { role: "face_b".to_string(), space_id: ids::FACES, coords: cb.clone() },
                            ],
                            weight_milli: None,
                            metadata: meta,
                        });
                    }
                }
            }
        }
    }
 
    // --- Encode Shells ---
    for shell in &model.shells {
        let coords = encode_point_3d(shell.centroid.x, shell.centroid.y, shell.centroid.z, config.coord_max, config.bits_per_dim);
 
        out.records.push(DbRecord {
            address: DbAddress { space_id: ids::SHELLS, coords: coords.to_vec() },
            payload: serde_json::json!({
                "id": shell.id,
                "name": shell.name,
                "is_closed": shell.is_closed,
                "face_count": shell.face_ids.len(),
                "centroid": { "x": shell.centroid.x, "y": shell.centroid.y, "z": shell.centroid.z },
            }),
            entity_type: "shell".to_string(),
        });
 
        // Shell → contained faces
        for fid in &shell.face_ids {
            if let Some(face) = model.faces.iter().find(|f| f.id == *fid) {
                let f_coords = encode_point_3d(face.centroid.x, face.centroid.y, face.centroid.z, config.coord_max, config.bits_per_dim);
                edge_id_counter += 1;
                out.hyperedges.push(DbHyperedge {
                    id: edge_id_counter,
                    kind: "shell.contains_face".to_string(),
                    endpoints: vec![
                        DbEndpoint { role: "shell".to_string(), space_id: ids::SHELLS, coords: coords.to_vec() },
                        DbEndpoint { role: "face".to_string(),  space_id: ids::FACES,  coords: f_coords.to_vec() },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
            }
        }
    }
 
    // --- Encode Solids ---
    for solid in &model.solids {
        let coords = encode_point_3d(solid.centroid.x, solid.centroid.y, solid.centroid.z, config.coord_max, config.bits_per_dim);
 
        out.records.push(DbRecord {
            address: DbAddress { space_id: ids::SOLIDS, coords: coords.to_vec() },
            payload: serde_json::json!({
                "id": solid.id,
                "name": solid.name,
                "shell_count": solid.shell_ids.len(),
                "centroid": { "x": solid.centroid.x, "y": solid.centroid.y, "z": solid.centroid.z },
                "bbox_min": { "x": solid.bounding_box_min.x, "y": solid.bounding_box_min.y, "z": solid.bounding_box_min.z },
                "bbox_max": { "x": solid.bounding_box_max.x, "y": solid.bounding_box_max.y, "z": solid.bounding_box_max.z },
            }),
            entity_type: "solid".to_string(),
        });
 
        // Solid → shells
        for sid in &solid.shell_ids {
            if let Some(shell) = model.shells.iter().find(|s| s.id == *sid) {
                let s_coords = encode_point_3d(shell.centroid.x, shell.centroid.y, shell.centroid.z, config.coord_max, config.bits_per_dim);
                edge_id_counter += 1;
                out.hyperedges.push(DbHyperedge {
                    id: edge_id_counter,
                    kind: "solid.contains_shell".to_string(),
                    endpoints: vec![
                        DbEndpoint { role: "solid".to_string(), space_id: ids::SOLIDS, coords: coords.to_vec() },
                        DbEndpoint { role: "shell".to_string(), space_id: ids::SHELLS, coords: s_coords.to_vec() },
                    ],
                    weight_milli: None,
                    metadata: HashMap::new(),
                });
            }
        }
    }
 
    Ok(out)
}
 
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
 
fn vertex_coords_for(vertex_id: u64, model: &GeometryModel, config: &EncoderConfig) -> Vec<u32> {
    model.vertices.iter()
        .find(|v| v.id == vertex_id)
        .map(|v| encode_point_3d(v.position.x, v.position.y, v.position.z, config.coord_max, config.bits_per_dim).to_vec())
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