//! Coordinate packing and the three-space converters (grid / frontend / hashlife).

// Coord pack/unpack: u64 coords packed into u128
pub fn coord_pack(x: u64, y: u64) -> u128 { (y as u128) << 64 | x as u128 }
pub fn coord_unpack(cell: u128) -> (u64, u64) { ((cell & ((1u128 << 64) - 1)) as u64, (cell >> 64) as u64) }

/// Grid coordinates are offset by this amount so the "origin" sits at (2e9, 2e9).
/// Allows negative real-world coordinates without wrapping.
pub const GRID_OFFSET: u64 = 2_000_000_000;

/// Frontend uses this offset when in hashlife mode (2e15 — safe for JS f64 precision).
pub const FRONTEND_OFFSET: u64 = 2_000_000_000_000_000;

/// HashLife coordinates are offset by this amount so the "origin" sits at (9e18, 9e18).
/// Gives ~9e18 range in both positive and negative directions before wrapping.
pub const HASHLIFE_OFFSET: u64 = 9_000_000_000_000_000_000;

/// Convert grid coords (GRID_OFFSET-based) to hashlife coords (HASHLIFE_OFFSET-based).
pub fn grid_to_hashlife(x: u64, y: u64) -> (u64, u64) {
    let gx = x as i64;
    let gy = y as i64;
    let hx = gx.wrapping_sub(GRID_OFFSET as i64).wrapping_add(HASHLIFE_OFFSET as i64);
    let hy = gy.wrapping_sub(GRID_OFFSET as i64).wrapping_add(HASHLIFE_OFFSET as i64);
    (hx as u64, hy as u64)
}

/// Convert hashlife coords (HASHLIFE_OFFSET-based) to grid coords (GRID_OFFSET-based).
pub fn hashlife_to_grid(x: u64, y: u64) -> (u64, u64) {
    let hx = x as i64;
    let hy = y as i64;
    let gx = hx.wrapping_sub(HASHLIFE_OFFSET as i64).wrapping_add(GRID_OFFSET as i64);
    let gy = hy.wrapping_sub(HASHLIFE_OFFSET as i64).wrapping_add(GRID_OFFSET as i64);
    (gx as u64, gy as u64)
}

/// Convert frontend coords (FRONTEND_OFFSET-based) to hashlife coords (HASHLIFE_OFFSET-based).
pub fn frontend_to_hashlife(x: u64, y: u64) -> (u64, u64) {
    let fx = x as i64;
    let fy = y as i64;
    let hx = fx.wrapping_sub(FRONTEND_OFFSET as i64).wrapping_add(HASHLIFE_OFFSET as i64);
    let hy = fy.wrapping_sub(FRONTEND_OFFSET as i64).wrapping_add(HASHLIFE_OFFSET as i64);
    (hx as u64, hy as u64)
}

/// Convert hashlife coords (HASHLIFE_OFFSET-based) to frontend coords (FRONTEND_OFFSET-based).
pub fn hashlife_to_frontend(x: u64, y: u64) -> (u64, u64) {
    let hx = x as i64;
    let hy = y as i64;
    let fx = hx.wrapping_sub(HASHLIFE_OFFSET as i64).wrapping_add(FRONTEND_OFFSET as i64);
    let fy = hy.wrapping_sub(HASHLIFE_OFFSET as i64).wrapping_add(FRONTEND_OFFSET as i64);
    (fx as u64, fy as u64)
}

/// Pad a raw-cell dimension so that `ceil(dim / scale)` is a multiple of 8.
/// Returns the smallest `dim' >= dim` with that property. For scale == 1 this
/// just rounds `dim` up to a multiple of 8.
pub fn pad_to_agg_multiple(dim: u64, scale: u32) -> u64 {
    let s = scale as u64;
    let agg = (dim + s - 1) / s;              // ceil(dim / scale)
    let agg_padded = (agg + 7) / 8 * 8;      // round up to a multiple of 8
    let min_dim = (agg_padded - 1) * s + 1;  // smallest raw dim mapping to agg_padded
    dim.max(min_dim)
}

/// Align a viewport so the parallel 4-quadrant fill has byte-aligned seams.
///
/// The parallel fill spawns one thread per quadrant; the NW/NE and SW/SE
/// quadrants meet at `center.0` (vertical seam) and NW/SW and NE/SE meet at
/// `center.1` (horizontal seam). A seam that falls mid-byte makes two threads
/// `|=` the same byte concurrently (data race). We shift the viewport origin
/// and pad its dimensions so both seams land exactly on byte boundaries:
///   - vertical seam:   `(center.0 - hvx_a) % (8*scale) == 0`
///   - horizontal seam: `(center.1 - hvy_a) % scale == 0` AND `agg_w % 8 == 0`
///
/// The aligned viewport is a superset of the requested one (origin shifted
/// left/down by < 8*scale cells, dimensions padded), so the returned bitmap
/// covers the requested region plus a small border.
pub fn align_viewport(
    center: (i64, i64),
    hvx: u64,
    hvy: u64,
    vw: u32,
    vh: u32,
    scale: u32,
) -> (u64, u64, u32, u32) {
    let unit_x = (8 * scale) as u64; // raw cells per aggregated byte-column
    let unit_y = scale as u64;       // raw cells per aggregated row
    // Shift x so the vertical seam (center.0) is byte-aligned.
    let dx = ((hvx as i64 - center.0).rem_euclid(unit_x as i64)) as u64;
    let hvx_a = hvx - dx;
    // Shift y so the horizontal seam (center.1) row is aligned.
    let dy = ((hvy as i64 - center.1).rem_euclid(unit_y as i64)) as u64;
    let hvy_a = hvy - dy;
    // Pad dimensions so the row length is a multiple of 8 (agg units) and the
    // aligned viewport still covers the requested region (origin shifted by
    // dx/dy). x-axis needs vw+dx, y-axis needs vh+dy.
    let vw_a = pad_to_agg_multiple(vw as u64 + dx, scale) as u32;
    let vh_a = pad_to_agg_multiple(vh as u64 + dy, scale) as u32;
    (hvx_a, hvy_a, vw_a, vh_a)
}

