//! STEP (ISO 10303-21) parser.
//!
//! STEP Part 21 files have this structure:
//!
//! ```text
//! ISO-10303-21;
//! HEADER;
//!   FILE_DESCRIPTION(...);
//!   FILE_NAME(...);
//!   FILE_SCHEMA(('AP203_CONFIGURATION_CONTROLLED_3D_DESIGN_OF_MECHANICAL_PARTS_AND_ASSEMBLIES_MIM_LF { ... }'));
//! ENDSEC;
//! DATA;
//!   #1 = PRODUCT('part_name', ...) ;
//!   #2 = CARTESIAN_POINT('', (0.0, 0.0, 0.0)) ;
//!   ...
//! ENDSEC;
//! END-ISO-10303-21;
//! ```
//!
//! Rather than depend on a full STEP schema parser (which would require the
//! entire AP203 schema compiled in), we implement a robust hand-written parser
//! that understands the P21 physical file format and extracts the entities we
//! care about by type name.
//!
//! ## Parsing Strategy
//!
//! 1. Tokenize: extract `#id = TYPE_NAME(args...)` entity records.
//! 2. Build an entity map: `HashMap<u64, RawEntity>`.
//! 3. Walk specific entity types (MANIFOLD_SOLID_BREP, ADVANCED_FACE, etc.)
//!    and dereference `#id` pointers to build our intermediate model.
//!
//! ## Entity Reference Resolution
//!
//! STEP entities refer to each other by `#id`. The approach is a two-pass:
//! - Pass 1: parse all raw entities into a map.
//! - Pass 2: for each solid/shell/face/edge/vertex, resolve `#id` refs
//!   and materialize the full typed struct.
 
use std::collections::HashMap;
use anyhow::{Result, anyhow, bail};
use crate::geometry::*;
 
/// A raw, unparsed STEP entity.
#[derive(Debug, Clone)]
pub struct RawEntity {
    pub id: u64,
    pub type_name: String,
    /// The raw argument string inside the outermost parens.
    pub args: String,
}
 
/// Parsed STEP file ready for entity resolution.
pub struct StepFile {
    pub entities: HashMap<u64, RawEntity>,
}
 
/// Parse the DATA section of a STEP file into raw entities.
pub fn parse_raw(text: &str) -> Result<StepFile> {
    let mut entities = HashMap::new();
    let mut in_data = false;
 
    for line in text.lines() {
        let line = line.trim();
        if line == "DATA;" { in_data = true; continue; }
        if line == "ENDSEC;" { in_data = false; continue; }
        if !in_data || !line.starts_with('#') { continue; }
 
        if let Some(entity) = parse_entity_line(line) {
            entities.insert(entity.id, entity);
        }
    }
 
    Ok(StepFile { entities })
}
 
/// Parse a single entity line like `#12 = CARTESIAN_POINT('',( 0.0,0.0,0.0));`
fn parse_entity_line(line: &str) -> Option<RawEntity> {
    // Strip trailing semicolon
    let line = line.trim_end_matches(';').trim();
 
    // Find `#id`
    let hash_pos = line.find('#')?;
    let rest = &line[hash_pos + 1..];
    let eq_pos = rest.find('=')?;
    let id_str = rest[..eq_pos].trim();
    let id: u64 = id_str.parse().ok()?;
 
    let after_eq = rest[eq_pos + 1..].trim();
 
    // Find type name and argument block
    let paren_pos = after_eq.find('(')?;
    let type_name = after_eq[..paren_pos].trim().to_uppercase();
    let args_with_close = &after_eq[paren_pos + 1..];
 
    // Extract balanced parens
    let args = extract_balanced_args(args_with_close);
 
    Some(RawEntity { id, type_name, args })
}
 
/// Extract the contents of the outermost parentheses, handling nesting.
fn extract_balanced_args(s: &str) -> String {
    let mut depth = 1i32;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 { end = i; break; }
            }
            _ => {}
        }
    }
    s[..end].to_string()
}
 
/// Split top-level comma-separated arguments (respecting nested parens and strings).
fn split_args(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut current = String::new();
 
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !in_string => { in_string = true; current.push(c); }
            '\'' if in_string => {
                // Handle escaped single quote: ''
                if i + 1 < chars.len() && chars[i+1] == '\'' {
                    current.push('\'');
                    i += 1;
                } else {
                    in_string = false;
                    current.push(c);
                }
            }
            '(' if !in_string => { depth += 1; current.push(c); }
            ')' if !in_string => { depth -= 1; current.push(c); }
            ',' if !in_string && depth == 0 => {
                result.push(current.trim().to_string());
                current = String::new();
            }
            _ => { current.push(c); }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        result.push(current.trim().to_string());
    }
    result
}
 
/// Parse a STEP string literal: `'contents'` → `contents`.
fn parse_string(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('\'') && s.ends_with('\'') {
        s[1..s.len()-1].to_string()
    } else {
        s.to_string()
    }
}

/// Name argument at `index`, or empty string if missing (avoids panics on short entity args).
fn arg_name(args: &[String], index: usize) -> String {
    args.get(index).map(|s| parse_string(s)).unwrap_or_default()
}
 
/// Parse a STEP float (handles '.' shorthand like `.5` or `1.`).
fn parse_float(s: &str) -> Result<f64> {
    s.trim().parse::<f64>().map_err(|e| anyhow!("bad float {:?}: {}", s, e))
}
 
/// Parse a `#id` reference.
fn parse_ref(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(stripped) = s.strip_prefix('#') {
        stripped.parse::<u64>().map_err(|e| anyhow!("bad ref {:?}: {}", s, e))
    } else {
        bail!("expected #ref, got {:?}", s)
    }
}
 
/// Parse `(x, y, z)` coordinates from a tuple argument.
fn parse_tuple_f64(s: &str) -> Result<Vec<f64>> {
    let s = s.trim().trim_start_matches('(').trim_end_matches(')');
    split_args(s).iter().map(|v| parse_float(v)).collect()
}
 
// ---------------------------------------------------------------------------
// Entity resolution
// ---------------------------------------------------------------------------
 
/// High-level parser: takes raw text and produces the full intermediate model.
pub fn parse_step(text: &str) -> Result<GeometryModel> {
    let step = parse_raw(text)?;
    let resolver = EntityResolver { entities: &step.entities };
    resolver.build_model()
}
 
struct EntityResolver<'a> {
    entities: &'a HashMap<u64, RawEntity>,
}
 
impl<'a> EntityResolver<'a> {
    fn get(&self, id: u64) -> Result<&RawEntity> {
        self.entities.get(&id).ok_or_else(|| anyhow!("missing entity #{}", id))
    }
 
    fn build_model(&self) -> Result<GeometryModel> {
        let mut model = GeometryModel::new();
 
        // Collect raw entity names for diagnostics
        for (id, e) in self.entities {
            model.entities.insert(*id, e.type_name.clone());
        }
 
        // Resolve vertices (VERTEX_POINT)
        for (id, e) in self.entities {
            if e.type_name == "VERTEX_POINT" {
                if let Ok(v) = self.resolve_vertex(*id) {
                    model.vertices.push(v);
                }
            }
        }
 
        // Resolve edges (EDGE_CURVE)
        for (id, e) in self.entities {
            if e.type_name == "EDGE_CURVE" {
                if let Ok(edge) = self.resolve_edge(*id, &model) {
                    model.edges.push(edge);
                }
            }
        }
 
        // Resolve faces (ADVANCED_FACE)
        for (id, e) in self.entities {
            if e.type_name == "ADVANCED_FACE" {
                if let Ok(face) = self.resolve_face(*id, &model) {
                    model.faces.push(face);
                }
            }
        }
 
        // Resolve shells
        let mut shell_id_counter = 0u64;
        for (id, e) in self.entities {
            let is_shell = matches!(
                e.type_name.as_str(),
                "CLOSED_SHELL" | "OPEN_SHELL" | "SHELL_BASED_SURFACE_MODEL"
            );
            if is_shell {
                if let Ok(shell) = self.resolve_shell(*id, &model, &mut shell_id_counter) {
                    model.shells.push(shell);
                }
            }
        }
 
        // Resolve solids (MANIFOLD_SOLID_BREP)
        for (id, e) in self.entities {
            if e.type_name == "MANIFOLD_SOLID_BREP" {
                if let Ok(solid) = self.resolve_solid(*id, &model) {
                    model.solids.push(solid);
                }
            }
        }
 
        // If no solids found but we have shells, synthesize a top-level solid
        if model.solids.is_empty() && !model.shells.is_empty() {
            let shell_ids: Vec<u64> = model.shells.iter().map(|s| s.id).collect();
            let all_centroids: Vec<Point3> = model.shells.iter().map(|s| s.centroid).collect();
            let centroid = Point3::centroid(&all_centroids);
            let (bbox_min, bbox_max) = model.global_bbox();
            model.solids.push(Solid {
                id: 0,
                name: "synthetic_solid".to_string(),
                shell_ids,
                centroid,
                bounding_box_min: bbox_min,
                bounding_box_max: bbox_max,
                volume_estimate: 0.0,
            });
        }
 
        Ok(model)
    }
 
    fn resolve_vertex(&self, id: u64) -> Result<Vertex> {
        let e = self.get(id)?;
        let args = split_args(&e.args);
        if args.len() < 2 { bail!("VERTEX_POINT #{} needs 2 args", id); }
        let name = arg_name(&args, 0);
        let pt_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
        let pos = self.resolve_cartesian_point(pt_id)?;
        Ok(Vertex { id, name, position: pos })
    }
 
    fn resolve_cartesian_point(&self, id: u64) -> Result<Point3> {
        let e = self.get(id)?;
        if e.type_name != "CARTESIAN_POINT" {
            bail!("expected CARTESIAN_POINT at #{}, got {}", id, e.type_name);
        }
        let args = split_args(&e.args);
        if args.len() < 2 { bail!("CARTESIAN_POINT needs coords"); }
        let coords = parse_tuple_f64(&args[1])?;
        Ok(Point3::new(
            *coords.get(0).unwrap_or(&0.0),
            *coords.get(1).unwrap_or(&0.0),
            *coords.get(2).unwrap_or(&0.0),
        ))
    }
 
    fn resolve_direction(&self, id: u64) -> Result<Dir3> {
        let e = self.get(id)?;
        if e.type_name != "DIRECTION" {
            bail!("expected DIRECTION at #{}", id);
        }
        let args = split_args(&e.args);
        if args.len() < 2 { bail!("DIRECTION needs coords"); }
        let coords = parse_tuple_f64(&args[1])?;
        Ok(Dir3::new(
            *coords.get(0).unwrap_or(&0.0),
            *coords.get(1).unwrap_or(&0.0),
            *coords.get(2).unwrap_or(&1.0),
        ))
    }
 
    fn resolve_axis2_placement(&self, id: u64) -> Result<Axis2Placement> {
        let e = self.get(id)?;
        if !e.type_name.contains("AXIS2_PLACEMENT") {
            return Ok(Axis2Placement::identity());
        }
        let args = split_args(&e.args);
        let location = if args.len() > 1 { self.resolve_cartesian_point(parse_ref(&args[1])?) } else { Ok(Point3::origin()) };
        let axis = if args.len() > 2 { self.resolve_direction(parse_ref(&args[2])?) } else { Ok(Dir3::up()) };
        let ref_dir = if args.len() > 3 { self.resolve_direction(parse_ref(&args[3])?) } else { Ok(Dir3::new(1.0, 0.0, 0.0)) };
        Ok(Axis2Placement {
            location: location.unwrap_or_else(|_| Point3::origin()),
            axis: axis.unwrap_or_else(|_| Dir3::up()),
            ref_dir: ref_dir.unwrap_or_else(|_| Dir3::new(1.0, 0.0, 0.0)),
        })
    }
 
    fn resolve_surface(&self, id: u64) -> Result<SurfaceKind> {
        let e = self.get(id)?;
        match e.type_name.as_str() {
            "PLANE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(&args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                Ok(SurfaceKind::Plane {
                    normal: placement.axis,
                    point_on_plane: placement.location,
                })
            }
            "CYLINDRICAL_SURFACE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let radius = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(SurfaceKind::Cylinder { placement, radius })
            }
            "SPHERICAL_SURFACE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let radius = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(SurfaceKind::Sphere {
                    center: placement.location,
                    radius,
                })
            }
            "CONICAL_SURFACE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let radius = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                let semi_angle = parse_float(args.get(3).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(SurfaceKind::Cone { placement, radius, semi_angle })
            }
            "TOROIDAL_SURFACE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let major_r = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                let minor_r = parse_float(args.get(3).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(SurfaceKind::Torus { placement, major_radius: major_r, minor_radius: minor_r })
            }
            t if t.contains("B_SPLINE") || t.contains("BEZIER") || t.contains("NURBS") => {
                let args = split_args(&e.args);
                // Extract any sub-entity control point refs we can find
                let mut control_pts = Vec::new();
                // Attempt to pull control point arrays from nested refs
                for arg in &args[2..] {
                    let trimmed = arg.trim().trim_matches('(').trim_matches(')');
                    for sub in split_args(trimmed) {
                        if let Ok(ref_id) = parse_ref(&sub) {
                            if let Ok(pt) = self.resolve_cartesian_point(ref_id) {
                                control_pts.push(vec![pt]);
                            }
                        }
                    }
                }
                let flat: Vec<Point3> = control_pts.iter().flatten().cloned().collect();
                let bsc = if flat.is_empty() { Point3::origin() } else { Point3::centroid(&flat) };
                let bsr = flat.iter().map(|p| p.distance_to(&bsc)).fold(0.0_f64, f64::max);
                Ok(SurfaceKind::BSplineSurface {
                    degree_u: 3, degree_v: 3,
                    control_points: control_pts,
                    bounding_sphere_center: bsc,
                    bounding_sphere_radius: bsr.max(1.0),
                })
            }
            other => Ok(SurfaceKind::Unknown { type_name: other.to_string() }),
        }
    }
 
    fn resolve_edge(&self, id: u64, model: &GeometryModel) -> Result<Edge> {
        let e = self.get(id)?;
        let args = split_args(&e.args);
        let name = arg_name(&args, 0);
        let start_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
        let end_id   = parse_ref(args.get(2).map(|s| s.as_str()).unwrap_or("$"))?;
        let curve_ref = args.get(3).and_then(|s| parse_ref(s).ok()).unwrap_or(0);
 
        let start_pos = model.vertex(start_id).map(|v| v.position).unwrap_or_default();
        let end_pos   = model.vertex(end_id).map(|v| v.position).unwrap_or_default();
        let midpoint  = Point3::centroid(&[start_pos, end_pos]);
        let length    = start_pos.distance_to(&end_pos);
 
        let curve = if curve_ref > 0 {
            self.resolve_curve(curve_ref).unwrap_or(CurveKind::Line {
                start: start_pos,
                direction: Dir3::new(
                    end_pos.x - start_pos.x,
                    end_pos.y - start_pos.y,
                    end_pos.z - start_pos.z,
                ),
            })
        } else {
            CurveKind::Line {
                start: start_pos,
                direction: Dir3::new(
                    end_pos.x - start_pos.x,
                    end_pos.y - start_pos.y,
                    end_pos.z - start_pos.z,
                ),
            }
        };
 
        Ok(Edge {
            id,
            name,
            start_vertex_id: start_id,
            end_vertex_id: end_id,
            curve,
            midpoint,
            length_estimate: length,
        })
    }
 
    fn resolve_curve(&self, id: u64) -> Result<CurveKind> {
        let e = self.get(id)?;
        match e.type_name.as_str() {
            "LINE" => {
                let args = split_args(&e.args);
                let pt_id  = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let dir_id = args.get(2).and_then(|s| {
                    // LINE references a VECTOR, which in turn refs a DIRECTION
                    parse_ref(s).ok()
                }).unwrap_or(0);
                let start = self.resolve_cartesian_point(pt_id)?;
                let dir = if dir_id > 0 { self.resolve_vector_direction(dir_id).unwrap_or(Dir3::up()) } else { Dir3::up() };
                Ok(CurveKind::Line { start, direction: dir })
            }
            "CIRCLE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let radius = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(CurveKind::Circle { placement, radius })
            }
            "ELLIPSE" => {
                let args = split_args(&e.args);
                let placement_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
                let placement = self.resolve_axis2_placement(placement_id)?;
                let a1 = parse_float(args.get(2).map(|s| s.as_str()).unwrap_or("0"))?;
                let a2 = parse_float(args.get(3).map(|s| s.as_str()).unwrap_or("0"))?;
                Ok(CurveKind::Ellipse { placement, semi_axis_1: a1, semi_axis_2: a2 })
            }
            t if t.contains("B_SPLINE") || t.contains("BEZIER") => {
                let args = split_args(&e.args);
                let mut pts = Vec::new();
                for arg in &args[2..] {
                    let trimmed = arg.trim().trim_matches('(').trim_matches(')');
                    for sub in split_args(trimmed) {
                        if let Ok(ref_id) = parse_ref(&sub) {
                            if let Ok(pt) = self.resolve_cartesian_point(ref_id) {
                                pts.push(pt);
                            }
                        }
                    }
                }
                Ok(CurveKind::BSplineCurve { degree: 3, control_points: pts })
            }
            other => Ok(CurveKind::Unknown { type_name: other.to_string() }),
        }
    }
 
    /// Resolve a VECTOR entity to its embedded DIRECTION.
    fn resolve_vector_direction(&self, id: u64) -> Result<Dir3> {
        let e = self.get(id)?;
        if e.type_name == "VECTOR" {
            let args = split_args(&e.args);
            let dir_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
            self.resolve_direction(dir_id)
        } else if e.type_name == "DIRECTION" {
            self.resolve_direction(id)
        } else {
            bail!("expected VECTOR/DIRECTION at #{}", id)
        }
    }
 
    fn resolve_face(&self, id: u64, model: &GeometryModel) -> Result<Face> {
        let e = self.get(id)?;
        let args = split_args(&e.args);
        let name = arg_name(&args, 0);

        // ADVANCED_FACE: (name, (edge_loops...), surface_ref, same_sense)
        // Collect edge loop face-bound refs
        let mut edge_ids: Vec<u64> = Vec::new();
        if let Some(loop_list) = args.get(1) {
            let trimmed = loop_list.trim().trim_matches('(').trim_matches(')');
            for sub in split_args(trimmed) {
                // Each is a FACE_OUTER_BOUND or FACE_BOUND ref
                if let Ok(bound_id) = parse_ref(&sub) {
                    let edges = self.resolve_face_bound(bound_id).unwrap_or_default();
                    edge_ids.extend(edges);
                }
            }
        }
 
        let surface_id = parse_ref(args.get(2).map(|s| s.as_str()).unwrap_or("$"))?;
        let surface = self.resolve_surface(surface_id)?;
 
        let same_sense_str = args.get(3).map(|s| s.trim()).unwrap_or(".T.");
        let same_sense = same_sense_str.contains('T');
 
        // Compute centroid from boundary vertices
        let mut boundary_pts: Vec<Point3> = Vec::new();
        for eid in &edge_ids {
            if let Some(edge) = model.edges.iter().find(|e| e.id == *eid) {
                boundary_pts.push(edge.midpoint);
            }
        }
        let centroid = if !boundary_pts.is_empty() {
            Point3::centroid(&boundary_pts)
        } else {
            // Fall back to surface placement
            match &surface {
                SurfaceKind::Plane { point_on_plane, .. } => *point_on_plane,
                SurfaceKind::Cylinder { placement, .. } => placement.location,
                SurfaceKind::Sphere { center, .. } => *center,
                SurfaceKind::Cone { placement, .. } => placement.location,
                SurfaceKind::Torus { placement, .. } => placement.location,
                SurfaceKind::BSplineSurface { bounding_sphere_center, .. } => *bounding_sphere_center,
                SurfaceKind::Unknown { .. } => Point3::origin(),
            }
        };
 
        let normal_at_centroid = match &surface {
            SurfaceKind::Plane { normal, .. } => {
                if same_sense { *normal } else { Dir3::new(-normal.x, -normal.y, -normal.z) }
            }
            SurfaceKind::Cylinder { placement, .. } => placement.axis,
            SurfaceKind::Sphere { center, .. } => Dir3::new(
                centroid.x - center.x,
                centroid.y - center.y,
                centroid.z - center.z,
            ),
            _ => Dir3::up(),
        };
 
        // Rough area estimate: bounding box of boundary points
        let area_estimate = if boundary_pts.len() >= 3 {
            let (min_x, max_x) = boundary_pts.iter().map(|p| p.x)
                .fold((f64::MAX, f64::MIN), |(a,b), v| (a.min(v), b.max(v)));
            let (min_y, max_y) = boundary_pts.iter().map(|p| p.y)
                .fold((f64::MAX, f64::MIN), |(a,b), v| (a.min(v), b.max(v)));
            (max_x - min_x) * (max_y - min_y)
        } else {
            0.0
        };
 
        Ok(Face {
            id,
            name,
            surface,
            edge_loop: edge_ids,
            same_sense,
            centroid,
            normal_at_centroid,
            area_estimate,
        })
    }
 
    /// Resolve a FACE_BOUND / FACE_OUTER_BOUND to its list of edge IDs.
    fn resolve_face_bound(&self, id: u64) -> Result<Vec<u64>> {
        let e = self.get(id)?;
        if !e.type_name.contains("FACE_BOUND") { bail!("not a face bound"); }
        let args = split_args(&e.args);
        // FACE_BOUND: (name, loop_ref, orientation)
        let loop_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
        self.resolve_edge_loop(loop_id)
    }
 
    /// Resolve an EDGE_LOOP to its list of edge IDs.
    fn resolve_edge_loop(&self, id: u64) -> Result<Vec<u64>> {
        let e = self.get(id)?;
        if e.type_name != "EDGE_LOOP" { bail!("not an EDGE_LOOP"); }
        let args = split_args(&e.args);
        // EDGE_LOOP: (name, (oriented_edges...))
        let edges_list = args.get(1).map(|s| s.trim().trim_matches('(').trim_matches(')')).unwrap_or("");
        let mut edge_ids = Vec::new();
        for sub in split_args(edges_list) {
            if let Ok(oe_id) = parse_ref(&sub) {
                if let Ok(edge_id) = self.resolve_oriented_edge(oe_id) {
                    edge_ids.push(edge_id);
                }
            }
        }
        Ok(edge_ids)
    }
 
    /// Resolve an ORIENTED_EDGE to the underlying EDGE_CURVE ID.
    fn resolve_oriented_edge(&self, id: u64) -> Result<u64> {
        let e = self.get(id)?;
        if e.type_name != "ORIENTED_EDGE" { bail!("not ORIENTED_EDGE"); }
        let args = split_args(&e.args);
        // ORIENTED_EDGE: (name, *, *, edge_curve_ref, orientation)
        let edge_ref = args.get(3).map(|s| s.as_str()).unwrap_or("$");
        parse_ref(edge_ref)
    }
 
    fn resolve_shell(&self, id: u64, model: &GeometryModel, counter: &mut u64) -> Result<Shell> {
        let e = self.get(id)?;
        let args = split_args(&e.args);
        let name = arg_name(&args, 0);
        let is_closed = e.type_name.contains("CLOSED");

        let faces_list = args.get(1).map(|s| s.trim().trim_matches('(').trim_matches(')')).unwrap_or("");
        let mut face_ids: Vec<u64> = Vec::new();
        for sub in split_args(faces_list) {
            if let Ok(fid) = parse_ref(&sub) {
                face_ids.push(fid);
            }
        }
 
        let centroids: Vec<Point3> = face_ids.iter()
            .filter_map(|fid| model.faces.iter().find(|f| f.id == *fid))
            .map(|f| f.centroid)
            .collect();
 
        let centroid = Point3::centroid(&centroids);
 
        *counter += 1;
        Ok(Shell { id, name, face_ids, is_closed, centroid })
    }
 
    fn resolve_solid(&self, id: u64, model: &GeometryModel) -> Result<Solid> {
        let e = self.get(id)?;
        let args = split_args(&e.args);
        let name = arg_name(&args, 0);

        // MANIFOLD_SOLID_BREP: (name, shell_ref)
        let shell_id = parse_ref(args.get(1).map(|s| s.as_str()).unwrap_or("$"))?;
 
        let (bbox_min, bbox_max) = model.global_bbox();
        let centroids: Vec<Point3> = model.shells.iter()
            .filter(|s| s.id == shell_id)
            .map(|s| s.centroid)
            .collect();
        let centroid = Point3::centroid(&centroids);
 
        Ok(Solid {
            id,
            name,
            shell_ids: vec![shell_id],
            centroid,
            bounding_box_min: bbox_min,
            bounding_box_max: bbox_max,
            volume_estimate: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
 
    const MINIMAL_STEP: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Test'),'2;1');
FILE_NAME('test.stp','2024-01-01',(''),(''),'','','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }'));
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('origin',(0.0,0.0,0.0));
#2 = CARTESIAN_POINT('x_unit',(1.0,0.0,0.0));
#3 = DIRECTION('z_axis',(0.0,0.0,1.0));
#4 = VERTEX_POINT('v1',#1);
#5 = VERTEX_POINT('v2',#2);
ENDSEC;
END-ISO-10303-21;"#;
 
    #[test]
    fn parse_vertices() {
        let model = parse_step(MINIMAL_STEP).unwrap();
        assert_eq!(model.vertices.len(), 2);
        let v1 = model.vertices.iter().find(|v| v.name == "v1").unwrap();
        assert!((v1.position.x).abs() < 1e-10);
    }
 
    #[test]
    fn split_args_handles_nested_parens() {
        let args = split_args("'name', (#1, #2), 5.0");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "'name'");
    }

    #[test]
    fn shell_with_empty_args_does_not_panic() {
        const STEP: &str = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Test'),'2;1');
FILE_NAME('test.stp','2024-01-01',(''),(''),'','','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('',(0.0,0.0,0.0));
#2 = OPEN_SHELL();
ENDSEC;
END-ISO-10303-21;"#;
        let model = parse_step(STEP).unwrap();
        assert_eq!(model.shells.len(), 1);
        assert_eq!(model.shells[0].name, "");
        assert!(model.shells[0].face_ids.is_empty());
    }
}