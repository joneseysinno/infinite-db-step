//! Boundary Function (BoundaryFn) representation.
//!
//! ## The Big Idea: Boundaries as Implicit Functions
//!
//! Traditional CAD importers store geometry as triangle meshes (STL, OBJ).
//! This loses the *mathematical* nature of the original surfaces — a cylinder
//! becomes thousands of triangles instead of the equation `x² + y² = r²`.
//!
//! We represent each surface as a **Signed Distance Function (SDF)**:
//!   f(x, y, z) → ℝ  where f < 0 inside, f = 0 on the surface, f > 0 outside
//!
//! Why is this powerful?
//! - **Ray casting**: where does a ray hit this surface? Solve f(ray(t)) = 0.
//! - **Interference checking**: two solids intersect iff some point has f_a < 0 AND f_b < 0.
//! - **Offsetting**: `f(x,y,z) - d = 0` gives the surface offset by distance d.
//! - **Blending**: `min(f_a, f_b)` is the union; `max(f_a, f_b)` is the intersection.
//! - **Query**: "is point P inside this face's region?" = evaluate the SDF.
//!
//! The SDF parameters are stored in InfiniteDB as a `BoundaryFunctionRecord`,
//! which also carries a spatial address derived from the surface's centroid +
//! normal, enabling proximity queries like "find all planar faces near point P
//! with normal pointing upward."
//!
//! ## Function Types
//!
//! Each STEP surface type maps to a canonical SDF:
//!
//! | STEP Surface       | SDF Formula                              |
//! |--------------------|------------------------------------------|
//! | PLANE              | dot(P - P0, n̂)                           |
//! | CYLINDRICAL_SURFACE| sqrt(x'² + y'²) - r  (local frame)      |
//! | SPHERICAL_SURFACE  | |P - C| - r                              |
//! | CONICAL_SURFACE    | sqrt(x'²+y'²) - (r + z'·tan(α))        |
//! | TOROIDAL_SURFACE   | sqrt((sqrt(x²+y²) - R)² + z²) - r      |
//! | B_SPLINE_SURFACE   | |P - nearest_point(P)| (approximate)    |
 
use serde::{Serialize, Deserialize};
use crate::geometry::{Point3, Dir3, SurfaceKind};
 
/// A stored boundary function record — serialized into InfiniteDB as the
/// data payload of a record in the `boundary_fn` space (Space 6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundaryFunctionRecord {
    /// The entity ID from the STEP file (e.g. face ID).
    pub entity_id: u64,
    /// The SDF type and parameters.
    pub function: BoundaryFunction,
    /// Centroid of the face/surface region (used for spatial indexing).
    pub centroid: Point3,
    /// Outward normal at centroid (used for 6D index — centroid + normal).
    pub normal: Dir3,
    /// True if this SDF is exact; false if it's an approximation.
    pub is_exact: bool,
    /// Human-readable description of the surface type.
    pub surface_type_name: String,
    /// Face area estimate in square model units.
    pub area_estimate: f64,
}
 
/// The mathematical description of a boundary surface as an SDF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BoundaryFunction {
    /// f(P) = dot(P - point_on_plane, normal)
    /// Positive on the normal side, negative on the other.
    Plane {
        /// A point known to lie on the plane.
        point_on_plane: Point3,
        /// Outward unit normal.
        normal: Dir3,
    },
 
    /// f(P) = sqrt((P-axis_pt projected onto XY of local frame)²) - radius
    /// Negative inside the cylinder, positive outside.
    Cylinder {
        /// Origin of the cylinder's local coordinate frame.
        axis_origin: Point3,
        /// Unit axis direction (the Z axis of the cylinder).
        axis_dir: Dir3,
        /// Cylinder radius.
        radius: f64,
    },
 
    /// f(P) = |P - center| - radius
    Sphere {
        center: Point3,
        radius: f64,
    },
 
    /// f(P) = sqrt(dist_to_apex²_in_XY_of_local_frame) - (radius_at_ref + z'*tan(alpha))
    Cone {
        apex: Point3,
        axis_dir: Dir3,
        /// Radius at the reference plane.
        radius_at_reference: f64,
        /// Half-angle of the cone in radians.
        semi_angle_rad: f64,
    },
 
    /// f(P) = sqrt( (sqrt(x²+y²) - major_radius)² + z² ) - minor_radius
    /// Evaluated in the torus's local frame.
    Torus {
        center: Point3,
        axis_dir: Dir3,
        major_radius: f64,
        minor_radius: f64,
    },
 
    /// Approximate SDF: distance to nearest control point minus a heuristic radius.
    /// Used for B-spline and NURBS surfaces where an exact SDF is intractable.
    /// The `control_points` can be queried to build a more precise answer at runtime.
    BSplineApprox {
        bounding_sphere_center: Point3,
        bounding_sphere_radius: f64,
        /// Flattened control point grid for more precise approximation queries.
        control_points: Vec<Point3>,
        degree_u: u32,
        degree_v: u32,
    },
 
    /// SDF unavailable — stores the raw type name for future handling.
    Unknown {
        type_name: String,
    },
}
 
impl BoundaryFunction {
    /// Evaluate the SDF at a world-space point P.
    /// Returns (signed_distance, is_approximate).
    pub fn evaluate(&self, p: Point3) -> (f64, bool) {
        match self {
            BoundaryFunction::Plane { point_on_plane, normal } => {
                let dx = p.x - point_on_plane.x;
                let dy = p.y - point_on_plane.y;
                let dz = p.z - point_on_plane.z;
                let dist = dx * normal.x + dy * normal.y + dz * normal.z;
                (dist, false)
            }
 
            BoundaryFunction::Cylinder { axis_origin, axis_dir, radius } => {
                // Project P onto the cylinder axis, then measure radial distance
                let dx = p.x - axis_origin.x;
                let dy = p.y - axis_origin.y;
                let dz = p.z - axis_origin.z;
                // Dot product with axis gives axial distance
                let axial = dx * axis_dir.x + dy * axis_dir.y + dz * axis_dir.z;
                // Subtract the axial component to get the radial vector
                let rx = dx - axial * axis_dir.x;
                let ry = dy - axial * axis_dir.y;
                let rz = dz - axial * axis_dir.z;
                let radial_dist = (rx*rx + ry*ry + rz*rz).sqrt();
                (radial_dist - radius, false)
            }
 
            BoundaryFunction::Sphere { center, radius } => {
                let dist = p.distance_to(center);
                (dist - radius, false)
            }
 
            BoundaryFunction::Cone { apex, axis_dir, radius_at_reference, semi_angle_rad } => {
                let dx = p.x - apex.x;
                let dy = p.y - apex.y;
                let dz = p.z - apex.z;
                let axial = dx * axis_dir.x + dy * axis_dir.y + dz * axis_dir.z;
                let rx = dx - axial * axis_dir.x;
                let ry = dy - axial * axis_dir.y;
                let rz = dz - axial * axis_dir.z;
                let radial = (rx*rx + ry*ry + rz*rz).sqrt();
                // Cone surface: radial = radius_at_ref + axial * tan(alpha)
                let cone_r = radius_at_reference + axial * semi_angle_rad.tan();
                (radial - cone_r, false)
            }
 
            BoundaryFunction::Torus { center, axis_dir, major_radius, minor_radius } => {
                // Transform to torus local frame
                let dx = p.x - center.x;
                let dy = p.y - center.y;
                let dz = p.z - center.z;
                // Axial component along torus axis
                let axial = dx * axis_dir.x + dy * axis_dir.y + dz * axis_dir.z;
                // Radial in the equatorial plane
                let rx = dx - axial * axis_dir.x;
                let ry = dy - axial * axis_dir.y;
                let rz = dz - axial * axis_dir.z;
                let radial = (rx*rx + ry*ry + rz*rz).sqrt();
                // Distance from point to the torus tube center ring
                let to_ring = ((radial - major_radius).powi(2) + axial.powi(2)).sqrt();
                (to_ring - minor_radius, false)
            }
 
            BoundaryFunction::BSplineApprox { bounding_sphere_center, bounding_sphere_radius, .. } => {
                // Fast approximate: distance to bounding sphere
                let dist = p.distance_to(bounding_sphere_center);
                (dist - bounding_sphere_radius, true)
            }
 
            BoundaryFunction::Unknown { .. } => (f64::NAN, true),
        }
    }
 
    /// Return true if the point P is inside this surface region (SDF < 0).
    pub fn contains(&self, p: Point3) -> bool {
        let (d, _approx) = self.evaluate(p);
        d < 0.0
    }
 
    /// A human-readable name for the function type.
    pub fn type_name(&self) -> &str {
        match self {
            BoundaryFunction::Plane { .. }       => "plane",
            BoundaryFunction::Cylinder { .. }    => "cylinder",
            BoundaryFunction::Sphere { .. }      => "sphere",
            BoundaryFunction::Cone { .. }        => "cone",
            BoundaryFunction::Torus { .. }       => "torus",
            BoundaryFunction::BSplineApprox { .. } => "bspline_approx",
            BoundaryFunction::Unknown { .. }     => "unknown",
        }
    }
}
 
/// Build a `BoundaryFunctionRecord` from a parsed face's surface description.
pub fn boundary_fn_from_surface(
    entity_id: u64,
    surface: &SurfaceKind,
    centroid: Point3,
    normal: Dir3,
    area_estimate: f64,
) -> BoundaryFunctionRecord {
    let (function, is_exact, type_name) = match surface {
        SurfaceKind::Plane { normal: n, point_on_plane } => (
            BoundaryFunction::Plane {
                point_on_plane: *point_on_plane,
                normal: *n,
            },
            true,
            "PLANE".to_string(),
        ),
 
        SurfaceKind::Cylinder { placement, radius } => (
            BoundaryFunction::Cylinder {
                axis_origin: placement.location,
                axis_dir: placement.axis,
                radius: *radius,
            },
            true,
            "CYLINDRICAL_SURFACE".to_string(),
        ),
 
        SurfaceKind::Sphere { center, radius } => (
            BoundaryFunction::Sphere {
                center: *center,
                radius: *radius,
            },
            true,
            "SPHERICAL_SURFACE".to_string(),
        ),
 
        SurfaceKind::Cone { placement, radius, semi_angle } => (
            BoundaryFunction::Cone {
                apex: placement.location,
                axis_dir: placement.axis,
                radius_at_reference: *radius,
                semi_angle_rad: *semi_angle,
            },
            true,
            "CONICAL_SURFACE".to_string(),
        ),
 
        SurfaceKind::Torus { placement, major_radius, minor_radius } => (
            BoundaryFunction::Torus {
                center: placement.location,
                axis_dir: placement.axis,
                major_radius: *major_radius,
                minor_radius: *minor_radius,
            },
            true,
            "TOROIDAL_SURFACE".to_string(),
        ),
 
        SurfaceKind::BSplineSurface {
            degree_u, degree_v, control_points,
            bounding_sphere_center, bounding_sphere_radius
        } => {
            let flat_pts: Vec<Point3> = control_points.iter().flatten().cloned().collect();
            (
                BoundaryFunction::BSplineApprox {
                    bounding_sphere_center: *bounding_sphere_center,
                    bounding_sphere_radius: *bounding_sphere_radius,
                    control_points: flat_pts,
                    degree_u: *degree_u,
                    degree_v: *degree_v,
                },
                false,
                "B_SPLINE_SURFACE".to_string(),
            )
        }
 
        SurfaceKind::Unknown { type_name } => (
            BoundaryFunction::Unknown { type_name: type_name.clone() },
            false,
            type_name.clone(),
        ),
    };
 
    BoundaryFunctionRecord {
        entity_id,
        function,
        centroid,
        normal,
        is_exact,
        surface_type_name: type_name,
        area_estimate,
    }
}
 
#[cfg(test)]
mod tests {
    use super::*;
 
    #[test]
    fn plane_sdf_above_is_positive() {
        let f = BoundaryFunction::Plane {
            point_on_plane: Point3::new(0.0, 0.0, 0.0),
            normal: Dir3::new(0.0, 0.0, 1.0),
        };
        let (d, _) = f.evaluate(Point3::new(0.0, 0.0, 5.0));
        assert!(d > 0.0, "above plane should be positive distance");
    }
 
    #[test]
    fn plane_sdf_below_is_negative() {
        let f = BoundaryFunction::Plane {
            point_on_plane: Point3::new(0.0, 0.0, 0.0),
            normal: Dir3::new(0.0, 0.0, 1.0),
        };
        let (d, _) = f.evaluate(Point3::new(0.0, 0.0, -5.0));
        assert!(d < 0.0, "below plane should be negative distance");
    }
 
    #[test]
    fn sphere_sdf_on_surface_is_zero() {
        let f = BoundaryFunction::Sphere {
            center: Point3::new(0.0, 0.0, 0.0),
            radius: 10.0,
        };
        let (d, _) = f.evaluate(Point3::new(10.0, 0.0, 0.0));
        assert!((d).abs() < 1e-10, "on sphere surface should be ~0");
    }
 
    #[test]
    fn cylinder_sdf_on_axis_is_negative() {
        let f = BoundaryFunction::Cylinder {
            axis_origin: Point3::origin(),
            axis_dir: Dir3::new(0.0, 0.0, 1.0),
            radius: 5.0,
        };
        let (d, _) = f.evaluate(Point3::new(0.0, 0.0, 10.0));
        assert!(d < 0.0, "on axis should be inside cylinder");
    }
}