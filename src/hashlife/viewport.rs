//! Bitmap viewport fill (plain + aggregated) and the SSE2 TRUE-fill fast path.
//!
//! Readability item #5: the two near-duplicate recursive walkers share ONE
//! generic walker (`walk_fill`), monomorphized per target type: `RawTarget`
//! carries no scale field at all (the plain fill gets exactly the same code
//! as before — no divide, no branch), `AggTarget` carries the runtime scale.
//! Public entry points and behavior are unchanged.
use crate::hashlife::cache::*;

// Profiling hook (candidate 4, SSE2 TRUE fill): cumulative bytes written by
// the TRUE_NODE fill branch of fill_viewport, across all threads. Cheap: one
// relaxed atomic add per TRUE visit, no clock call.
pub static TRUE_FILL_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Take and reset the accumulated TRUE_NODE fill byte count.
pub fn take_true_fill_bytes() -> u64 {
    TRUE_FILL_BYTES.swap(0, std::sync::atomic::Ordering::Relaxed)
}

// Candidate 4: SSE2 fast path for the TRUE_NODE middle-bytes fill.
// The buffer is zeroed at call entry and nothing clears bits, so for 16-byte
// chunks `v | (v == 0)` is exact: zero lane -> 0xFF, nonzero lane -> unchanged
// — identical to the scalar `if bits[b] == 0 { bits[b] = 0xFF; }` loop.
// Processes [lo, hi) in 16-byte chunks and returns the next byte to handle.
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::{
    _mm_cmpeq_epi8, _mm_loadu_si128, _mm_or_si128, _mm_setzero_si128, _mm_storeu_si128, __m128i,
};
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn true_fill_sse2(base: *mut u8, mut lo: usize, hi: usize) -> usize {
    unsafe {
        let zero = _mm_setzero_si128();
        while hi - lo >= 16 {
            let ptr = base.add(lo) as *mut __m128i;
            let v = _mm_loadu_si128(ptr);
            let z = _mm_cmpeq_epi8(v, zero);
            _mm_storeu_si128(ptr, _mm_or_si128(v, z));
            lo += 16;
        }
    }
    lo
}

/// Where the walker paints bits. All coordinates are raw cell coords; the
/// target maps them into its own buffer (identity for raw, /scale for agg).
trait FillTarget {
    fn vx(&self) -> u64;
    fn vy(&self) -> u64;
    fn vw(&self) -> u64;
    fn vh(&self) -> u64;
    /// Paint every bit of the (viewport-clamped) rect [rx,rx2) x [ry,ry2).
    fn fill_rect(&mut self, rx: u64, ry: u64, rx2: u64, ry2: u64);
    /// Paint the single cell at (x, y), known to be inside the viewport.
    fn set_cell(&mut self, x: u64, y: u64);
}

/// The shared recursive quadtree walk. One instantiation per target type.
fn walk_fill<T: FillTarget>(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, t: &mut T) {
    if node_idx == FALSE_NODE { return; }
    let size = 1u32 << depth;
    // Overlap check: node [ox, ox+size) vs viewport [vx, vx+vw)
    if ox >= t.vx() + t.vw() || ox + size as u64 <= t.vx()
        || oy >= t.vy() + t.vh() || oy + size as u64 <= t.vy()
    {
        return; // No overlap
    }
    if node_idx == TRUE_NODE {
        let rx = ox.max(t.vx());
        let ry = oy.max(t.vy());
        let rx2 = (ox + size as u64).min(t.vx() + t.vw());
        let ry2 = (oy + size as u64).min(t.vy() + t.vh());
        t.fill_rect(rx, ry, rx2, ry2);
        return;
    }
    if depth == 0 {
        t.set_cell(ox, oy);
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    walk_fill(cache, node.north_west, depth - 1, ox, oy, t);
    walk_fill(cache, node.north_east, depth - 1, ox + half as u64, oy, t);
    walk_fill(cache, node.south_west, depth - 1, ox, oy + half as u64, t);
    walk_fill(cache, node.south_east, depth - 1, ox + half as u64, oy + half as u64, t);
}

// ---------------------------------------------------------------- raw target

struct RawTarget<'a> {
    vx: u64,
    vy: u64,
    vw: u64,
    vh: u64,
    bits: &'a mut [u8],
}

impl FillTarget for RawTarget<'_> {
    #[inline(always)] fn vx(&self) -> u64 { self.vx }
    #[inline(always)] fn vy(&self) -> u64 { self.vy }
    #[inline(always)] fn vw(&self) -> u64 { self.vw }
    #[inline(always)] fn vh(&self) -> u64 { self.vh }

    fn fill_rect(&mut self, rx: u64, ry: u64, rx2: u64, ry2: u64) {
        // Iterate by row, then by byte: skip non-zero bytes, set zero bytes to 0xFF
        let mut tf_bytes: u64 = 0;
        for y in ry..ry2 {
            let row_base = ((y - self.vy) * self.vw) as usize;
            let start_idx = row_base + (rx - self.vx) as usize;
            let end_idx = row_base + (rx2 - self.vx) as usize;
            let start_byte = start_idx >> 3;
            let end_byte = (end_idx - 1) >> 3;
            let max_byte = self.bits.len();
            let sb = start_byte.min(max_byte - 1);
            let eb = end_byte.min(max_byte - 1);
            if sb == eb {
                for i in start_idx..end_idx {
                    if i < max_byte * 8 {
                        self.bits[i >> 3] |= 1 << (i & 7);
                    }
                }
            } else {
                // Middle bytes: SSE2 16-at-a-time on x86_64, scalar remainder
                // (and the whole range on non-x86_64).
                #[cfg(target_arch = "x86_64")]
                let mid_lo = {
                    let mut b = sb + 1;
                    b = unsafe { true_fill_sse2(self.bits.as_mut_ptr(), b, eb) };
                    b
                };
                #[cfg(not(target_arch = "x86_64"))]
                let mid_lo = sb + 1;
                for b in mid_lo..eb {
                    if self.bits[b] == 0 { self.bits[b] = 0xFF; }
                }
                {
                    let bit = start_idx & 7;
                    let mask = (!0u8) << bit;
                    if self.bits[sb] == 0 { self.bits[sb] = mask; } else { self.bits[sb] |= mask; }
                }
                {
                    let bit = end_idx & 7;
                    // bit == 0: region ends exactly on a byte boundary — eb is
                    // fully covered but the middle loop stopped at eb-1, so
                    // write it here (0xFF) instead of skipping it.
                    let mask = if bit == 0 { 0xFF } else { (1u8 << bit) - 1 };
                    if self.bits[eb] == 0 { self.bits[eb] = mask; } else { self.bits[eb] |= mask; }
                }
            }
            tf_bytes += (eb - sb + 1) as u64;
        }
        TRUE_FILL_BYTES.fetch_add(tf_bytes, std::sync::atomic::Ordering::Relaxed);
    }

    fn set_cell(&mut self, x: u64, y: u64) {
        // Single cell at (x, y)
        let idx = ((y - self.vy) * self.vw + (x - self.vx)) as usize;
        let byte = idx / 8;
        if byte < self.bits.len() {
            self.bits[byte] |= 1 << (idx % 8);
        }
    }
}

pub fn fill_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, bits: &mut [u8]) {
    let mut t = RawTarget { vx, vy, vw: vw as u64, vh: vh as u64, bits };
    walk_fill(cache, node_idx, depth, ox, oy, &mut t);
}

// ------------------------------------------------------------- aggregated target

struct AggTarget<'a> {
    vx: u64,
    vy: u64,
    vw: u64,
    vh: u64,
    scale: u64,
    agg_w: u64,
    agg_h: u64,
    agg: &'a mut [u8],
}

impl FillTarget for AggTarget<'_> {
    #[inline(always)] fn vx(&self) -> u64 { self.vx }
    #[inline(always)] fn vy(&self) -> u64 { self.vy }
    #[inline(always)] fn vw(&self) -> u64 { self.vw }
    #[inline(always)] fn vh(&self) -> u64 { self.vh }

    /// Each raw cell (x, y) maps to aggregated pixel (aggx, aggy) = ((x-vx)/scale, (y-vy)/scale).
    fn fill_rect(&mut self, rx: u64, ry: u64, rx2: u64, ry2: u64) {
        // Map bounds to aggregated coordinates
        let agg_rx = (rx - self.vx) / self.scale;
        let agg_ry = (ry - self.vy) / self.scale;
        let agg_rx2 = (rx2 - self.vx + self.scale - 1) / self.scale; // ceiling
        let agg_ry2 = (ry2 - self.vy + self.scale - 1) / self.scale;
        let agg_rx2 = agg_rx2.min(self.agg_w);
        let agg_ry2 = agg_ry2.min(self.agg_h);
        for aggy in agg_ry..agg_ry2 {
            let row_base = (aggy * self.agg_w) as usize;
            let start_idx = row_base + agg_rx as usize;
            let end_idx = row_base + agg_rx2 as usize;
            let start_byte = start_idx >> 3;
            let end_byte = (end_idx - 1) >> 3;
            let max_byte = self.agg.len();
            let sb = start_byte.min(max_byte - 1);
            let eb = end_byte.min(max_byte - 1);
            if sb == eb {
                // Single byte: set individual bits
                for i in start_idx..end_idx {
                    if i < max_byte * 8 {
                        self.agg[i >> 3] |= 1 << (i & 7);
                    }
                }
            } else {
                // Full bytes in the middle: skip non-zero, set zero to 0xFF.
                // SSE2 16-at-a-time on x86_64, scalar remainder (and the whole
                // range on non-x86_64) — same shape as RawTarget::fill_rect.
                #[cfg(target_arch = "x86_64")]
                let mid_lo = {
                    let mut b = sb + 1;
                    b = unsafe { true_fill_sse2(self.agg.as_mut_ptr(), b, eb) };
                    b
                };
                #[cfg(not(target_arch = "x86_64"))]
                let mid_lo = sb + 1;
                for b in mid_lo..eb {
                    if self.agg[b] == 0 { self.agg[b] = 0xFF; }
                }
                // Handle start byte
                {
                    let bit = start_idx & 7;
                    let mask = (!0u8) << bit;
                    if self.agg[sb] == 0 { self.agg[sb] = mask; } else { self.agg[sb] |= mask; }
                }
                // Handle end byte
                {
                    let bit = end_idx & 7;
                    // bit == 0: region ends exactly on a byte boundary — eb is
                    // fully covered but the middle loop stopped at eb-1, so
                    // write it here (0xFF) instead of skipping it.
                    let mask = if bit == 0 { 0xFF } else { (1u8 << bit) - 1 };
                    if self.agg[eb] == 0 { self.agg[eb] = mask; } else { self.agg[eb] |= mask; }
                }
            }
        }
    }

    fn set_cell(&mut self, x: u64, y: u64) {
        let aggx = (x - self.vx) / self.scale;
        let aggy = (y - self.vy) / self.scale;
        if aggx < self.agg_w && aggy < self.agg_h {
            let aidx = (aggy * self.agg_w + aggx) as usize;
            if aidx < self.agg.len() * 8 {
                self.agg[aidx >> 3] |= 1 << (aidx & 7);
            }
        }
    }
}

/// Like fill_viewport but writes directly to an aggregated bitmap.
pub fn fill_aggregated_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, scale: u32, agg_w: u32, agg_h: u32, agg: &mut [u8]) {
    let mut t = AggTarget { vx, vy, vw: vw as u64, vh: vh as u64, scale: scale as u64, agg_w: agg_w as u64, agg_h: agg_h as u64, agg };
    walk_fill(cache, node_idx, depth, ox, oy, &mut t);
}
