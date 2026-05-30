//! Intermediate geometry model — the bridge between raw STEP entities
//! and InfiniteDB records.
//!
//! STEP (ISO 10303-21) represents solids as a Boundary REPresentation (BREP):
//! a hierarchy of topological entities referencing geometric ones.
//!
//! Topological (what connects to what):
//!   MANIFOLD_SOLID_BREP -> CLOSED_SHELL -> ADVANCED_FACE -> EDGE_LOOP ->
//!   ORIENTED_EDGE -> EDGE_CURVE -> VERTEX_POINT
//!
//! Geometric (where things are):
//!   CARTESIAN_POINT, DIRECTION, AXIS2_PLACEMENT_3D,
//!   PLANE, CYLINDRICAL_SURFACE, B_SPLINE_SURFACE, ...
//!
//! Our intermediate model flattens these into typed Rust structs that are
//! easier to work with than the raw STEP entity strings.
 
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
 
/// A 3D point in world space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
 
impl Point3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self { Self { x, y, z } }
    pub fn origin() -> Self { Self::new(0.0, 0.0, 0.0) }
 
    /// Centroid of a list of points.
    pub fn centroid(pts: &[Point3]) -> Self {
        if pts.is_empty() { return Self::origin(); }
        let n = pts.len() as f64;
        Self::new(
            pts.iter().map(|p| p.x).sum::<f64>() / n,
            pts.iter().map(|p| p.y).sum::<f64>() / n,
            pts.iter().map(|p| p.z).sum::<f64>() / n,
        )
    }
 
    pub fn distance_to(&self, other: &Point3) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx*dx + dy*dy + dz*dz).sqrt()
    }
}

impl Default for Point3 {
    fn default() -> Self { Point3::origin() }
}

/// A unit direction vector.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Dir3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
 
impl Dir3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        let len = (x*x + y*y + z*z).sqrt();
        if len < 1e-12 { return Self { x: 0.0, y: 0.0, z: 1.0 }; }
        Self { x: x/len, y: y/len, z: z/len }
    }
    pub fn up() -> Self { Self::new(0.0, 0.0, 1.0) }
}
 
/// An axis placement: a local coordinate frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Axis2Placement {
    pub location: Point3,
    pub axis: Dir3,     // Z axis
    pub ref_dir: Dir3,  // X axis (Y = axis x ref_dir)
}
 
impl Axis2Placement {
    pub fn identity() -> Self {
        Self {
            location: Point3::origin(),
            axis: Dir3::new(0.0, 0.0, 1.0),
            ref_dir: Dir3::new(1.0, 0.0, 0.0),
        }
    }
}
 
/// The type of surface a face lies on. This drives the SDF representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurfaceKind {
    /// Infinite flat plane: normal + point on plane.
    Plane {
        normal: Dir3,
        point_on_plane: Point3,
    },
    /// Right circular cylinder: axis, radius.
    Cylinder {
        placement: Axis2Placement,
        radius: f64,
    },
    /// Right circular cone: axis, radius at reference plane, half-angle.
    Cone {
        placement: Axis2Placement,
        radius: f64,
        semi_angle: f64,
    },
    /// Sphere: center + radius.
    Sphere {
        center: Point3,
        radius: f64,
    },
    /// Torus: center, major radius, minor radius.
    Torus {
        placement: Axis2Placement,
        major_radius: f64,
        minor_radius: f64,
    },
    /// General spline surface — we store the control points and fall back
    /// to a bounding-sphere approximation for the SDF.
    BSplineSurface {
        degree_u: u32,
        degree_v: u32,
        control_points: Vec<Vec<Point3>>,
        bounding_sphere_center: Point3,
        bounding_sphere_radius: f64,
    },
    /// Catch-all for unsupported surface types (still stored, SDF unavailable).
    Unknown {
        type_name: String,
    },
}
 
/// The type of curve an edge lies on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CurveKind {
    Line {
        start: Point3,
        direction: Dir3,
    },
    Circle {
        placement: Axis2Placement,
        radius: f64,
    },
    Ellipse {
        placement: Axis2Placement,
        semi_axis_1: f64,
        semi_axis_2: f64,
    },
    BSplineCurve {
        degree: u32,
        control_points: Vec<Point3>,
    },
    Unknown {
        type_name: String,
    },
}
 
/// A vertex — a point in 3D space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vertex {
    pub id: u64,
    pub name: String,
    pub position: Point3,
}
 
/// An edge connecting two vertices along a curve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub id: u64,
    pub name: String,
    pub start_vertex_id: u64,
    pub end_vertex_id: u64,
    pub curve: CurveKind,
    /// Midpoint of the edge (for spatial indexing).
    pub midpoint: Point3,
    /// Length estimate (used for weight_milli in topology hyperedges).
    pub length_estimate: f64,
}
 
/// A face — a bounded region of a surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    pub id: u64,
    pub name: String,
    pub surface: SurfaceKind,
    /// Edge IDs bounding this face.
    pub edge_loop: Vec<u64>,
    /// Face orientation (same/opposite sense relative to the surface normal).
    pub same_sense: bool,
    /// Centroid of the face's boundary vertices (for spatial indexing).
    pub centroid: Point3,
    /// Outward normal at the centroid.
    pub normal_at_centroid: Dir3,
    /// Approximate area (used in weight_milli).
    pub area_estimate: f64,
}
 
/// A shell — a connected set of faces forming a closed or open surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shell {
    pub id: u64,
    pub name: String,
    pub face_ids: Vec<u64>,
    pub is_closed: bool,
    pub centroid: Point3,
}
 
/// A manifold solid body — the top-level BREP entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Solid {
    pub id: u64,
    pub name: String,
    pub shell_ids: Vec<u64>,
    pub centroid: Point3,
    pub bounding_box_min: Point3,
    pub bounding_box_max: Point3,
    /// Volume estimate (milliliters if model is in mm).
    pub volume_estimate: f64,
}
 
/// The complete intermediate model extracted from one STEP file.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct GeometryModel {
    /// Raw entity count from the STEP DATA section.
    pub entities: HashMap<u64, String>,
 
    pub vertices: Vec<Vertex>,
    pub edges:    Vec<Edge>,
    pub faces:    Vec<Face>,
    pub shells:   Vec<Shell>,
    pub solids:   Vec<Solid>,
}
 
impl GeometryModel {
    pub fn new() -> Self { Self::default() }
 
    /// Look up a vertex by ID.
    pub fn vertex(&self, id: u64) -> Option<&Vertex> {
        self.vertices.iter().find(|v| v.id == id)
    }
 
    /// Compute the axis-aligned bounding box of all vertices.
    pub fn global_bbox(&self) -> (Point3, Point3) {
        if self.vertices.is_empty() {
            return (Point3::origin(), Point3::origin());
        }
        let mut min = Point3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut max = Point3::new(f64::MIN, f64::MIN, f64::MIN);
        for v in &self.vertices {
            let p = v.position;
            if p.x < min.x { min.x = p.x; }
            if p.y < min.y { min.y = p.y; }
            if p.z < min.z { min.z = p.z; }
            if p.x > max.x { max.x = p.x; }
            if p.y > max.y { max.y = p.y; }
            if p.z > max.z { max.z = p.z; }
        }
        (min, max)
    }
}