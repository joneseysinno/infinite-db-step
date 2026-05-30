//! Space ID constants and coordinate encoding for InfiniteDB.
//!
//! Each geometric entity type lives in its own named space. The Hilbert curve
//! maps 3D float coordinates into a 1D key, preserving spatial locality.
 
/// Space IDs — must match the registry in the InfiniteDB instance that consumes the output.
pub mod ids {
    pub const SOLIDS:        u64 = 1;
    pub const SHELLS:        u64 = 2;
    pub const FACES:         u64 = 3;
    pub const EDGES:         u64 = 4;
    pub const VERTICES:      u64 = 5;
    pub const BOUNDARY_FN:   u64 = 6;  // 6-dimensional: centroid(3) + normal/axis(3)
    pub const TOPOLOGY:      u64 = 10; // Hyperedges live here
}
 
/// Dimension count for each space.
pub mod dims {
    pub const SPATIAL_3D:    usize = 3;
    pub const BOUNDARY_6D:   usize = 6;
    pub const TOPOLOGY_2D:   usize = 2; // (from_id_hi, from_id_lo) for edge indexing
}
 
/// Normalize a floating-point world coordinate to a u32 Hilbert coordinate.
///
/// The normalization maps [-coord_max, +coord_max] -> [0, u32::MAX].
/// This preserves the sort order needed for range queries.
///
/// # Why This Matters
/// InfiniteDB's Hilbert encoder works on u32 values. Preserving sort order
/// means a 3D bounding box query translates directly to a Hilbert key range scan —
/// the entire spatial index is implicit in the key ordering.
pub fn normalize_coord(val: f64, coord_max: f64) -> u32 {
    // Clamp to [-coord_max, coord_max]
    let clamped = val.max(-coord_max).min(coord_max);
    // Map to [0.0, 1.0]
    let normalized = (clamped + coord_max) / (2.0 * coord_max);
    // Scale to u32 range
    (normalized * u32::MAX as f64) as u32
}

/// Reduce effective precision to `bits` per Hilbert dimension (1–32), clearing low bits.
pub fn quantize_bits(coord: u32, bits: u32) -> u32 {
    let bits = bits.clamp(1, 32);
    if bits >= 32 {
        return coord;
    }
    let shift = 32 - bits;
    (coord >> shift) << shift
}

/// Encode a 3D world point into a 3-element coordinate vector for InfiniteDB.
pub fn encode_point_3d(x: f64, y: f64, z: f64, coord_max: f64, bits_per_dim: u32) -> [u32; 3] {
    [
        quantize_bits(normalize_coord(x, coord_max), bits_per_dim),
        quantize_bits(normalize_coord(y, coord_max), bits_per_dim),
        quantize_bits(normalize_coord(z, coord_max), bits_per_dim),
    ]
}

/// Encode a 6D point: first 3 from centroid, next 3 from a unit direction vector.
///
/// Direction vectors are in [-1, 1] so coord_max=1.0 is appropriate for those dims.
pub fn encode_point_6d(
    cx: f64, cy: f64, cz: f64,
    nx: f64, ny: f64, nz: f64,
    coord_max: f64,
    bits_per_dim: u32,
) -> [u32; 6] {
    [
        quantize_bits(normalize_coord(cx, coord_max), bits_per_dim),
        quantize_bits(normalize_coord(cy, coord_max), bits_per_dim),
        quantize_bits(normalize_coord(cz, coord_max), bits_per_dim),
        quantize_bits(normalize_coord(nx, 1.0), bits_per_dim),
        quantize_bits(normalize_coord(ny, 1.0), bits_per_dim),
        quantize_bits(normalize_coord(nz, 1.0), bits_per_dim),
    ]
}
 
#[cfg(test)]
mod tests {
    use super::*;
 
    #[test]
    fn normalize_maps_extremes() {
        assert_eq!(normalize_coord(-1000.0, 1000.0), 0);
        assert_eq!(normalize_coord(1000.0, 1000.0), u32::MAX);
    }
 
    #[test]
    fn normalize_midpoint() {
        let mid = normalize_coord(0.0, 1000.0);
        let expected = u32::MAX / 2;
        // Allow ±1 for rounding
        assert!((mid as i64 - expected as i64).abs() <= 1);
    }
 
    #[test]
    fn quantize_bits_reduces_precision() {
        let full = u32::MAX;
        let q8 = quantize_bits(full, 8);
        assert_eq!(q8 & 0x00FF_FFFF, 0);
    }

    #[test]
    fn encode_point_3d_origin() {
        let pt = encode_point_3d(0.0, 0.0, 0.0, 1000.0, 32);
        let mid = u32::MAX / 2;
        for &c in &pt {
            assert!((c as i64 - mid as i64).abs() <= 1);
        }
    }
}