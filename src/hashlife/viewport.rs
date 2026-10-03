//! Bitmap viewport fill (plain + aggregated) and the SSE2 TRUE-fill fast path.
use crate::hashlife::cache::*;

// Profiling hook (candidate 4, SSE2 TRUE fill): cumulative bytes written by
// the TRUE_NODE fill branch of fill_viewport, across all threads. Cheap: one
// relaxed atomic add per TRUE visit, no clock reads.
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

pub fn fill_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, bits: &mut [u8]) {
    if node_idx == FALSE_NODE { return; }
    let size = 1u32 << depth;
    // Overlap check: node [ox, ox+size) vs viewport [vx, vx+vw)
    if ox >= vx + vw as u64 || ox + size as u64 <= vx || oy >= vy + vh as u64 || oy + size as u64 <= vy {
        return; // No overlap
    }
    if node_idx == TRUE_NODE {
        let rx = ox.max(vx);
        let ry = oy.max(vy);
        let rx2 = (ox + size as u64).min(vx + vw as u64);
        let ry2 = (oy + size as u64).min(vy + vh as u64);
        // Iterate by row, then by byte: skip non-zero bytes, set zero bytes to 0xFF
        let mut tf_bytes: u64 = 0;
        for y in ry..ry2 {
            let row_base = ((y - vy) * vw as u64) as usize;
            let start_idx = row_base + (rx - vx) as usize;
            let end_idx = row_base + (rx2 - vx) as usize;
            let start_byte = start_idx >> 3;
            let end_byte = (end_idx - 1) >> 3;
            let max_byte = bits.len();
            let sb = start_byte.min(max_byte - 1);
            let eb = end_byte.min(max_byte - 1);
            if sb == eb {
                for i in start_idx..end_idx {
                    if i < max_byte * 8 {
                        bits[i >> 3] |= 1 << (i & 7);
                    }
                }
            } else {
                // Middle bytes: SSE2 16-at-a-time on x86_64, scalar remainder
                // (and the whole range on non-x86_64).
                #[cfg(target_arch = "x86_64")]
                let mid_lo = {
                    let mut b = sb + 1;
                    b = unsafe { true_fill_sse2(bits.as_mut_ptr(), b, eb) };
                    b
                };
                #[cfg(not(target_arch = "x86_64"))]
                let mid_lo = sb + 1;
                for b in mid_lo..eb {
                    if bits[b] == 0 { bits[b] = 0xFF; }
                }
                {
                    let bit = start_idx & 7;
                    let mask = (!0u8) << bit;
                    if bits[sb] == 0 { bits[sb] = mask; } else { bits[sb] |= mask; }
                }
                {
                    let bit = end_idx & 7;
                    // bit == 0: region ends exactly on a byte boundary — eb is
                    // fully covered but the middle loop stopped at eb-1, so
                    // write it here (0xFF) instead of skipping it.
                    let mask = if bit == 0 { 0xFF } else { (1u8 << bit) - 1 };
                    if bits[eb] == 0 { bits[eb] = mask; } else { bits[eb] |= mask; }
                }
            }
            tf_bytes += (eb - sb + 1) as u64;
        }
        TRUE_FILL_BYTES.fetch_add(tf_bytes, std::sync::atomic::Ordering::Relaxed);
        return;
    }
    if depth == 0 {
        // Single cell at (ox, oy)
        if ox >= vx && oy >= vy && ox < vx + vw as u64 && oy < vy + vh as u64 {
            let idx = ((oy - vy) * vw as u64 + (ox - vx)) as usize;
            let byte = idx / 8;
            if byte < bits.len() {
                bits[byte] |= 1 << (idx % 8);
            }
        }
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    fill_viewport(cache, node.north_west, depth - 1, ox, oy, vx, vy, vw, vh, bits);
    fill_viewport(cache, node.north_east, depth - 1, ox + half as u64, oy, vx, vy, vw, vh, bits);
    fill_viewport(cache, node.south_west, depth - 1, ox, oy + half as u64, vx, vy, vw, vh, bits);
    fill_viewport(cache, node.south_east, depth - 1, ox + half as u64, oy + half as u64, vx, vy, vw, vh, bits);
}

/// Like fill_viewport but writes directly to an aggregated bitmap.
/// Each raw cell (x, y) maps to aggregated pixel (aggx, aggy) = ((x-vx)/scale, (y-vy)/scale).
pub fn fill_aggregated_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, scale: u32, agg_w: u32, agg_h: u32, agg: &mut [u8]) {
    if node_idx == FALSE_NODE { return; }
    let size = 1u32 << depth;
    if ox >= vx + vw as u64 || ox + size as u64 <= vx || oy >= vy + vh as u64 || oy + size as u64 <= vy {
        return;
    }
    if node_idx == TRUE_NODE {
        let rx = ox.max(vx);
        let ry = oy.max(vy);
        let rx2 = (ox + size as u64).min(vx + vw as u64);
        let ry2 = (oy + size as u64).min(vy + vh as u64);
        // Map bounds to aggregated coordinates
        let agg_rx = (rx - vx) / scale as u64;
        let agg_ry = (ry - vy) / scale as u64;
        let agg_rx2 = (rx2 - vx + scale as u64 - 1) / scale as u64; // ceiling
        let agg_ry2 = (ry2 - vy + scale as u64 - 1) / scale as u64;
        let agg_rx2 = agg_rx2.min(agg_w as u64);
        let agg_ry2 = agg_ry2.min(agg_h as u64);
        for aggy in agg_ry..agg_ry2 {
            let row_base = (aggy * agg_w as u64) as usize;
            let start_idx = row_base + agg_rx as usize;
            let end_idx = row_base + agg_rx2 as usize;
            let start_byte = start_idx >> 3;
            let end_byte = (end_idx - 1) >> 3;
            let max_byte = agg.len();
            let sb = start_byte.min(max_byte - 1);
            let eb = end_byte.min(max_byte - 1);
            if sb == eb {
                // Single byte: set individual bits
                for i in start_idx..end_idx {
                    if i < max_byte * 8 {
                        agg[i >> 3] |= 1 << (i & 7);
                    }
                }
            } else {
                // Full bytes in the middle: skip non-zero, set zero to 0xFF
                for b in (sb + 1)..eb {
                    if agg[b] == 0 { agg[b] = 0xFF; }
                }
                // Handle start byte
                {
                    let bit = start_idx & 7;
                    let mask = (!0u8) << bit;
                    if agg[sb] == 0 { agg[sb] = mask; } else { agg[sb] |= mask; }
                }
                // Handle end byte
                {
                    let bit = end_idx & 7;
                    // bit == 0: region ends exactly on a byte boundary — eb is
                    // fully covered but the middle loop stopped at eb-1, so
                    // write it here (0xFF) instead of skipping it.
                    let mask = if bit == 0 { 0xFF } else { (1u8 << bit) - 1 };
                    if agg[eb] == 0 { agg[eb] = mask; } else { agg[eb] |= mask; }
                }
            }
        }
        return;
    }
    if depth == 0 {
        if ox >= vx && oy >= vy && ox < vx + vw as u64 && oy < vy + vh as u64 {
            let aggx = (ox - vx) / scale as u64;
            let aggy = (oy - vy) / scale as u64;
            if aggx < agg_w as u64 && aggy < agg_h as u64 {
                let aidx = (aggy * agg_w as u64 + aggx) as usize;
                if aidx < agg.len() * 8 {
                    agg[aidx >> 3] |= 1 << (aidx & 7);
                }
            }
        }
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    fill_aggregated_viewport(cache, node.north_west, depth - 1, ox, oy, vx, vy, vw, vh, scale, agg_w, agg_h, agg);
    fill_aggregated_viewport(cache, node.north_east, depth - 1, ox + half as u64, oy, vx, vy, vw, vh, scale, agg_w, agg_h, agg);
    fill_aggregated_viewport(cache, node.south_west, depth - 1, ox, oy + half as u64, vx, vy, vw, vh, scale, agg_w, agg_h, agg);
    fill_aggregated_viewport(cache, node.south_east, depth - 1, ox + half as u64, oy + half as u64, vx, vy, vw, vh, scale, agg_w, agg_h, agg);
}
