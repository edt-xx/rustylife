#![allow(dead_code)]
//! GOLDE-style HashLife implementation.
//!
//! Core design (matching GOLDE):
//! - LifeNode: arena-indexed struct with 4 child pointers + pre-computed hash
//! - FALSE_NODE = index 0 (all children 0 = empty)
//! - TRUE_NODE = index 1 (static alive leaf, children all FALSE_NODE)
//! - Arena: bump allocator — indices never invalidate
//! - FindOrCreate: canonicalization via hash table + arena
//! - Center-based tracking (like GOLDE's m_SeedOffset), NOT origin-based
//! - 65536-entry rule table: maps 16-bit 4x4 patterns → 4-bit 2x2 center results
//! - AdvanceFast: recursive multi-gen advance (3x3 grid of overlapping sub-nodes)
//! - AdvanceSlow: recursive exact-gen advance (8x8 grid of segments)

use std::hash::{Hash, Hasher};

use crate::grid::Coord;

// ============================================================================
// Constants
// ============================================================================

/// FALSE_NODE = index 0 (all children 0 = empty)
pub const FALSE_NODE: usize = 0;

/// TRUE_NODE = index 1 (static alive leaf, children all FALSE_NODE)
pub const TRUE_NODE: usize = 1;

/// Bitmasks for extracting 2x2 quadrants from a 16-bit 4x4 grid.
const MASK_NW: u16 = 0xCC00;
const MASK_NE: u16 = 0x3300;
const MASK_SW: u16 = 0x00CC;
const MASK_SE: u16 = 0x0033;

// ============================================================================
// LifeNode
// ============================================================================

#[derive(Clone, Copy)]
pub struct LifeNode {
    pub north_west: usize,
    pub north_east: usize,
    pub south_west: usize,
    pub south_east: usize,
    pub hash: u64,
    pub is_empty: bool,
}

impl Hash for LifeNode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

impl PartialEq for LifeNode {
    fn eq(&self, other: &Self) -> bool {
        self.north_west == other.north_west
            && self.north_east == other.north_east
            && self.south_west == other.south_west
            && self.south_east == other.south_east
    }
}

impl Eq for LifeNode {}

// ============================================================================
// Arena + Cache
// ============================================================================

/// HashLife arena and canonicalization cache.
pub struct HashLifeCache {
    /// Arena of nodes. Index 0 = FALSE_NODE, index 1 = TRUE_NODE.
    pub nodes: Vec<LifeNode>,
    /// Canonicalization map: (hash, [nw,ne,sw,se]) → arena index
    node_map: ahash::AHashMap<(u64, [usize; 4]), usize>,
}

impl HashLifeCache {
    pub fn new() -> Self {
        let mut nodes = Vec::with_capacity(65536);
        let false_hash = compute_hash(0, 0, 0, 0);
        // Index 0 = FALSE_NODE
        nodes.push(LifeNode {
            north_west: 0, north_east: 0, south_west: 0, south_east: 0,
            hash: false_hash, is_empty: true,
        });
        // Index 1 = TRUE_NODE (static alive leaf, children all FALSE_NODE)
        // Use unique hash to avoid collision with FALSE_NODE
        nodes.push(LifeNode {
            north_west: 0, north_east: 0, south_west: 0, south_east: 0,
            hash: 0xFFFFFFFFFFFFFFFF, is_empty: false,
        });
        Self {
            nodes,
            node_map: ahash::AHashMap::with_capacity(65536),
        }
    }

    pub fn get_node(&self, idx: usize) -> &LifeNode {
        &self.nodes[idx]
    }

    /// Find existing canonical node or create new one in arena.
    pub fn find_or_create(&mut self, nw: usize, ne: usize, sw: usize, se: usize) -> usize {
        // Fast path: all children same → collapse
        if nw == ne && ne == sw && sw == se {
            return nw;
        }

        let hash = compute_hash(nw, ne, sw, se);
        let key = (hash, [nw, ne, sw, se]);

        if let Some(&idx) = self.node_map.get(&key) {
            return idx;
        }

        let is_empty = {
            let is_e = |i: usize| -> bool {
                if i == FALSE_NODE { true } else { self.nodes[i].is_empty }
            };
            is_e(nw) && is_e(ne) && is_e(sw) && is_e(se)
        };

        let node = LifeNode {
            north_west: nw,
            north_east: ne,
            south_west: sw,
            south_east: se,
            hash,
            is_empty,
        };
        let idx = self.nodes.len();
        self.nodes.push(node);
        self.node_map.insert(key, idx);
        idx
    }
}

// ============================================================================
// Hash computation
// ============================================================================

fn compute_hash(nw: usize, ne: usize, sw: usize, se: usize) -> u64 {
    let mut h: u64 = 0x9E3779B97F4A7C15;
    h = mix64(h.wrapping_add(nw as u64));
    h = mix64(h.wrapping_add(ne as u64));
    h = mix64(h.wrapping_add(sw as u64));
    h = mix64(h.wrapping_add(se as u64));
    h
}

fn mix64(mut z: u64) -> u64 {
    z ^= z >> 33;
    z = z.wrapping_mul(0xff51afd7ed558ccd);
    z ^= z >> 33;
    z = z.wrapping_mul(0xc4ceb9fe1a85ec53);
    z ^= z >> 33;
    z
}

// ============================================================================
// Utility
// ============================================================================

fn is_alive(cache: &HashLifeCache, idx: usize) -> bool {
    if idx == FALSE_NODE { return false; }
    if idx == TRUE_NODE { return true; }
    !cache.get_node(idx).is_empty
}

fn next_power_of2(n: usize) -> usize {
    if n <= 1 { return 1; }
    n.next_power_of_two()
}

// ============================================================================
// Rule table (GOLDE-style: 16-bit 4x4 → 4-bit 2x2 center)
// ============================================================================

fn build_rule_table() -> [u16; 65536] {
    let mut table = [0u16; 65536];
    for pattern in 0..=65535u16 {
        table[pattern as usize] = advance_4x4(pattern);
    }
    table
}

fn advance_4x4(pattern: u16) -> u16 {
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

fn rule_table() -> &'static [u16; 65536] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u16; 65536]> = OnceLock::new();
    TABLE.get_or_init(|| build_rule_table())
}

// ============================================================================
// Encoding / Decoding
// ============================================================================

struct LeafQuadrants {
    nw: u16, ne: u16, sw: u16, se: u16,
}

fn encode_quadrant_nw(cache: &HashLifeCache, q: usize) -> u16 {
    if q == TRUE_NODE { return 0xCC00; }
    if q == FALSE_NODE { return 0; }
    let node = cache.get_node(q);
    let mut bits = 0u16;
    if is_alive(cache, node.north_west) { bits |= 1 << 15; }
    if is_alive(cache, node.north_east) { bits |= 1 << 14; }
    if is_alive(cache, node.south_west) { bits |= 1 << 11; }
    if is_alive(cache, node.south_east) { bits |= 1 << 10; }
    bits
}

fn encode_quadrant_ne(cache: &HashLifeCache, q: usize) -> u16 {
    if q == TRUE_NODE { return 0x3300; }
    if q == FALSE_NODE { return 0; }
    let node = cache.get_node(q);
    let mut bits = 0u16;
    if is_alive(cache, node.north_west) { bits |= 1 << 13; }
    if is_alive(cache, node.north_east) { bits |= 1 << 12; }
    if is_alive(cache, node.south_west) { bits |= 1 << 9; }
    if is_alive(cache, node.south_east) { bits |= 1 << 8; }
    bits
}

fn encode_quadrant_sw(cache: &HashLifeCache, q: usize) -> u16 {
    if q == TRUE_NODE { return 0x00CC; }
    if q == FALSE_NODE { return 0; }
    let node = cache.get_node(q);
    let mut bits = 0u16;
    if is_alive(cache, node.north_west) { bits |= 1 << 7; }
    if is_alive(cache, node.north_east) { bits |= 1 << 6; }
    if is_alive(cache, node.south_west) { bits |= 1 << 3; }
    if is_alive(cache, node.south_east) { bits |= 1 << 2; }
    bits
}

fn encode_quadrant_se(cache: &HashLifeCache, q: usize) -> u16 {
    if q == TRUE_NODE { return 0x0033; }
    if q == FALSE_NODE { return 0; }
    let node = cache.get_node(q);
    let mut bits = 0u16;
    if is_alive(cache, node.north_west) { bits |= 1 << 5; }
    if is_alive(cache, node.north_east) { bits |= 1 << 4; }
    if is_alive(cache, node.south_west) { bits |= 1 << 1; }
    if is_alive(cache, node.south_east) { bits |= 1 << 0; }
    bits
}

// Encodes a level-2 node (4x4 grid of leaf cells) as a 16-bit value.
fn encode_level2(cache: &HashLifeCache, node_idx: usize) -> u16 {
    if node_idx == FALSE_NODE { return 0; }
    if node_idx == TRUE_NODE { return 0xFFFF; }
    let node = cache.get_node(node_idx);
    if node.north_west == FALSE_NODE && node.north_east == FALSE_NODE
        && node.south_west == FALSE_NODE && node.south_east == FALSE_NODE {
        return 0;
    }
    encode_quadrant_nw(cache, node.north_west)
        | encode_quadrant_ne(cache, node.north_east)
        | encode_quadrant_sw(cache, node.south_west)
        | encode_quadrant_se(cache, node.south_east)
}

fn encode_level3(cache: &HashLifeCache, node_idx: usize) -> LeafQuadrants {
    let node = cache.get_node(node_idx);
    LeafQuadrants {
        nw: encode_level2(cache, node.north_west),
        ne: encode_level2(cache, node.north_east),
        sw: encode_level2(cache, node.south_west),
        se: encode_level2(cache, node.south_east),
    }
}

// ============================================================================
// Window extraction — exact GOLDE bitwise operations
// ============================================================================

fn window_n(nw: u16, ne: u16) -> u16 {
    (((nw as u32) << 2) & 0xCCCC) as u16 | (((ne as u32) >> 2) & 0x3333) as u16
}

fn window_w(nw: u16, sw: u16) -> u16 {
    (((nw as u32) << 8) & 0xFF00) as u16 | (((sw as u32) >> 8) & 0x00FF) as u16
}

fn window_e(ne: u16, se: u16) -> u16 {
    (((ne as u32) << 8) & 0xFF00) as u16 | (((se as u32) >> 8) & 0x00FF) as u16
}

fn window_s(sw: u16, se: u16) -> u16 {
    (((sw as u32) << 2) & 0xCCCC) as u16 | (((se as u32) >> 2) & 0x3333) as u16
}

fn window_center(nw: u16, ne: u16, sw: u16, se: u16) -> u16 {
    (((nw as u32) << 10) & MASK_NW as u32) as u16
        | (((ne as u32) << 6) & MASK_NE as u32) as u16
        | (((sw as u32) >> 6) & MASK_SW as u32) as u16
        | (((se as u32) >> 10) & MASK_SE as u32) as u16
}

// ============================================================================
// Assembly
// ============================================================================

fn assemble_quadrants(r_nw: u16, r_ne: u16, r_sw: u16, r_se: u16) -> u16 {
    ((r_nw as u32) << 10) as u16 & MASK_NW
        | ((r_ne as u32) << 8) as u16 & MASK_NE
        | ((r_sw as u32) << 2) as u16 & MASK_SW
        | r_se & MASK_SE
}

fn assemble_centered_6x6(r_nw: u16, r_n: u16, r_ne: u16, r_w: u16, r_c: u16, r_e: u16, r_sw: u16, r_s: u16, r_se: u16) -> u16 {
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

fn decode_level2(cache: &mut HashLifeCache, bits: u16) -> usize {
    let bit_to_cell = |b: u16, pos: u32| -> usize {
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

// ============================================================================
// Advance functions
// ============================================================================

fn advance_base_one_gen(cache: &mut HashLifeCache, node_idx: usize) -> usize {
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

fn advance_fast(cache: &mut HashLifeCache, node_idx: usize, level: u32) -> usize {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if level < 3 { return node_idx; }

    // Base case: level 3 → advance 2 generations using 8x8 rule table
    if level == 3 {
        return advance_base_two_gen(cache, node_idx);
    }

    let node = *cache.get_node(node_idx);

    // Compute all 9 sub-nodes first (borrow checker)
    let ch_nw_ne = centered_horizontal(cache, node.north_west, node.north_east);
    let cv_nw_sw = centered_vertical(cache, node.north_west, node.south_west);
    let cs_node = centered_subnode(cache, node_idx);
    let cv_ne_se = centered_vertical(cache, node.north_east, node.south_east);
    let ch_sw_se = centered_horizontal(cache, node.south_west, node.south_east);

    // Advance all 9 sub-nodes
    let n00 = advance_fast(cache, node.north_west, level - 1);
    let n01 = advance_fast(cache, ch_nw_ne, level - 1);
    let n02 = advance_fast(cache, node.north_east, level - 1);
    let n10 = advance_fast(cache, cv_nw_sw, level - 1);
    let n11 = advance_fast(cache, cs_node, level - 1);
    let n12 = advance_fast(cache, cv_ne_se, level - 1);
    let n20 = advance_fast(cache, node.south_west, level - 1);
    let n21 = advance_fast(cache, ch_sw_se, level - 1);
    let n22 = advance_fast(cache, node.south_east, level - 1);

    // Build 4 windows and advance each
    let tl = cache.find_or_create(n00, n01, n10, n11);
    let tr = cache.find_or_create(n01, n02, n11, n12);
    let bl = cache.find_or_create(n10, n11, n20, n21);
    let br = cache.find_or_create(n11, n12, n21, n22);

    let topLeft = advance_fast(cache, tl, level - 1);
    let topRight = advance_fast(cache, tr, level - 1);
    let bottomLeft = advance_fast(cache, bl, level - 1);
    let bottomRight = advance_fast(cache, br, level - 1);

    cache.find_or_create(topLeft, topRight, bottomLeft, bottomRight)
}

/// CenteredHorizontal: extract inner 2x2 from west+east nodes
fn centered_horizontal(cache: &mut HashLifeCache, west: usize, east: usize) -> usize {
    let wn = cache.get_node(west);
    let en = cache.get_node(east);
    cache.find_or_create(wn.north_east, en.north_west, wn.south_east, en.south_west)
}

/// CenteredVertical: extract inner 2x2 from north+south nodes
fn centered_vertical(cache: &mut HashLifeCache, north: usize, south: usize) -> usize {
    let nn = cache.get_node(north);
    let sn = cache.get_node(south);
    cache.find_or_create(nn.south_west, nn.south_east, sn.north_west, sn.north_east)
}

/// CenteredSubNode: extract central 2x2 from node's 4 corners
fn centered_subnode(cache: &mut HashLifeCache, node_idx: usize) -> usize {
    let node = cache.get_node(node_idx);
    let nw_se = cache.get_node(node.north_west).south_east;
    let ne_sw = cache.get_node(node.north_east).south_west;
    let sw_ne = cache.get_node(node.south_west).north_east;
    let se_nw = cache.get_node(node.south_east).north_west;
    cache.find_or_create(nw_se, ne_sw, sw_ne, se_nw)
}

/// Advance 2 generations at level 3 (8x8 base case)
fn advance_base_two_gen(cache: &mut HashLifeCache, node_idx: usize) -> usize {
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

fn compute_first_generation(table: &[u16], q: &LeafQuadrants) -> FirstGenResults {
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

struct FirstGenResults {
    nw: u16, n: u16, ne: u16, w: u16, center: u16, e: u16, sw: u16, s: u16, se: u16,
}

/// Combine four 2x2 results (in SE encoding) into a 4x4 lookup index
fn combine_2x2(tl: u16, tr: u16, bl: u16, br: u16) -> u16 {
    ((tl as u32) << 10 | (tr as u32) << 8 | (bl as u32) << 2 | br as u32) as u16
}

fn advance_slow(cache: &mut HashLifeCache, node_idx: usize, level: u32) -> usize {
    if node_idx == FALSE_NODE { return FALSE_NODE; }

    // Base case: level <= 3 → advance 1 generation using 8x8 rule table
    if level <= 3 {
        return advance_base_one_gen(cache, node_idx);
    }

    // Fetch 64 sub-segments (8x8 grid) — matches GOLDE fetchSegment
    let segments = fetch_segments(cache, node_idx);

    // Build only the 9 sub-nodes needed for the 4 windows
    let s = |sx: usize, sy: usize| segments[sy * 8 + sx];
    let mut c2x2 = |sx: usize, sy: usize| {
        cache.find_or_create(s(sx,sy), s(sx+1,sy), s(sx,sy+1), s(sx+1,sy+1))
    };
    let c_11 = c2x2(1,1); let c_13 = c2x2(3,1); let c_15 = c2x2(5,1);
    let c_31 = c2x2(1,3); let c_33 = c2x2(3,3); let c_35 = c2x2(5,3);
    let c_51 = c2x2(1,5); let c_53 = c2x2(3,5); let c_55 = c2x2(5,5);

    // Build 4 windows at positions (1,1), (3,1), (1,3), (3,3) — matches GOLDE buildWindow
    let window00 = cache.find_or_create(c_11, c_13, c_31, c_33);
    let window01 = cache.find_or_create(c_13, c_15, c_33, c_35);
    let window10 = cache.find_or_create(c_31, c_33, c_51, c_53);
    let window11 = cache.find_or_create(c_33, c_35, c_53, c_55);

    // Recursively advance each window at level-1 (single-gen stepping)
    let r00 = advance_slow(cache, window00, level - 1);
    let r01 = advance_slow(cache, window01, level - 1);
    let r10 = advance_slow(cache, window10, level - 1);
    let r11 = advance_slow(cache, window11, level - 1);

    cache.find_or_create(r00, r01, r10, r11)
}

/// Fetch 64 sub-segments (8x8 grid) from a node
fn fetch_segments(cache: &HashLifeCache, node_idx: usize) -> [usize; 64] {
    let mut segments = [FALSE_NODE; 64];
    let fetch = |x: u32, y: u32| -> usize {
        let mut current = node_idx;
        for bit in (0..3).rev() {
            if current == FALSE_NODE { break; }
            let east = (x >> bit) & 1 == 1;
            let south = (y >> bit) & 1 == 1;
            let node = cache.get_node(current);
            current = if south {
                if east { node.south_east } else { node.south_west }
            } else {
                if east { node.north_east } else { node.north_west }
            };
        }
        current
    };
    for y in 0..8u32 {
        for x in 0..8u32 {
            segments[(y * 8 + x) as usize] = fetch(x, y);
        }
    }
    segments
}

// ============================================================================
// Empty tree at a given level (all FALSE_NODE)
fn empty_tree(cache: &mut HashLifeCache, level: u32) -> usize {
    if level <= 0 { return FALSE_NODE; }
    let child = empty_tree(cache, level - 1);
    cache.find_or_create(child, child, child, child)
}

// Expand node — GOLDE-style shift-up.
// Places each child in a corner with empty padding, creating a node one level deeper.
fn expand_node(cache: &mut HashLifeCache, node_idx: usize, level: u32) -> usize {
    if node_idx == FALSE_NODE {
        return empty_tree(cache, level + 1);
    }
    if node_idx == TRUE_NODE {
        return cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, TRUE_NODE);
    }
    let empty = if level > 0 { empty_tree(cache, level - 1) } else { FALSE_NODE };
    let node = *cache.get_node(node_idx);
    let expanded_nw = cache.find_or_create(empty, empty, empty, node.north_west);
    let expanded_ne = cache.find_or_create(empty, empty, node.north_east, empty);
    let expanded_sw = cache.find_or_create(empty, node.south_west, empty, empty);
    let expanded_se = cache.find_or_create(node.south_east, empty, empty, empty);
    cache.find_or_create(expanded_nw, expanded_ne, expanded_sw, expanded_se)
}

// ============================================================================
// Quadtree build / walk helpers
// ============================================================================

fn build_quadtree(cache: &mut HashLifeCache, grid: &[Vec<u8>], x0: usize, y0: usize, w: usize, h: usize) -> usize {
    if w == 0 || h == 0 { return FALSE_NODE; }
    if w == 1 && h == 1 {
        if grid[y0][x0] == 1 { TRUE_NODE } else { FALSE_NODE }
    } else {
        let hw = w / 2; let hh = h / 2;
        let nw = build_quadtree(cache, grid, x0, y0, hw, hh);
        let ne = build_quadtree(cache, grid, x0 + hw, y0, w - hw, hh);
        let sw = build_quadtree(cache, grid, x0, y0 + hh, hw, h - hh);
        let se = build_quadtree(cache, grid, x0 + hw, y0 + hh, w - hw, h - hh);
        cache.find_or_create(nw, ne, sw, se)
    }
}

fn count_cells(cache: &HashLifeCache, node_idx: usize, depth: u32) -> usize {
    if node_idx == FALSE_NODE { return 0; }
    if node_idx == TRUE_NODE { return 1usize << depth; }
    if depth == 0 { return 1; }
    let node = cache.get_node(node_idx);
    count_cells(cache, node.north_west, depth - 1)
        + count_cells(cache, node.north_east, depth - 1)
        + count_cells(cache, node.south_west, depth - 1)
        + count_cells(cache, node.south_east, depth - 1)
}

fn collect_alive(cache: &HashLifeCache, node_idx: usize, depth: u32, ox: u32, oy: u32, result: &mut Vec<u64>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        // Uniform block — fill entire region
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push(Coord::pack(ox + x, oy + y));
            }
        }
        return;
    }
    if depth == 0 {
        // Leaf cell
        result.push(Coord::pack(ox, oy));
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    collect_alive(cache, node.north_west, depth - 1, ox, oy, result);
    collect_alive(cache, node.north_east, depth - 1, ox + half, oy, result);
    collect_alive(cache, node.south_west, depth - 1, ox, oy + half, result);
    collect_alive(cache, node.south_east, depth - 1, ox + half, oy + half, result);
}

fn get_cell_at(cache: &HashLifeCache, node_idx: usize, depth: u32, x: u32, y: u32) -> bool {
    if node_idx == FALSE_NODE { return false; }
    if node_idx == TRUE_NODE { return true; }
    if depth == 0 { return true; }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    if x < half {
        if y < half {
            get_cell_at(cache, node.north_west, depth - 1, x, y)
        } else {
            get_cell_at(cache, node.south_west, depth - 1, x, y - half)
        }
    } else {
        if y < half {
            get_cell_at(cache, node.north_east, depth - 1, x - half, y)
        } else {
            get_cell_at(cache, node.south_east, depth - 1, x - half, y - half)
        }
    }
}

fn set_cell_at(cache: &mut HashLifeCache, node_idx: usize, depth: u32, x: u32, y: u32, alive: bool) -> usize {
    if depth == 0 {
        return if alive { TRUE_NODE } else { FALSE_NODE };
    }
    let node = *cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    let (nw, ne, sw, se);
    if x < half {
        if y < half {
            nw = set_cell_at(cache, node.north_west, depth - 1, x, y, alive);
            ne = node.north_east;
            sw = node.south_west;
            se = node.south_east;
        } else {
            nw = node.north_west;
            ne = node.north_east;
            sw = set_cell_at(cache, node.south_west, depth - 1, x, y - half, alive);
            se = node.south_east;
        }
    } else {
        if y < half {
            nw = node.north_west;
            ne = set_cell_at(cache, node.north_east, depth - 1, x - half, y, alive);
            sw = node.south_west;
            se = node.south_east;
        } else {
            nw = node.north_west;
            ne = node.north_east;
            sw = node.south_west;
            se = set_cell_at(cache, node.south_east, depth - 1, x - half, y - half, alive);
        }
    }
    cache.find_or_create(nw, ne, sw, se)
}

fn fill_viewport(cache: &HashLifeCache, node_idx: usize, depth: u32, ox: u32, oy: u32, vx: u32, vy: u32, vw: u32, vh: u32, bits: &mut Vec<u8>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        let size = 1u32 << depth;
        let rx = ox.max(vx);
        let ry = oy.max(vy);
        let rx2 = (ox + size).min(vx + vw);
        let ry2 = (oy + size).min(vy + vh);
        for y in ry..ry2 {
            for x in rx..rx2 {
                let idx = ((y - vy) * vw + (x - vx)) as usize;
                let byte = idx / 8;
                let bit = idx % 8;
                if byte < bits.len() {
                    bits[byte] |= 1 << bit;
                }
            }
        }
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    let rx = ox + half;
    let ry = oy + half;
    // Overlap check
    if rx < vx + vw && ox < vx + vw && ry < vy + vh && oy < vy + vh {
        fill_viewport(cache, node.north_west, depth - 1, ox, oy, vx, vy, vw, vh, bits);
        fill_viewport(cache, node.north_east, depth - 1, rx, oy, vx, vy, vw, vh, bits);
        fill_viewport(cache, node.south_west, depth - 1, ox, ry, vx, vy, vw, vh, bits);
        fill_viewport(cache, node.south_east, depth - 1, rx, ry, vx, vy, vw, vh, bits);
    }
}

// ============================================================================
// HashLife main struct
// ============================================================================

pub struct HashLife {
    cache: HashLifeCache,
    root: usize,
    center: (i64, i64),
    depth: u32,
}

impl HashLife {
    pub fn new() -> Self {
        let cache = HashLifeCache::new();
        Self {
            cache,
            root: FALSE_NODE,
            center: (0, 0),
            depth: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root == FALSE_NODE
    }

    pub fn cell_count(&self) -> usize {
        count_cells(&self.cache, self.root, self.depth)
    }

    pub fn alive_count(&self) -> usize {
        count_cells(&self.cache, self.root, self.depth)
    }

    pub fn from_flat(data: &[u64]) -> Self {
        if data.is_empty() { return Self::new(); }

        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = u32::MIN;
        let mut max_y = u32::MIN;
        for &cell in data {
            let (x, y) = Coord::unpack(cell);
            min_x = min_x.min(x); min_y = min_y.min(y);
            max_x = max_x.max(x); max_y = max_y.max(y);
        }

        let span_x = (max_x - min_x + 1) as usize;
        let span_y = (max_y - min_y + 1) as usize;
        let size = next_power_of2(span_x.max(span_y).max(1));

        // Center the pattern in the grid.
        let ox = (size - span_x) / 2;
        let oy = (size - span_y) / 2;

        let mut grid_2d = vec![vec![0u8; size]; size];
        for &cell in data {
            let (x, y) = Coord::unpack(cell);
            grid_2d[(y - min_y + oy as u32) as usize][(x - min_x + ox as u32) as usize] = 1;
        }

        let mut cache = HashLifeCache::new();
        let mut tree = build_quadtree(&mut cache, &grid_2d, 0, 0, size, size);
        let mut depth = size.trailing_zeros();

        // Expand to at least depth 3 so step() works immediately
        while depth < 3 {
            tree = expand_node(&mut cache, tree, depth);
            depth += 1;
        }

        let origin_x = min_x as i64 - (ox as i64);
        let origin_y = min_y as i64 - (oy as i64);

        Self {
            cache,
            root: tree,
            center: (origin_x + size as i64 / 2, origin_y + size as i64 / 2),
            depth,
        }
    }

    pub fn to_flat(&self) -> Vec<u64> {
        let mut result = Vec::new();
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        collect_alive(&self.cache, self.root, self.depth, origin_x as u32, origin_y as u32, &mut result);
        result
    }

    pub fn size(&self) -> usize { 1usize << self.depth }

    pub fn populate_viewport(&self, vx: u32, vy: u32, vw: u32, vh: u32, bits: &mut Vec<u8>) {
        let bits_len = (vw as usize * vh as usize + 7) / 8;
        if bits.len() < bits_len { bits.resize(bits_len, 0); }
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        fill_viewport(&self.cache, self.root, self.depth,
            origin_x as u32, origin_y as u32, vx, vy, vw, vh, bits);
    }

    pub fn get_cell(&self, x: u32, y: u32) -> bool {
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        let rel_x = x as i64 - origin_x;
        let rel_y = y as i64 - origin_y;
        let size = self.size() as i64;
        if rel_x < 0 || rel_y < 0 || rel_x >= size || rel_y >= size { return false; }
        get_cell_at(&self.cache, self.root, self.depth, rel_x as u32, rel_y as u32)
    }

    pub fn set_cell(&mut self, x: u32, y: u32, alive: bool) {
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        let rel_x = x as i64 - origin_x;
        let rel_y = y as i64 - origin_y;
        let size = self.size() as i64;
        if rel_x < 0 || rel_y < 0 || rel_x >= size || rel_y >= size { return; }
        self.root = set_cell_at(&mut self.cache, self.root, self.depth, rel_x as u32, rel_y as u32, alive);
    }

    pub fn step(&mut self) {
        if self.is_empty() { return; }
        // Expand until tree is large enough for advance_slow base case
        while needs_expansion(&self.cache, self.root, self.depth) || self.depth < 3 {
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
        // advance_slow advances exactly 1 generation, returns level-1 node
        self.root = advance_slow(&mut self.cache, self.root, self.depth);
        self.depth -= 1;
    }

    pub fn step_n(&mut self, n: u32) {
        if self.is_empty() || n == 0 { return; }
        let mut remaining = n;
        while remaining > 0 {
            let level = self.depth;
            if level == 0 {
                self.step();
                remaining -= 1;
                continue;
            }
            let max_fast = 1u64 << (level.saturating_sub(2));
            if remaining as u64 >= max_fast {
                let mut lvl = level;
                while needs_expansion(&self.cache, self.root, lvl) || lvl < 3 {
                    self.root = expand_node(&mut self.cache, self.root, lvl);
                    lvl += 1;
                }
                self.root = expand_node(&mut self.cache, self.root, lvl);
                lvl += 1;
                self.root = advance_fast(&mut self.cache, self.root, lvl);
                lvl -= 2;
                self.depth = lvl;
                remaining -= max_fast as u32;
            } else {
                self.step();
                remaining -= 1;
            }
        }
    }
}

fn needs_expansion(cache: &HashLifeCache, node_idx: usize, level: u32) -> bool {
    if node_idx == FALSE_NODE { return false; }
    if node_idx == TRUE_NODE { return false; }
    if level <= 3 { return true; }

    let not_empty = |idx: usize| -> bool {
        idx != FALSE_NODE && !cache.get_node(idx).is_empty
    };

    let node = cache.get_node(node_idx);

    // GOLDE-style rim check: for each quadrant, check rim-facing children
    // NW quadrant: rim = NW, NE, SW (inner = SE)
    let nw = node.north_west;
    if not_empty(nw) {
        let nwn = cache.get_node(nw);
        if not_empty(nwn.north_west) || not_empty(nwn.north_east) || not_empty(nwn.south_west) { return true; }
        let nw_se = nwn.south_east;
        if not_empty(nw_se) {
            let nws = cache.get_node(nw_se);
            if not_empty(nws.north_west) || not_empty(nws.north_east) || not_empty(nws.south_west) { return true; }
        }
    }

    // NE quadrant: rim = NW, NE, SE (inner = SW)
    let ne = node.north_east;
    if not_empty(ne) {
        let nen = cache.get_node(ne);
        if not_empty(nen.north_west) || not_empty(nen.north_east) || not_empty(nen.south_east) { return true; }
        let ne_sw = nen.south_west;
        if not_empty(ne_sw) {
            let nes = cache.get_node(ne_sw);
            if not_empty(nes.north_west) || not_empty(nes.north_east) || not_empty(nes.south_east) { return true; }
        }
    }

    // SW quadrant: rim = NW, SW, SE (inner = NE)
    let sw = node.south_west;
    if not_empty(sw) {
        let swn = cache.get_node(sw);
        if not_empty(swn.north_west) || not_empty(swn.south_west) || not_empty(swn.south_east) { return true; }
        let sw_ne = swn.north_east;
        if not_empty(sw_ne) {
            let swn = cache.get_node(sw_ne);
            if not_empty(swn.north_west) || not_empty(swn.south_west) || not_empty(swn.south_east) { return true; }
        }
    }

    // SE quadrant: rim = NE, SW, SE (inner = NW)
    let se = node.south_east;
    if not_empty(se) {
        let sen = cache.get_node(se);
        if not_empty(sen.north_east) || not_empty(sen.south_west) || not_empty(sen.south_east) { return true; }
        let se_nw = sen.north_west;
        if not_empty(se_nw) {
            let sen = cache.get_node(se_nw);
            if not_empty(sen.north_east) || not_empty(sen.south_west) || not_empty(sen.south_east) { return true; }
        }
    }

    false
}
