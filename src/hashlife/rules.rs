//! Rule table, 4x4 encode/decode, and the advance machinery (fast/slow/base/centered).
use crate::hashlife::{cache::*, coords::*};
use ahash::AHashMap;

pub fn build_rule_table() -> [u16; 65536] {
    let mut table = [0u16; 65536];
    for pattern in 0..=65535u16 {
        table[pattern as usize] = advance_4x4(pattern);
    }
    table
}

pub fn advance_4x4(pattern: u16) -> u16 {
    let get_bit = |bit: u32| ((pattern >> bit) & 1) as u8;
    let bit_idx = |r: u32, c: u32| 15 - (r * 4 + c);
    let mut result = 0u16;

    for dy in 0..2 {
        for dx in 0..2 {
            let cx = 1 + dx;
            let cy = 1 + dy;
            let mut neighbors = 0u8;
            for ny in 0..4 {
                for nx in 0..4 {
                    if nx == cx && ny == cy { continue; }
                    if (nx as i32 - cx as i32).abs() > 1 || (ny as i32 - cy as i32).abs() > 1 {
                        continue;
                    }
                    neighbors += get_bit(bit_idx(ny, nx));
                }
            }
            let center = get_bit(bit_idx(cy, cx));
            let next = if center == 1 {
                if neighbors == 2 || neighbors == 3 { 1 } else { 0 }
            } else {
                if neighbors == 3 { 1 } else { 0 }
            };
            let bit_pos = match (dx, dy) {
                (0, 0) => 5, (1, 0) => 4, (0, 1) => 1, (1, 1) => 0,
                _ => unreachable!(),
            };
            result |= (next as u16) << bit_pos;
        }
    }
    result
}

pub fn rule_table() -> &'static [u16; 65536] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u16; 65536]> = OnceLock::new();
    TABLE.get_or_init(|| build_rule_table())
}


pub struct LeafQuadrants {
    pub nw: u16, pub ne: u16, pub sw: u16, pub se: u16,
}

/// Inline encode_level3 — fetches all nodes in a tight loop, computes bits inline.
/// Eliminates 20 function calls per base case vs. the old encode_quadrant_* chain.
#[inline]
pub fn encode_level3(cache: &HashLifeCache, node_idx: u32) -> LeafQuadrants {
    let node = cache.get_node(node_idx);
    LeafQuadrants {
        nw: encode_level2_inline(cache, node.north_west),
        ne: encode_level2_inline(cache, node.north_east),
        sw: encode_level2_inline(cache, node.south_west),
        se: encode_level2_inline(cache, node.south_east),
    }
}

/// Inline encode_level2 — fetches 4 level-1 nodes, computes all 16 bits inline.
#[inline]
pub fn encode_level2_inline(cache: &HashLifeCache, node_idx: u32) -> u16 {
    if node_idx == FALSE_NODE { return 0; }
    if node_idx == TRUE_NODE { return 0xFFFF; }
    let l2 = cache.get_node(node_idx);
    if l2.north_west == FALSE_NODE && l2.north_east == FALSE_NODE
        && l2.south_west == FALSE_NODE && l2.south_east == FALSE_NODE {
        return 0;
    }

    // Fetch all 4 level-1 nodes in order
    let nw1 = cache.get_node(l2.north_west);
    let ne1 = cache.get_node(l2.north_east);
    let sw1 = cache.get_node(l2.south_west);
    let se1 = cache.get_node(l2.south_east);

    // Compute all 16 bits inline
    // NW quadrant bits: 15,14,11,10
    // NE quadrant bits: 13,12,9,8
    // SW quadrant bits: 7,6,3,2
    // SE quadrant bits: 5,4,1,0
    let mut bits = 0u16;
    if nw1.north_west != FALSE_NODE { bits |= 1 << 15; }
    if nw1.north_east != FALSE_NODE { bits |= 1 << 14; }
    if nw1.south_west != FALSE_NODE { bits |= 1 << 11; }
    if nw1.south_east != FALSE_NODE { bits |= 1 << 10; }
    if ne1.north_west != FALSE_NODE { bits |= 1 << 13; }
    if ne1.north_east != FALSE_NODE { bits |= 1 << 12; }
    if ne1.south_west != FALSE_NODE { bits |= 1 << 9; }
    if ne1.south_east != FALSE_NODE { bits |= 1 << 8; }
    if sw1.north_west != FALSE_NODE { bits |= 1 << 7; }
    if sw1.north_east != FALSE_NODE { bits |= 1 << 6; }
    if sw1.south_west != FALSE_NODE { bits |= 1 << 3; }
    if sw1.south_east != FALSE_NODE { bits |= 1 << 2; }
    if se1.north_west != FALSE_NODE { bits |= 1 << 5; }
    if se1.north_east != FALSE_NODE { bits |= 1 << 4; }
    if se1.south_west != FALSE_NODE { bits |= 1 << 1; }
    if se1.south_east != FALSE_NODE { bits |= 1 << 0; }
    bits
}


pub fn window_n(nw: u16, ne: u16) -> u16 {
    (((nw as u32) << 2) & 0xCCCC) as u16 | (((ne as u32) >> 2) & 0x3333) as u16
}

pub fn window_w(nw: u16, sw: u16) -> u16 {
    (((nw as u32) << 8) & 0xFF00) as u16 | (((sw as u32) >> 8) & 0x00FF) as u16
}

pub fn window_e(ne: u16, se: u16) -> u16 {
    (((ne as u32) << 8) & 0xFF00) as u16 | (((se as u32) >> 8) & 0x00FF) as u16
}

pub fn window_s(sw: u16, se: u16) -> u16 {
    (((sw as u32) << 2) & 0xCCCC) as u16 | (((se as u32) >> 2) & 0x3333) as u16
}

pub fn window_center(nw: u16, ne: u16, sw: u16, se: u16) -> u16 {
    (((nw as u32) << 10) & MASK_NW as u32) as u16
        | (((ne as u32) << 6) & MASK_NE as u32) as u16
        | (((sw as u32) >> 6) & MASK_SW as u32) as u16
        | (((se as u32) >> 10) & MASK_SE as u32) as u16
}


pub fn assemble_quadrants(r_nw: u16, r_ne: u16, r_sw: u16, r_se: u16) -> u16 {
    ((r_nw as u32) << 10) as u16 & MASK_NW
        | ((r_ne as u32) << 8) as u16 & MASK_NE
        | ((r_sw as u32) << 2) as u16 & MASK_SW
        | r_se & MASK_SE
}

pub fn assemble_centered_6x6(r_nw: u16, r_n: u16, r_ne: u16, r_w: u16, r_c: u16, r_e: u16, r_sw: u16, r_s: u16, r_se: u16) -> u16 {
    (((r_nw as u32) << 15)
        | ((r_n as u32) << 13)
        | ((r_ne as u32) << 11) & 0x1000
        | ((r_w as u32) << 7) & 0x0880
        | ((r_c as u32) << 5)
        | ((r_e as u32) << 3) & 0x0110
        | ((r_sw as u32) >> 1) & 0x0008
        | (r_s as u32) >> 3
        | (r_se as u32) >> 5) as u16
}

/// Decode 16-bit value into a level-2 node (4x4 grid).
/// Used by advance_base_one_gen (single-gen advance at level 3).
pub fn decode_level2(cache: &mut HashLifeCache, bits: u16) -> u32 {
    let bit_to_cell = |b: u16, pos: u32| -> u32 {
        if ((b >> pos) & 1) != 0 { TRUE_NODE } else { FALSE_NODE }
    };
    let q_nw = cache.find_or_create(
        bit_to_cell(bits, 15), bit_to_cell(bits, 14),
        bit_to_cell(bits, 11), bit_to_cell(bits, 10),
    );
    let q_ne = cache.find_or_create(
        bit_to_cell(bits, 13), bit_to_cell(bits, 12),
        bit_to_cell(bits, 9), bit_to_cell(bits, 8),
    );
    let q_sw = cache.find_or_create(
        bit_to_cell(bits, 7), bit_to_cell(bits, 6),
        bit_to_cell(bits, 3), bit_to_cell(bits, 2),
    );
    let q_se = cache.find_or_create(
        bit_to_cell(bits, 5), bit_to_cell(bits, 4),
        bit_to_cell(bits, 1), bit_to_cell(bits, 0),
    );
    cache.find_or_create(q_nw, q_ne, q_sw, q_se)
}


/// Build quadtree directly from packed cell list (no 2D grid allocation).
/// Partitions cells into quadrants recursively, creating leaf nodes at 8x8 blocks.
pub fn build_quadtree_from_cells(cache: &mut HashLifeCache, cells: &[u128], ox: u64, oy: u64, size: u32, depth: u32) -> u32 {
    if cells.is_empty() {
        return FALSE_NODE;
    }
    // Base case: single cell
    if size == 1 {
        let packed = coord_pack(ox, oy);
        return if cells.contains(&packed) { TRUE_NODE } else { FALSE_NODE };
    }
    // Partition into 4 quadrants
    let half = size / 2;
    let mut nw_cells = Vec::with_capacity(cells.len() / 4);
    let mut ne_cells = Vec::with_capacity(cells.len() / 4);
    let mut sw_cells = Vec::with_capacity(cells.len() / 4);
    let mut se_cells = Vec::with_capacity(cells.len() / 4);
    for &cell in cells {
        let (x, y) = coord_unpack(cell);
        match (x >= ox + half as u64, y >= oy + half as u64) {
            (false, false) => nw_cells.push(cell),
            (true, false) => ne_cells.push(cell),
            (false, true) => sw_cells.push(cell),
            (true, true) => se_cells.push(cell),
        }
    }
    let nw = build_quadtree_from_cells(cache, &nw_cells, ox, oy, half, depth - 1);
    let ne = build_quadtree_from_cells(cache, &ne_cells, ox + half as u64, oy, half, depth - 1);
    let sw = build_quadtree_from_cells(cache, &sw_cells, ox, oy + half as u64, half, depth - 1);
    let se = build_quadtree_from_cells(cache, &se_cells, ox + half as u64, oy + half as u64, half, depth - 1);
    cache.find_or_create(nw, ne, sw, se)
}

pub fn advance_base_one_gen(cache: &mut HashLifeCache, node_idx: u32) -> u32 {
    let q = encode_level3(cache, node_idx);
    let table = rule_table();
    let r_nw = table[q.nw as usize];
    let r_n = table[window_n(q.nw, q.ne) as usize];
    let r_ne = table[q.ne as usize];
    let r_w = table[window_w(q.nw, q.sw) as usize];
    let r_c = table[window_center(q.nw, q.ne, q.sw, q.se) as usize];
    let r_e = table[window_e(q.ne, q.se) as usize];
    let r_sw = table[q.sw as usize];
    let r_s = table[window_s(q.sw, q.se) as usize];
    let r_se = table[q.se as usize];

    let result_bits = assemble_centered_6x6(r_nw, r_n, r_ne, r_w, r_c, r_e, r_sw, r_s, r_se);
    decode_level2(cache, result_bits)
}

/// GOLDE-style dispatcher: choose advance_fast or advance_slow based on target_depth.
/// If `level - 2 > target_depth` → use advance_slow (recursive, drops 1 level).
/// Otherwise → use advance_fast (logarithmic, drops 1 level).
/// Both drop exactly 1 level, so they can be mixed safely in the same recursion tree.
pub fn advance_node(cache: &mut HashLifeCache, slow_cache_n: &mut AHashMap<u64, u32>,
                 slow_cache_n1: &mut AHashMap<u64, u32>,
                 node_idx: u32, level: u32, target_depth: u32,
                 hits: &mut u64, misses: &mut u64) -> u32 {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    if level - 2 > target_depth {
        advance_slow(cache, slow_cache_n, slow_cache_n1, node_idx, level, target_depth, hits, misses)
    } else {
        advance_fast(cache, node_idx, level)
    }
}

/// GOLDE AdvanceFast: classic 9-subnode centered approach.
/// Advances 2^(level-2) generations. Calls ITSELF recursively (like GOLDE).
/// The dispatcher (advance_node) routes TO this function but it recurses directly.
/// Fast cache keyed by (node_idx, level) — same canonical node at different
/// levels advances different numbers of generations.
pub fn advance_fast(cache: &mut HashLifeCache,
                 node_idx: u32, level: u32) -> u32 {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    // Fast cache lookup
    let fkey = (node_idx, level);
    if let Some(&cached) = cache.fast_cache.get(&fkey) {
        return cached;
    }

    // Base case: level 3 → advance 2 generations using 8x8 rule table
    if level == 3 {
        let result = advance_base_two_gen(cache, node_idx);
        cache.fast_cache.insert(fkey, result);
        return result;
    }

    // Recursive case: classic 9-subnode approach.
    let node = cache.get_node(node_idx);

    // Get child indices
    let nw_idx = node.north_west;
    let ne_idx = node.north_east;
    let sw_idx = node.south_west;
    let se_idx = node.south_east;

    // Fetch child nodes only for centered operations - avoid redundant lookups
    let nw_node = cache.get_node(nw_idx);
    let ne_node = cache.get_node(ne_idx);
    let sw_node = cache.get_node(sw_idx);
    let se_node = cache.get_node(se_idx);

    // Centered operations use node data directly
    let ch_nw_ne = centered_horizontal(cache, &nw_node, &ne_node);
    let cv_nw_sw = centered_vertical(cache, &nw_node, &sw_node);
    let cs_node = centered_subnode(cache, &nw_node, &ne_node, &sw_node, &se_node);
    let cv_ne_se = centered_vertical(cache, &ne_node, &se_node);
    let ch_sw_se = centered_horizontal(cache, &sw_node, &se_node);

    // Advance all 9 sub-nodes at level-(L-1) — call advance_fast directly
    // Use indices directly to avoid redundant lookups
    let n00 = advance_fast(cache, nw_idx, level - 1);
    let n01 = advance_fast(cache, ch_nw_ne, level - 1);
    let n02 = advance_fast(cache, ne_idx, level - 1);
    let n10 = advance_fast(cache, cv_nw_sw, level - 1);
    let n11 = advance_fast(cache, cs_node, level - 1);
    let n12 = advance_fast(cache, cv_ne_se, level - 1);
    let n20 = advance_fast(cache, sw_idx, level - 1);
    let n21 = advance_fast(cache, ch_sw_se, level - 1);
    let n22 = advance_fast(cache, se_idx, level - 1);

    // Build 4 windows and advance each — call advance_fast directly
    let tl = cache.find_or_create(n00, n01, n10, n11);
    let tr = cache.find_or_create(n01, n02, n11, n12);
    let bl = cache.find_or_create(n10, n11, n20, n21);
    let br = cache.find_or_create(n11, n12, n21, n22);

    let top_left = advance_fast(cache, tl, level - 1);
    let top_right = advance_fast(cache, tr, level - 1);
    let bottom_left = advance_fast(cache, bl, level - 1);
    let bottom_right = advance_fast(cache, br, level - 1);

    let result = cache.find_or_create(top_left, top_right, bottom_left, bottom_right);
    cache.fast_cache.insert(fkey, result);
    result
}

/// CenteredHorizontal: extract inner 2x2 from west+east nodes
pub fn centered_horizontal(cache: &mut HashLifeCache, west: &LifeNode, east: &LifeNode) -> u32 {
    cache.find_or_create(west.north_east, east.north_west, west.south_east, east.south_west)
}

/// CenteredVertical: extract inner 2x2 from north+south nodes
pub fn centered_vertical(cache: &mut HashLifeCache, north: &LifeNode, south: &LifeNode) -> u32 {
    cache.find_or_create(north.south_west, north.south_east, south.north_west, south.north_east)
}

/// CenteredSubNode: extract central 2x2 from 4 child nodes
pub fn centered_subnode(cache: &mut HashLifeCache, nw: &LifeNode, ne: &LifeNode, sw: &LifeNode, se: &LifeNode) -> u32 {
    cache.find_or_create(nw.south_east, ne.south_west, sw.north_east, se.north_west)
}

/// Advance 2 generations at level 3 (8x8 base case)
/// Returns a level-2 node (4x4 grid) — drops 1 level, matching GOLDE's AdvanceBase.
pub fn advance_base_two_gen(cache: &mut HashLifeCache, node_idx: u32) -> u32 {
    let q = encode_level3(cache, node_idx);
    let table = rule_table();
    let gen1 = compute_first_generation(table, &q);
    let gen2_nw = table[combine_2x2(gen1.nw, gen1.n, gen1.w, gen1.center) as usize];
    let gen2_ne = table[combine_2x2(gen1.n, gen1.ne, gen1.center, gen1.e) as usize];
    let gen2_sw = table[combine_2x2(gen1.w, gen1.center, gen1.sw, gen1.s) as usize];
    let gen2_se = table[combine_2x2(gen1.center, gen1.e, gen1.s, gen1.se) as usize];
    let result_bits = assemble_quadrants(gen2_nw, gen2_ne, gen2_sw, gen2_se);
    decode_level2(cache, result_bits)
}

pub fn compute_first_generation(table: &[u16], q: &LeafQuadrants) -> FirstGenResults {
    FirstGenResults {
        nw: table[q.nw as usize],
        n: table[window_n(q.nw, q.ne) as usize],
        ne: table[q.ne as usize],
        w: table[window_w(q.nw, q.sw) as usize],
        center: table[window_center(q.nw, q.ne, q.sw, q.se) as usize],
        e: table[window_e(q.ne, q.se) as usize],
        sw: table[q.sw as usize],
        s: table[window_s(q.sw, q.se) as usize],
        se: table[q.se as usize],
    }
}

pub struct FirstGenResults {
    nw: u16, n: u16, ne: u16, w: u16, center: u16, e: u16, sw: u16, s: u16, se: u16,
}

/// Combine four 2x2 results (in SE encoding) into a 4x4 lookup index
pub fn combine_2x2(tl: u16, tr: u16, bl: u16, br: u16) -> u16 {
    ((tl as u32) << 10 | (tr as u32) << 8 | (bl as u32) << 2 | br as u32) as u16
}

pub fn advance_slow(cache: &mut HashLifeCache, slow_cache_n: &mut AHashMap<u64, u32>,
                 slow_cache_n1: &mut AHashMap<u64, u32>,
                 node_idx: u32, level: u32, target_depth: u32,
                 hits: &mut u64, misses: &mut u64) -> u32 {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    // NOTE: advance_slow must NOT read/write fast_cache.
    // That cache is exclusively for advance_fast (multi-gen cache).
    // advance_slow only uses slow_cache (persistent, cross-step).

    // Check slow cache — packed u64 key: node_idx in high 32 bits,
    // (level << 8 | target_depth) in low 32 bits.
    // GOLDE keys slow cache by {node, m_StepAdvanceDepth}.
    let key = ((node_idx as u64) << 32) | (((level as u64) << 8) | (target_depth as u64));

    // Two-tier lookup: check n1 first, then n (promote on hit)
    if let Some(&cached) = slow_cache_n1.get(&key) {
        *hits += 1;
        return cached;
    }
    if let Some(&cached) = slow_cache_n.get(&key) {
        *hits += 1;
        slow_cache_n1.insert(key, cached); // promote to current tier
        slow_cache_n.remove(&key);         // remove from old tier
        return cached;
    }
    *misses += 1;

    // Base case: level <= 3 → advance 1 generation (or 2 if target_depth >= 1)
    if level <= 3 {
        let result = if target_depth >= 1 {
            advance_base_two_gen(cache, node_idx)
        } else {
            advance_base_one_gen(cache, node_idx)
        };
        slow_cache_n1.insert(key, result);
        return result;
    }

    // Build 8x8 windows at level-(L-1)
    let segments = fetch_segments(cache, node_idx);

    let s = |sx: usize, sy: usize| segments[sy * 8 + sx];
    let mut c2x2 = |sx: usize, sy: usize| {
        cache.find_or_create(s(sx,sy), s(sx+1,sy), s(sx,sy+1), s(sx+1,sy+1))
    };
    let c_11 = c2x2(1,1); let c_13 = c2x2(3,1); let c_15 = c2x2(5,1);
    let c_31 = c2x2(1,3); let c_33 = c2x2(3,3); let c_35 = c2x2(5,3);
    let c_51 = c2x2(1,5); let c_53 = c2x2(3,5); let c_55 = c2x2(5,5);

    let window00 = cache.find_or_create(c_11, c_13, c_31, c_33);
    let window01 = cache.find_or_create(c_13, c_15, c_33, c_35);
    let window10 = cache.find_or_create(c_31, c_33, c_51, c_53);
    let window11 = cache.find_or_create(c_33, c_35, c_53, c_55);

    // GOLDE: AdvanceSlow calls AdvanceNode (dispatcher) recursively,
    // NOT AdvanceSlow. This allows routing to AdvanceFast when level-2 <= target_depth.
    let r00 = advance_node(cache, slow_cache_n, slow_cache_n1, window00, level - 1, target_depth, hits, misses);
    let r01 = advance_node(cache, slow_cache_n, slow_cache_n1, window01, level - 1, target_depth, hits, misses);
    let r10 = advance_node(cache, slow_cache_n, slow_cache_n1, window10, level - 1, target_depth, hits, misses);
    let r11 = advance_node(cache, slow_cache_n, slow_cache_n1, window11, level - 1, target_depth, hits, misses);

    let result = cache.find_or_create(r00, r01, r10, r11);
    slow_cache_n1.insert(key, result);
    result
}

/// Fetch 64 sub-segments (8x8 grid) from a node.
/// Top-down recursive descent: visits each parent once, fans out to children.
/// ~21 node lookups vs 192 with the old per-segment approach.
pub fn fetch_segments(cache: &HashLifeCache, node_idx: u32) -> [u32; 64] {
    let mut segments = [FALSE_NODE; 64];
    fn fill(
        cache: &HashLifeCache, node_idx: u32, level: u32,
        row: usize, col: usize, step: usize, seg: &mut [u32; 64],
    ) {
        if level == 0 || node_idx == FALSE_NODE {
            for dy in 0..step {
                for dx in 0..step {
                    seg[(row + dy) * 8 + (col + dx)] = node_idx;
                }
            }
            return;
        }
        if node_idx == TRUE_NODE {
            for dy in 0..step {
                for dx in 0..step {
                    seg[(row + dy) * 8 + (col + dx)] = TRUE_NODE;
                }
            }
            return;
        }
        let node = cache.get_node(node_idx);
        let half = step / 2;
        fill(cache, node.north_west, level - 1, row, col, half, seg);
        fill(cache, node.north_east, level - 1, row, col + half, half, seg);
        fill(cache, node.south_west, level - 1, row + half, col, half, seg);
        fill(cache, node.south_east, level - 1, row + half, col + half, half, seg);
    }
    fill(cache, node_idx, 3, 0, 0, 8, &mut segments);
    segments
}

