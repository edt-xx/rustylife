#![allow(dead_code)]
//! GOLDE-style HashLife implementation.
//!
//! Core design (matching GOLDE):
//! - LifeNode: arena-indexed struct with 4 child pointers + pre-computed hash
//! - FALSE_NODE = index 0 (all children 0 = empty)
//! - TRUE_NODE = index 1 (static alive leaf, children all TRUE_NODE)
//! - Arena: bump allocator — indices never invalidate
//! - FindOrCreate: canonicalization via hash table + arena
//! - Center-based tracking (like GOLDE's m_SeedOffset), NOT origin-based
//! - 65536-entry rule table: maps 16-bit 4x4 patterns → 4-bit 2x2 center results
//! - AdvanceFast: recursive multi-gen advance (3x3 grid of overlapping sub-nodes)
//! - AdvanceSlow: recursive exact-gen advance (8x8 grid of segments)

use ahash::AHashMap;
use std::hash::{Hash, Hasher};

// Coord pack/unpack (inline to avoid grid dependency in lib context)
fn coord_pack(x: u32, y: u32) -> u64 { (y as u64) << 32 | x as u64 }
fn coord_unpack(cell: u64) -> (u32, u32) { ((cell & 0xFFFFFFFF) as u32, (cell >> 32) as u32) }

// ============================================================================
// Constants
// ============================================================================

/// FALSE_NODE = index 0 (all children 0 = empty)
pub const FALSE_NODE: usize = 0;

/// TRUE_NODE = index 1 (static alive leaf, children all TRUE_NODE)
pub const TRUE_NODE: usize = 1;

/// Sentinel for advance_result meaning "no cached fast result"
const NO_FAST_CACHE: usize = usize::MAX;

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
    /// GOLDE-style fast cache: cached advance result for this node.
    /// NO_FAST_CACHE means no cached result. Cleared at start of each step.
    pub advance_result: usize,
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
            hash: false_hash, is_empty: true, advance_result: NO_FAST_CACHE,
        });
        // Index 1 = TRUE_NODE (static alive leaf, children all TRUE_NODE)
        // Use unique hash to avoid collision with FALSE_NODE
        nodes.push(LifeNode {
            north_west: 1, north_east: 1, south_west: 1, south_east: 1,
            hash: 0xFFFFFFFFFFFFFFFF, is_empty: false, advance_result: NO_FAST_CACHE,
        });
        Self {
            nodes,
            node_map: ahash::AHashMap::with_capacity(65536),
        }
    }

    pub fn get_node(&self, idx: usize) -> &LifeNode {
        &self.nodes[idx]
    }

    /// Clear all advance_result fields. Called at start of each step.
    pub fn clear_advance_results(&mut self) {
        for node in self.nodes.iter_mut() {
            node.advance_result = NO_FAST_CACHE;
        }
    }

    /// Find existing canonical node or create new one in arena.
    pub fn find_or_create(&mut self, nw: usize, ne: usize, sw: usize, se: usize) -> usize {
        // Keep FALSE_NODE and TRUE_NODE as sentinels
        if nw == FALSE_NODE && ne == FALSE_NODE && sw == FALSE_NODE && se == FALSE_NODE {
            return FALSE_NODE;
        }
        if nw == TRUE_NODE && ne == TRUE_NODE && sw == TRUE_NODE && se == TRUE_NODE {
            return TRUE_NODE;
        }

        let hash = compute_hash(nw, ne, sw, se);
        let key = (hash, [nw, ne, sw, se]);

        if let Some(&idx) = self.node_map.get(&key) {
            // Verify cached node matches (debug)
            let cached = &self.nodes[idx];
            debug_assert!(cached.north_west == nw && cached.north_east == ne
                && cached.south_west == sw && cached.south_east == se,
                "CACHE CORRUPTION: key=({},{},{},{}) cached=({},{},{},{}) idx={}",
                nw, ne, sw, se, cached.north_west, cached.north_east, cached.south_west, cached.south_east, idx);
            return idx;
        }

        // Hash collision fallback: scan all nodes for exact match
        for (i, node) in self.nodes.iter().enumerate() {
            if node.hash == hash {
                if node.north_west == nw && node.north_east == ne
                    && node.south_west == sw && node.south_east == se {
                    return i;
                }
            }
        }

        let is_empty = {
            let is_e = |i: usize| -> bool {
                if i == FALSE_NODE { true }
                else if i >= self.nodes.len() {
                    // eprintln!("FIND_OR_CREATE OOB: idx={} arena_len={} children=({},{},{},{})",
                    //           i, self.nodes.len(), nw, ne, sw, se);
                    true
                } else { self.nodes[i].is_empty }
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
            advance_result: NO_FAST_CACHE,
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

/// Decode 16-bit value into a level-2 node (4x4 grid).
/// Used by advance_base_one_gen (single-gen advance at level 3).
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

/// Decode 16-bit value into a level-1 node (2x2 grid).
/// GOLDE's DecodeLevel2 does this — creates 4 leaf children from
/// the center 2x2 of the 4x4 result. Used by advance_base_two_gen.
fn decode_level1(cache: &mut HashLifeCache, bits: u16) -> usize {
    // Center 2x2 of the 4x4 result:
    // NW quadrant: bits 15,14,11,10 → SE corner is bit 10
    // NE quadrant: bits 13,12,9,8 → SW corner is bit 9
    // SW quadrant: bits 7,6,3,2 → NE corner is bit 6
    // SE quadrant: bits 5,4,1,0 → NW corner is bit 5
    cache.find_or_create(
        if ((bits >> 10) & 1) != 0 { TRUE_NODE } else { FALSE_NODE },
        if ((bits >> 9) & 1) != 0 { TRUE_NODE } else { FALSE_NODE },
        if ((bits >> 6) & 1) != 0 { TRUE_NODE } else { FALSE_NODE },
        if ((bits >> 5) & 1) != 0 { TRUE_NODE } else { FALSE_NODE },
    )
}

// ============================================================================
// Advance functions
// ============================================================================

/// Ensure a node is at exactly level 1 (2x2 of leaf cells)
fn ensure_level1(cache: &mut HashLifeCache, idx: usize) -> usize {
    if idx == FALSE_NODE {
        cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, FALSE_NODE)
    } else if idx == TRUE_NODE {
        cache.find_or_create(TRUE_NODE, TRUE_NODE, TRUE_NODE, TRUE_NODE)
    } else {
        idx // already a proper level-1 node
    }
}

/// Ensure a node is at exactly level 2 (4x4 grid)
fn ensure_level2(cache: &mut HashLifeCache, idx: usize) -> usize {
    if idx == FALSE_NODE {
        let l1 = cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, FALSE_NODE);
        cache.find_or_create(l1, l1, l1, l1)
    } else if idx == TRUE_NODE {
        let l1 = cache.find_or_create(TRUE_NODE, TRUE_NODE, TRUE_NODE, TRUE_NODE);
        cache.find_or_create(l1, l1, l1, l1)
    } else {
        // Check if grandchildren are at level 1
        let c = *cache.get_node(idx);
        let nw = ensure_level1(cache, c.north_west);
        let ne = ensure_level1(cache, c.north_east);
        let sw = ensure_level1(cache, c.south_west);
        let se = ensure_level1(cache, c.south_east);
        cache.find_or_create(nw, ne, sw, se)
    }
}

/// Ensure a node is at exactly level 3 (8x8 grid) by expanding collapsed children.
/// When find_or_create collapses identical children, a node that should be
/// at level 3 may actually be at a lower level. This function rebuilds the
/// node to ensure proper level-3 structure.
fn ensure_level3(cache: &mut HashLifeCache, node_idx: usize) -> usize {
    if node_idx == FALSE_NODE {
        let l1 = cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, FALSE_NODE);
        let l2 = cache.find_or_create(l1, l1, l1, l1);
        return cache.find_or_create(l2, l2, l2, l2);
    }
    if node_idx == TRUE_NODE {
        let l1 = cache.find_or_create(TRUE_NODE, TRUE_NODE, TRUE_NODE, TRUE_NODE);
        let l2 = cache.find_or_create(l1, l1, l1, l1);
        return cache.find_or_create(l2, l2, l2, l2);
    }

    let node = *cache.get_node(node_idx);
    let nw = ensure_level2(cache, node.north_west);
    let ne = ensure_level2(cache, node.north_east);
    let sw = ensure_level2(cache, node.south_west);
    let se = ensure_level2(cache, node.south_east);
    cache.find_or_create(nw, ne, sw, se)
}

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

/// GOLDE-style dispatcher: choose advance_fast or advance_slow based on target_depth.
/// If `level - 2 > target_depth` → use advance_slow (recursive, drops 1 level).
/// Otherwise → use advance_fast (logarithmic, drops 1 level).
/// Both drop exactly 1 level, so they can be mixed safely in the same recursion tree.
fn advance_node(cache: &mut HashLifeCache, slow_cache: &mut AHashMap<u64, usize>,
                 node_idx: usize, level: u32, target_depth: u32,
                 hits: &mut u64, misses: &mut u64) -> usize {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    if level - 2 > target_depth {
        advance_slow(cache, slow_cache, node_idx, level, target_depth, hits, misses)
    } else {
        advance_fast(cache, slow_cache, node_idx, level)
    }
}

/// GOLDE AdvanceFast: classic 9-subnode centered approach.
/// Advances 2^(level-2) generations. Calls ITSELF recursively (like GOLDE).
/// The dispatcher (advance_node) routes TO this function but it recurses directly.
fn advance_fast(cache: &mut HashLifeCache, _slow_cache: &mut AHashMap<u64, usize>,
                 node_idx: usize, level: u32) -> usize {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    // GOLDE-style fast cache: check advance_result on the node itself.
    let node = cache.get_node(node_idx);
    if node.advance_result != NO_FAST_CACHE {
        return node.advance_result;
    }

    // Base case: level 3 → advance 2 generations using 8x8 rule table
    if level == 3 {
        let result = advance_base_two_gen(cache, node_idx);
        cache.nodes[node_idx].advance_result = result;
        return result;
    }

    // Recursive case: classic 9-subnode approach.
    let node = *cache.get_node(node_idx);

    let ch_nw_ne = centered_horizontal(cache, node.north_west, node.north_east);
    let cv_nw_sw = centered_vertical(cache, node.north_west, node.south_west);
    let cs_node = centered_subnode(cache, node_idx);
    let cv_ne_se = centered_vertical(cache, node.north_east, node.south_east);
    let ch_sw_se = centered_horizontal(cache, node.south_west, node.south_east);

    // Advance all 9 sub-nodes at level-(L-1) — call advance_fast directly
    let n00 = advance_fast(cache, &mut AHashMap::new(), node.north_west, level - 1);
    let n01 = advance_fast(cache, &mut AHashMap::new(), ch_nw_ne, level - 1);
    let n02 = advance_fast(cache, &mut AHashMap::new(), node.north_east, level - 1);
    let n10 = advance_fast(cache, &mut AHashMap::new(), cv_nw_sw, level - 1);
    let n11 = advance_fast(cache, &mut AHashMap::new(), cs_node, level - 1);
    let n12 = advance_fast(cache, &mut AHashMap::new(), cv_ne_se, level - 1);
    let n20 = advance_fast(cache, &mut AHashMap::new(), node.south_west, level - 1);
    let n21 = advance_fast(cache, &mut AHashMap::new(), ch_sw_se, level - 1);
    let n22 = advance_fast(cache, &mut AHashMap::new(), node.south_east, level - 1);

    // Build 4 windows and advance each — call advance_fast directly
    let tl = cache.find_or_create(n00, n01, n10, n11);
    let tr = cache.find_or_create(n01, n02, n11, n12);
    let bl = cache.find_or_create(n10, n11, n20, n21);
    let br = cache.find_or_create(n11, n12, n21, n22);

    let topLeft = advance_fast(cache, &mut AHashMap::new(), tl, level - 1);
    let topRight = advance_fast(cache, &mut AHashMap::new(), tr, level - 1);
    let bottomLeft = advance_fast(cache, &mut AHashMap::new(), bl, level - 1);
    let bottomRight = advance_fast(cache, &mut AHashMap::new(), br, level - 1);

    let result = cache.find_or_create(topLeft, topRight, bottomLeft, bottomRight);
    cache.nodes[node_idx].advance_result = result;
    result
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
/// Returns a level-2 node (4x4 grid) — drops 1 level, matching GOLDE's AdvanceBase.
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

fn advance_slow(cache: &mut HashLifeCache, slow_cache: &mut AHashMap<u64, usize>,
                 node_idx: usize, level: u32, target_depth: u32,
                 hits: &mut u64, misses: &mut u64) -> usize {
    if node_idx == FALSE_NODE { return FALSE_NODE; }
    if node_idx == TRUE_NODE { return TRUE_NODE; }
    if level < 3 { return node_idx; }

    // NOTE: advance_slow must NOT read/write advance_result.
    // That field is exclusively for advance_fast (multi-gen cache).
    // advance_slow only uses slow_cache (persistent, cross-step).

    // Check slow cache — packed u64 key: node_idx in high 32 bits,
    // (level << 8 | target_depth) in low 32 bits.
    // GOLDE keys slow cache by {node, m_StepAdvanceDepth}.
    let key = ((node_idx as u64) << 32) | (((level as u64) << 8) | (target_depth as u64));
    if let Some(&cached) = slow_cache.get(&key) {
        *hits += 1;
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
        slow_cache.insert(key, result);
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
    let r00 = advance_node(cache, slow_cache, window00, level - 1, target_depth, hits, misses);
    let r01 = advance_node(cache, slow_cache, window01, level - 1, target_depth, hits, misses);
    let r10 = advance_node(cache, slow_cache, window10, level - 1, target_depth, hits, misses);
    let r11 = advance_node(cache, slow_cache, window11, level - 1, target_depth, hits, misses);

    let result = cache.find_or_create(r00, r01, r10, r11);
    slow_cache.insert(key, result);
    result
}

/// Fetch 64 sub-segments (8x8 grid) from a node
fn fetch_segments(cache: &HashLifeCache, node_idx: usize) -> [usize; 64] {
    let mut segments = [FALSE_NODE; 64];
    let fetch = |x: u32, y: u32| -> usize {
        let mut current = node_idx;
        for bit in (0..3).rev() {
            if current == FALSE_NODE { break; }
            if current == TRUE_NODE { break; }
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

/// Collect alive cells from a node (relative coordinates, no packing)
fn collect_alive_helper(cache: &HashLifeCache, node_idx: usize, depth: u32) -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    collect_alive_rel(cache, node_idx, depth, 0, 0, &mut result);
    result
}

fn collect_alive_rel(cache: &HashLifeCache, node_idx: usize, depth: u32, ox: u32, oy: u32, result: &mut Vec<(u32, u32)>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push((ox + x, oy + y));
            }
        }
        return;
    }
    if depth == 0 {
        result.push((ox, oy));
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    collect_alive_rel(cache, node.north_west, depth - 1, ox, oy, result);
    collect_alive_rel(cache, node.north_east, depth - 1, ox + half, oy, result);
    collect_alive_rel(cache, node.south_west, depth - 1, ox, oy + half, result);
    collect_alive_rel(cache, node.south_east, depth - 1, ox + half, oy + half, result);
}

/// Rebuild tree in new cache by walking old tree
fn rebuild_tree(new_cache: &mut HashLifeCache, old_idx: usize, old_cache: &HashLifeCache) -> usize {
    if old_idx == FALSE_NODE { return FALSE_NODE; }
    if old_idx == TRUE_NODE { return TRUE_NODE; }
    let old_node = old_cache.get_node(old_idx);
    let nw = rebuild_tree(new_cache, old_node.north_west, old_cache);
    let ne = rebuild_tree(new_cache, old_node.north_east, old_cache);
    let sw = rebuild_tree(new_cache, old_node.south_west, old_cache);
    let se = rebuild_tree(new_cache, old_node.south_east, old_cache);
    new_cache.find_or_create(nw, ne, sw, se)
}

fn collect_alive(cache: &HashLifeCache, node_idx: usize, depth: u32, ox: u32, oy: u32, result: &mut Vec<u64>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        // Uniform block — fill entire region
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push(coord_pack(ox + x, oy + y));
            }
        }
        return;
    }
    if depth == 0 {
        // Leaf cell
        result.push(coord_pack(ox, oy));
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
    let size = 1u32 << depth;
    // Overlap check: node [ox, ox+size) vs viewport [vx, vx+vw)
    if ox >= vx + vw || ox + size <= vx || oy >= vy + vh || oy + size <= vy {
        return; // No overlap
    }
    if node_idx == TRUE_NODE {
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
    if depth == 0 {
        // Single cell at (ox, oy)
        if ox >= vx && oy >= vy && ox < vx + vw && oy < vy + vh {
            let idx = ((oy - vy) * vw + (ox - vx)) as usize;
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
    fill_viewport(cache, node.north_east, depth - 1, ox + half, oy, vx, vy, vw, vh, bits);
    fill_viewport(cache, node.south_west, depth - 1, ox, oy + half, vx, vy, vw, vh, bits);
    fill_viewport(cache, node.south_east, depth - 1, ox + half, oy + half, vx, vy, vw, vh, bits);
}

// ============================================================================
// HashLife main struct
// ============================================================================

pub struct HashLife {
    pub cache: HashLifeCache,
    root: usize,
    center: (i64, i64),
    depth: u32,
    /// GOLDE-style slow cache: (node_idx, level, advance_depth) → result_node
    /// Persists across step_n iterations to avoid recomputing overlapping sub-trees.
    pub slow_cache: AHashMap<u64, usize>,
    /// Cache hit/miss counters (reset each step)
    pub slow_cache_hits: u64,
    pub slow_cache_misses: u64,
    /// Last step's cache stats for display
    pub last_cache_size: u32,
    /// Last step's cache hit rate * 10 (e.g., 45.2% → 452)
    pub last_cache_hit_rate: u32,
}

impl HashLife {
    pub fn new() -> Self {
        let cache = HashLifeCache::new();
        Self {
            cache,
            root: FALSE_NODE,
            center: (0, 0),
            depth: 0,
            slow_cache: AHashMap::new(),
            slow_cache_hits: 0,
            slow_cache_misses: 0,
            last_cache_size: 0,
            last_cache_hit_rate: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root == FALSE_NODE
    }

    pub fn center(&self) -> (i64, i64) {
        self.center
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
            let (x, y) = coord_unpack(cell);
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
            let (x, y) = coord_unpack(cell);
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
            slow_cache: AHashMap::new(),
            slow_cache_hits: 0,
            slow_cache_misses: 0,
            last_cache_size: 0,
            last_cache_hit_rate: 0,
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
        // Arena is append-only — nodes are immutable, no rebuild needed.
        // Slow_cache persists across steps for GOLDE-style performance.
        // Clear fast cache (advance_result) at start of each step.
        self.cache.clear_advance_results();

        // Expand until tree is large enough for advance_slow base case
        while needs_expansion(&self.cache, self.root, self.depth) || self.depth < 3 {
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
        // Clear fast cache — advance_result is only valid within a single step
        self.cache.clear_advance_results();
        // advance_node with target_depth=0 → always uses advance_slow (single-gen)
        // eprintln!("STEP arena_len={} slow_cache_size={} root={} depth={}",
        //           self.cache.nodes.len(), self.slow_cache.len(), self.root, self.depth);
        self.root = advance_node(&mut self.cache, &mut self.slow_cache, self.root, self.depth, 0,
                                  &mut self.slow_cache_hits, &mut self.slow_cache_misses);
        self.depth -= 1;
        // eprintln!("POST-STEP arena_len={} root={} depth={}",
        //           self.cache.nodes.len(), self.root, self.depth);
        // Store cache stats for display
        let total = self.slow_cache_hits + self.slow_cache_misses;
        if total > 0 {
            self.last_cache_size = self.slow_cache.len() as u32;
            self.last_cache_hit_rate = ((self.slow_cache_hits as f64 / total as f64) * 1000.0).round() as u32;
        } else {
            self.last_cache_size = 0;
            self.last_cache_hit_rate = 0;
        }
        self.slow_cache_hits = 0;
        self.slow_cache_misses = 0;
        // After shrinking, pattern may touch rim — expand again if needed
        while needs_expansion(&self.cache, self.root, self.depth) {
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
    }

    pub fn step_n(&mut self, n: u32) {
        if self.is_empty() || n == 0 { return; }
        // eprintln!("STEP_N requested={} n={} depth={}", self.alive_count(), n, self.depth);

        // GOLDE-style multi-gen advance using AdvanceNode dispatcher.
        // AdvanceFast drops 1 level per call, advancing 2^(level-2) generations.
        // GOLDE: expand tree, call AdvanceNode, result is at depth-1.
        // Falls back to single-gen steps for the remainder.
        let mut remaining = n;

        while remaining > 0 {
            // Find largest power of 2 that fits in remaining: 2^k <= remaining
            let k = remaining.ilog2();
            let advance_gens = 1u32 << k;

            // If advance_gens is too small (1 gen), just use single-gen steps for remainder
            if advance_gens < 2 {
                break;
            }

            // GOLDE: expand until depth - 2 >= k (m_StepAdvanceDepth)
            // We need depth >= k + 2 for advance_fast to work at root.
            let needed_depth = k + 2;
            while self.depth < needed_depth {
                self.root = expand_node(&mut self.cache, self.root, self.depth);
                self.depth += 1;
            }
            // Also expand for pattern boundary
            while needs_expansion(&self.cache, self.root, self.depth) {
                self.root = expand_node(&mut self.cache, self.root, self.depth);
                self.depth += 1;
            }

            // Clear fast cache for this multi-gen advance
            self.cache.clear_advance_results();

            let _cells_before = self.alive_count();
            let target_depth = k;
            // eprintln!("MULTI-GEN(advance_node): depth={} root={} cells={} advancing {} gens target_depth={} (remaining={})",
            //           self.depth, self.root, _cells_before, advance_gens, target_depth, remaining);

            self.root = advance_node(&mut self.cache, &mut self.slow_cache,
                                      self.root, self.depth, target_depth,
                                      &mut self.slow_cache_hits, &mut self.slow_cache_misses);
            // GOLDE always drops 1 level per DoOneJump, regardless of advance depth.
            // advance_fast returns a node at (level - 1), not (level - 2).
            self.depth -= 1;

            let _cells_after = self.alive_count();
            // eprintln!("MULTI-GEN(advance_node) AFTER: depth={} root={} cells={} (delta={:+})",
            //           self.depth, self.root, _cells_after, _cells_after as i64 - _cells_before as i64);

            remaining -= advance_gens;

            // Store cache stats
            let total = self.slow_cache_hits + self.slow_cache_misses;
            if total > 0 {
                self.last_cache_size = self.slow_cache.len() as u32;
                self.last_cache_hit_rate = ((self.slow_cache_hits as f64 / total as f64) * 1000.0).round() as u32;
            }
            self.slow_cache_hits = 0;
            self.slow_cache_misses = 0;

            // Expand after shrinking
            while needs_expansion(&self.cache, self.root, self.depth) {
                self.root = expand_node(&mut self.cache, self.root, self.depth);
                self.depth += 1;
            }
        }

        // Fall back to single-gen steps for remainder
        // Clear fast cache — advance_result from multi-gen advances are stale
        self.cache.clear_advance_results();
        for _ in 0..remaining {
            self.step();
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

// ============================================================================
// Test: compare HashLife against flat-grid stepping
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use ahash::AHashMap;

    // Inline Coord for test context — must match module-level coord_pack/coord_unpack
    fn pack(x: u32, y: u32) -> u64 { (y as u64) << 32 | x as u64 }
    fn unpack(cell: u64) -> (u32, u32) { ((cell & 0xFFFFFFFF) as u32, (cell >> 32) as u32) }

    fn step_flat(alive: &HashSet<u64>) -> HashSet<u64> {
        let mut counts = AHashMap::new();
        for &cell in alive {
            let (x, y) = unpack(cell);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = x.wrapping_add(dx as u32);
                    let ny = y.wrapping_add(dy as u32);
                    let key = pack(nx, ny);
                    *counts.entry(key).or_insert(0u32) += 1;
                }
            }
        }
        let mut next = HashSet::new();
        for &cell in alive {
            let c = counts.get(&cell).copied().unwrap_or(0);
            if c == 2 || c == 3 { next.insert(cell); }
        }
        for (&cell, &c) in &counts {
            if !alive.contains(&cell) && c == 3 {
                next.insert(cell);
            }
        }
        next
    }

    fn test_pattern(name: &str, pattern: &[(u32, u32)], gens: usize) {
        let flat_initial: HashSet<u64> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();
        let mut flat: HashSet<u64> = flat_initial.clone();
        let mut hf = HashLife::from_flat(&flat_initial.iter().copied().collect::<Vec<_>>());

        for i in 0..gens {
            // Check to_flat matches BEFORE stepping
            let hf_before: HashSet<u64> = hf.to_flat().into_iter().collect();
            if flat != hf_before {
                let only_flat: Vec<_> = flat.difference(&hf_before).collect();
                let only_hf: Vec<_> = hf_before.difference(&flat).collect();
                if !only_flat.is_empty() || !only_hf.is_empty() {
                    panic!(
                        "{} TREE MISMATCH at gen {} (before step): flat={} hf={}\n\
                         flat_only({}): {:?}\n\
                         hf_only({}): {:?}\n\
                         depth={} center={:?}",
                        name, i, flat.len(), hf_before.len(),
                        only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                        only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                        hf.depth, hf.center,
                    );
                }
            }
            let flat_next = step_flat(&flat);
            hf.step();
            let hf_result: HashSet<u64> = hf.to_flat().into_iter().collect();
            if flat_next != hf_result {
                let only_flat: Vec<_> = flat_next.difference(&hf_result).collect();
                let only_hf: Vec<_> = hf_result.difference(&flat_next).collect();

                panic!(
                    "{} STEP MISMATCH gen {}->{}: flat={} hf={}\n\
                     depth={} center={:?} nodes={}\n\
                     flat_only({}): {:?}\n\
                     hf_only({}): {:?}",
                    name, i, i+1, flat_next.len(), hf_result.len(),
                    hf.depth, hf.center, hf.cache.nodes.len(),
                    only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                    only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>()
                );
            }
            println!("{} gen {} OK ({} cells) depth={} nodes={}",
                name, i+1, flat_next.len(), hf.depth, hf.cache.nodes.len());
            flat = flat_next;
        }
    }

    #[test]
    fn test_blinker() {
        test_pattern("blinker", &[(100,100), (100,101), (100,102)], 10);
    }

    #[test]
    fn test_glider() {
        test_pattern("glider", &[(100,100), (101,101), (99,102), (100,102), (101,102)], 20);
    }

    #[test]
    fn test_pi_heptamino() {
        test_pattern("pi_heptamino", &[
            (100,100), (101,100), (102,100),
            (100,101),
            (100,102), (101,102), (102,102),
        ], 30);
    }

    /// Test advance_slow at specific levels by building a tree of known depth
    /// and comparing the advance result with flat stepping.
    #[test]
    fn test_advance_slow_level4() {
        // Create a 16x16 pattern (level 4) centered at (8,8)
        // Place cells that will produce interesting births/deaths
        let pattern_cells: Vec<(u32, u32)> = vec![
            // Pi heptamino in center of 16x16
            (6,6), (7,6), (8,6),
            (6,7),
            (6,8), (7,8), (8,8),
        ];

        // Build flat grid
        let mut flat: HashSet<u64> = HashSet::new();
        for &(x, y) in &pattern_cells {
            flat.insert(pack(x, y));
        }

        // Step flat
        let flat_next = step_flat(&flat);

        // Build HashLife tree at depth 4 (16x16)
        let mut cache = HashLifeCache::new();
        let mut grid_2d = vec![vec![0u8; 16]; 16];
        for &(x, y) in &pattern_cells {
            grid_2d[y as usize][x as usize] = 1;
        }
        let tree = build_quadtree(&mut cache, &grid_2d, 0, 0, 16, 16);

        // Advance with advance_slow at level 4
        let result = advance_node(&mut cache, &mut AHashMap::new(), tree, 4, 0, &mut 0, &mut 0);

        // Result should be a level-3 node (8x8), centered
        // The center 8x8 covers cells (4,4) to (11,11) in the 16x16 grid
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 3, 4, 4, &mut hf_result);
        let hf_set: HashSet<u64> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        // Compare: flat_next should match hf_set for the center 8x8 region
        let flat_center: HashSet<u64> = flat_next.iter()
            .filter(|&&c| {
                let (x, y) = unpack(c);
                x >= 4 && x < 12 && y >= 4 && y < 12
            })
            .copied()
            .collect();

        if flat_center != hf_set {
            let only_flat: Vec<_> = flat_center.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_center).collect();
            panic!(
                "advance_slow level 4 MISMATCH:\n\
                 flat_center({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only: {:?}\n\
                 hf_only: {:?}",
                flat_center.len(), flat_center.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
            );
        } else {
            println!("advance_slow level 4 OK: {} cells match", hf_set.len());
        }
    }

    /// Test advance_slow at level 5 (32x32)
    #[test]
    fn test_advance_slow_level5() {
        let pattern_cells: Vec<(u32, u32)> = vec![
            (14,14), (15,14), (16,14),
            (14,15),
            (14,16), (15,16), (16,16),
        ];

        let mut flat: HashSet<u64> = HashSet::new();
        for &(x, y) in &pattern_cells {
            flat.insert(pack(x, y));
        }
        let flat_next = step_flat(&flat);

        let mut cache = HashLifeCache::new();
        let mut grid_2d = vec![vec![0u8; 32]; 32];
        for &(x, y) in &pattern_cells {
            grid_2d[y as usize][x as usize] = 1;
        }
        let tree = build_quadtree(&mut cache, &grid_2d, 0, 0, 32, 32);

        let result = advance_node(&mut cache, &mut AHashMap::new(), tree, 5, 0, &mut 0, &mut 0);

        // Result is level-4 (16x16), centered at (8,8) of the 32x32
        // Covers cells (8,8) to (23,23)
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 4, 8, 8, &mut hf_result);
        let hf_set: HashSet<u64> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        let flat_center: HashSet<u64> = flat_next.iter()
            .filter(|&&c| {
                let (x, y) = unpack(c);
                x >= 8 && x < 24 && y >= 8 && y < 24
            })
            .copied()
            .collect();

        if flat_center != hf_set {
            let only_flat: Vec<_> = flat_center.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_center).collect();
            panic!(
                "advance_slow level 5 MISMATCH:\n\
                 flat_center({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only: {:?}\n\
                 hf_only: {:?}",
                flat_center.len(), flat_center.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
            );
        } else {
            println!("advance_slow level 5 OK: {} cells match", hf_set.len());
        }
    }

    /// Test advance_slow at level 7 (128x128) with pi heptamino stepped to gen 22
    #[test]
    fn test_advance_slow_level7_gen22() {
        // Step pi heptamino 22 times using flat
        let initial: HashSet<u64> = [
            pack(100,100), pack(101,100), pack(102,100),
            pack(100,101),
            pack(100,102), pack(101,102), pack(102,102),
        ].into_iter().collect();
        let mut flat = initial.clone();
        for _ in 0..22 {
            flat = step_flat(&flat);
        }
        // flat is now gen 22
        let flat_next = step_flat(&flat);

        // Build HashLife from gen 22 state
        let hf = HashLife::from_flat(&flat.iter().copied().collect::<Vec<_>>());
        
        // Expand to ensure tree is big enough
        let mut cache = hf.cache;
        let mut root = hf.root;
        let mut depth = hf.depth;
        while needs_expansion(&cache, root, depth) || depth < 3 {
            root = expand_node(&mut cache, root, depth);
            depth += 1;
        }

        // Advance with advance_slow
        let result = advance_node(&mut cache, &mut AHashMap::new(), root, depth, 0, &mut 0, &mut 0);

        // Result is at level depth-1
        let result_depth = depth - 1;
        let result_size = 1u32 << result_depth;
        
        // The result covers the center of the expanded tree
        let tree_size = 1u32 << depth;
        let offset = (tree_size - result_size) / 2;
        let origin_x = hf.center.0 - (tree_size as i64 / 2) + offset as i64;
        let origin_y = hf.center.1 - (tree_size as i64 / 2) + offset as i64;
        
        let mut hf_result = Vec::new();
        collect_alive(&cache, result, result_depth, origin_x as u32, origin_y as u32, &mut hf_result);
        let hf_set: HashSet<u64> = hf_result.into_iter().collect();

        if flat_next != hf_set {
            let only_flat: Vec<_> = flat_next.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_next).collect();
            panic!(
                "advance_slow level {} MISMATCH at gen 22->23:\n\
                 flat({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only({}): {:?}\n\
                 hf_only({}): {:?}\n\
                 result_depth={} origin=({},{})",
                depth,
                flat_next.len(), flat_next.iter().take(10).map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().take(10).map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                result_depth, origin_x, origin_y,
            );
        } else {
            println!("advance_slow level {} OK at gen 22->23: {} cells match", depth, hf_set.len());
        }
    }

    /// Find all distinct level-3 nodes in the tree, recording the absolute (top-left)
    /// coordinate of each occurrence.  Returns Vec<(node_idx, abs_x, abs_y)>.
    fn find_level3_nodes(
        cache: &HashLifeCache,
        node_idx: usize,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(usize, i64, i64)>,
    ) {
        if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
            return;
        }
        if depth == 3 {
            out.push((node_idx, abs_x, abs_y));
            return;
        }
        let node = cache.get_node(node_idx);
        let half = 1i64 << (depth - 1);
        find_level3_nodes(cache, node.north_west, depth - 1, abs_x, abs_y, out);
        find_level3_nodes(cache, node.north_east, depth - 1, abs_x + half, abs_y, out);
        find_level3_nodes(cache, node.south_west, depth - 1, abs_x, abs_y + half, out);
        find_level3_nodes(cache, node.south_east, depth - 1, abs_x + half, abs_y + half, out);
    }

    /// Same as find_level3_nodes but for level-4 nodes (16×16).
    fn find_level4_nodes(
        cache: &HashLifeCache,
        node_idx: usize,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(usize, i64, i64)>,
    ) {
        if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
            return;
        }
        if depth == 4 {
            out.push((node_idx, abs_x, abs_y));
            return;
        }
        let node = cache.get_node(node_idx);
        let half = 1i64 << (depth - 1);
        find_level4_nodes(cache, node.north_west, depth - 1, abs_x, abs_y, out);
        find_level4_nodes(cache, node.north_east, depth - 1, abs_x + half, abs_y, out);
        find_level4_nodes(cache, node.south_west, depth - 1, abs_x, abs_y + half, out);
        find_level4_nodes(cache, node.south_east, depth - 1, abs_x + half, abs_y + half, out);
    }

    /// Convert a 4×4 cell block (top-left at tx,ty in the flat grid) into a 16-bit
    /// encoded value using the same bit layout as encode_level2:
    ///   bit_idx(r, c) = 15 - (r*4 + c),  where r=0 is the top row, c=0 is left.
    fn encode_4x4_from_grid(flat: &HashSet<u64>, tx: u32, ty: u32) -> u16 {
        let get = |x: u32, y: u32| -> u16 {
            if flat.contains(&pack(x, y)) { 1 } else { 0 }
        };
        let mut bits = 0u16;
        for r in 0..4u32 {
            for c in 0..4u32 {
                let bit_pos = 15 - (r * 4 + c);
                bits |= get(tx + c, ty + r) << bit_pos;
            }
        }
        bits
    }

    #[test]
    fn test_pi_heptamino_base_case() {
        // Pi heptamino initial pattern
        let pattern: &[(u32, u32)] = &[
            (100,100), (101,100), (102,100),
            (100,101),
            (100,102), (101,102), (102,102),
        ];
        let flat_initial: HashSet<u64> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();

        // Step 22 times using flat stepping (the correct reference)
        let mut flat = flat_initial.clone();
        for _ in 0..22 {
            flat = step_flat(&flat);
        }
        println!("\n=== Pi heptamino at generation 22 (flat reference) ===");
        println!("Alive cells ({}):", flat.len());
        let mut cells: Vec<(u32, u32)> = flat.iter().map(|&c| unpack(c)).collect();
        cells.sort();
        for (x, y) in &cells {
            println!("  ({}, {})", x, y);
        }

        // Build a fresh HashLife from gen-22 flat state
        let flat_vec: Vec<u64> = flat.iter().copied().collect();
        let hf = HashLife::from_flat(&flat_vec);
        println!("\nHashLife: depth={} center={:?} size={}", hf.depth, hf.center, hf.size());

        // Find all level-3 nodes in the tree and their absolute positions
        let origin_x = hf.center.0 - (hf.size() as i64 / 2);
        let origin_y = hf.center.1 - (hf.size() as i64 / 2);
        let mut l3_nodes: Vec<(usize, i64, i64)> = Vec::new();
        find_level3_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l3_nodes);

        // Filter to level-3 nodes that contain at least one alive cell (non-empty)
        let nonempty: Vec<(usize, i64, i64)> = l3_nodes.iter().filter(|(idx, _, _)| {
            let node = hf.cache.get_node(*idx);
            !node.is_empty
        }).copied().collect();
        println!("\nFound {} level-3 nodes ({} non-empty)", l3_nodes.len(), nonempty.len());

        // For each non-empty level-3 node, encode its 4 quadrants (each 4×4) and
        // compare with what the flat grid says.
        //
        // A level-3 node covers an 8×8 region.  Its layout (top-left at ax, ay):
        //   NW quadrant: (ax+0..3, ay+0..3)  → bits in NW mask (0xCC00)
        //   NE quadrant: (ax+4..7, ay+0..3)  → bits in NE mask (0x3300)
        //   SW quadrant: (ax+0..3, ay+4..7)  → bits in SW mask (0x00CC)
        //   SE quadrant: (ax+4..7, ay+4..7)  → bits in SE mask (0x0033)
        //
        // encode_level3 calls encode_level2 on each child (level-2 node = 4×4).
        // The child's 4 leaf cells map to the 4 bits within that quadrant's mask.

        let mut found_mismatch = false;
        for &(idx, ax, ay) in nonempty.iter() {
            let q = encode_level3(&hf.cache, idx);

            // Expected 4×4 blocks from the flat grid
            let exp_nw = encode_4x4_from_grid(&flat, ax as u32, ay as u32);
            let exp_ne = encode_4x4_from_grid(&flat, (ax + 4) as u32, ay as u32);
            let exp_sw = encode_4x4_from_grid(&flat, ax as u32, (ay + 4) as u32);
            let exp_se = encode_4x4_from_grid(&flat, (ax + 4) as u32, (ay + 4) as u32);

            let nw_ok = q.nw == exp_nw;
            let ne_ok = q.ne == exp_ne;
            let sw_ok = q.sw == exp_sw;
            let se_ok = q.se == exp_se;

            if !nw_ok || !ne_ok || !sw_ok || !se_ok {
                found_mismatch = true;
                println!("\n--- ENCODING MISMATCH at level-3 node idx={} abs=({},{}) ---", idx, ax, ay);
                if !nw_ok {
                    println!("  NW: encoded={:#018b} expected={:#018b}", q.nw, exp_nw);
                }
                if !ne_ok {
                    println!("  NE: encoded={:#018b} expected={:#018b}", q.ne, exp_ne);
                }
                if !sw_ok {
                    println!("  SW: encoded={:#018b} expected={:#018b}", q.sw, exp_sw);
                }
                if !se_ok {
                    println!("  SE: encoded={:#018b} expected={:#018b}", q.se, exp_se);
                }
                // Print the 8×8 cell block from the flat grid for visual reference
                println!("  Flat grid 8×8 block at ({},{})", ax, ay);
                for r in 0..8u32 {
                    let mut row = String::new();
                    for c in 0..8u32 {
                        let x = ax as u32 + c;
                        let y = ay as u32 + r;
                        row.push(if flat.contains(&pack(x, y)) { '#' } else { '.' });
                    }
                    println!("    {}", row);
                }
            } else {
                // Encoding matches — now test the rule table lookup + assembly.
                // Reproduce advance_base_one_gen logic and check intermediate values.
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

                // Decode the result bits into a 4×4 grid (same layout as encode_level2).
                let result_4x4: [[bool; 4]; 4] = {
                    let mut g = [[false; 4]; 4];
                    for r in 0..4u32 {
                        for c in 0..4u32 {
                            let bit_pos = 15 - (r * 4 + c);
                            g[r as usize][c as usize] = ((result_bits >> bit_pos) & 1) != 0;
                        }
                    }
                    g
                };

                // advance_base_one_gen advances the 8×8 region one generation.
                // The result is a 4×4 node.  The question is: what absolute region
                // does this 4×4 result correspond to?
                //
                // advance_slow at level 3 calls advance_base_one_gen, which returns
                // a level-2 node.  This node represents the centered 4×4 of the next
                // generation.  In the GOLDE scheme, advancing an 8×8 by 1 gen produces
                // a 6×6 centered result, but only the inner 4×4 is stored.
                //
                // The 4×4 result maps to absolute coordinates (ax+2 .. ax+5, ay+2 .. ay+5).
                let flat_next = step_flat(&flat);

                // Build expected 4×4 from flat_next at (ax+2, ay+2)
                let mut expected_4x4 = [[false; 4]; 4];
                for r in 0..4usize {
                    for c in 0..4usize {
                        let x = ax as u32 + 2 + c as u32;
                        let y = ay as u32 + 2 + r as u32;
                        expected_4x4[r][c] = flat_next.contains(&pack(x, y));
                    }
                }

                let result_ok = result_4x4 == expected_4x4;
                let near_center = (ax >= 94 && ax <= 110 && ay >= 90 && ay <= 110);

                if !result_ok || near_center {
                    println!("\n  Level-3 node idx={} abs=({},{}) {}", idx, ax, ay,
                             if result_ok { "— result OK" } else { "— *** RESULT MISMATCH ***" });
                    println!("  Rule results: nw={:#06b} n={:#06b} ne={:#06b} w={:#06b} c={:#06b} e={:#06b} sw={:#06b} s={:#06b} se={:#06b}",
                             r_nw & 0x3F, r_n & 0x3F, r_ne & 0x3F, r_w & 0x3F, r_c & 0x3F, r_e & 0x3F, r_sw & 0x3F, r_s & 0x3F, r_se & 0x3F);
                    println!("  Assembled result_bits = {:#018b}", result_bits);

                    // Print result 4×4
                    print!("  Result  4×4 (ax+2..+5, ay+2..+5):  | ");
                    for r in 0..4usize {
                        if r > 0 { print!("\n                                      | "); }
                        for c in 0..4usize {
                            print!("{}", if result_4x4[r][c] { '#' } else { '.' });
                        }
                    }
                    println!();

                    // Print expected 4×4
                    print!("  Expected 4×4 from flat gen-23:      | ");
                    for r in 0..4usize {
                        if r > 0 { print!("\n                                      | "); }
                        for c in 0..4usize {
                            print!("{}", if expected_4x4[r][c] { '#' } else { '.' });
                        }
                    }
                    println!();

                    // Also show the full 8×8 at gen 22 and the inner 6×6 at gen 23 for context
                    println!("  Input 8×8 at gen 22 (abs {},{}):", ax, ay);
                    for r in 0..8u32 {
                        print!("    ");
                        for c in 0..8u32 {
                            let x = ax as u32 + c;
                            let y = ay as u32 + r;
                            print!("{}", if flat.contains(&pack(x, y)) { '#' } else { '.' });
                        }
                        println!();
                    }
                    println!("  Flat gen-23 inner 6×6 (abs {}+1..+6, {}+1..+6):", ax, ay);
                    for r in 0..6u32 {
                        print!("    ");
                        for c in 0..6u32 {
                            let x = ax as u32 + 1 + c;
                            let y = ay as u32 + 1 + r;
                            print!("{}", if flat_next.contains(&pack(x, y)) { '#' } else { '.' });
                        }
                        println!();
                    }

                    if !result_ok {
                        found_mismatch = true;
                    }
                }
            }
        }

        if found_mismatch {
            panic!("advance_base_one_gen result mismatch detected — see output above");
        } else {
            println!("\n=== All level-3 nodes: encoding OK, advance_base_one_gen result OK ===");
        }

        // Now test level-4 sub-nodes via advance_slow, since the base case is fine.
        // Find all level-4 nodes and verify advance_slow on them matches flat stepping.
        let mut l4_nodes: Vec<(usize, i64, i64)> = Vec::new();
        find_level4_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l4_nodes);
        let l4_nonempty: Vec<(usize, i64, i64)> = l4_nodes.iter().filter(|(idx, _, _)| {
            let node = hf.cache.get_node(*idx);
            !node.is_empty
        }).copied().collect();
        println!("\nFound {} level-4 nodes ({} non-empty)", l4_nodes.len(), l4_nonempty.len());

        let flat_next = step_flat(&flat);
        let mut l4_mismatch = false;

        // advance_slow needs &mut cache, so clone the cache by rebuilding.
        // We'll test each level-4 node by rebuilding it into a fresh cache.
        for &(idx, ax, ay) in l4_nonempty.iter() {
            // Build a fresh cache with just this sub-tree copied
            let mut test_cache = HashLifeCache::new();
            let copied = rebuild_tree(&mut test_cache, idx, &hf.cache);

            // advance_slow at level 4 recurses to level 3 base case.
            // The result is a level-3 node (8×8) representing (ax+4 .. ax+11, ay+4 .. ay+11).
            let result_idx = advance_node(&mut test_cache, &mut AHashMap::new(), copied, 4, 0, &mut 0, &mut 0);
            let result_cells = collect_alive_helper(&test_cache, result_idx, 3);

            // Expected: cells in the flat_next grid at (ax+4 .. ax+11, ay+4 .. ay+11)
            let mut expected: Vec<(u32, u32)> = Vec::new();
            for r in 0..8u32 {
                for c in 0..8u32 {
                    let x = ax as u32 + 4 + c;
                    let y = ay as u32 + 4 + r;
                    if flat_next.contains(&pack(x, y)) {
                        expected.push((c, r));
                    }
                }
            }
            expected.sort();

            let mut got: Vec<(u32, u32)> = result_cells.iter().copied().collect();
            got.sort();

            if got != expected {
                l4_mismatch = true;
                println!("\n--- LEVEL-4 advance_slow MISMATCH at node idx={} abs=({},{}) ---", idx, ax, ay);
                println!("  Got ({} cells): {:?}", got.len(), got);
                println!("  Expected ({} cells): {:?}", expected.len(), expected);

                // Show the input 16×16 block for context
                println!("  Input 16×16 at gen 22 (abs {},{}):", ax, ay);
                for r in 0..16u32 {
                    print!("    ");
                    for c in 0..16u32 {
                        let x = ax as u32 + c;
                        let y = ay as u32 + r;
                        print!("{}", if flat.contains(&pack(x, y)) { '#' } else { '.' });
                    }
                    println!();
                }
            }
        }
        if l4_mismatch {
            println!("\n=== Level-4 advance_slow mismatches found ===");
        } else {
            println!("\n=== All level-4 advance_slow results OK ===");
        }

        // Finally: step the fresh HashLife once and compare with flat_next
        let mut hf2 = HashLife::from_flat(&flat_vec);
        hf2.step();
        let hf2_result: HashSet<u64> = hf2.to_flat().into_iter().collect();
        if hf2_result != flat_next {
            let only_flat: Vec<_> = flat_next.difference(&hf2_result).collect();
            let only_hf: Vec<_> = hf2_result.difference(&flat_next).collect();
            println!("\n=== FRESH HASHLIFE STEP MISMATCH (gen 22->23) ===");
            println!("depth={} center={:?}", hf2.depth, hf2.center);
            println!("flat_only({}): {:?}", only_flat.len(), only_flat.iter().take(10).map(|&c| unpack(*c)).collect::<Vec<_>>());
            println!("hf_only({}): {:?}", only_hf.len(), only_hf.iter().take(10).map(|&c| unpack(*c)).collect::<Vec<_>>());
        } else {
            println!("\n=== Fresh HashLife step gen 22->23: {} cells match ===", hf2_result.len());
        }
    }

    #[test]
    fn test_populate_viewport() {
        // Build a simple pattern and verify populate_viewport produces non-zero bits
        let cells: Vec<u64> = vec![
            pack(100, 100), pack(101, 100), pack(102, 100),
            pack(100, 101),
            pack(100, 102), pack(101, 102), pack(102, 102),
        ];
        let hf = HashLife::from_flat(&cells);

        // Viewport covering the pattern
        let mut bits = vec![0u8; (20 * 20 + 7) / 8];
        hf.populate_viewport(95, 95, 20, 20, &mut bits);

        // Count set bits
        let mut set_bits = 0;
        for &b in &bits {
            set_bits += b.count_ones() as usize;
        }
        println!("Viewport bits: {} set out of {} total", set_bits, bits.len() * 8);

        // The pi heptamino has 7 cells, all within [95,95]-[115,115]
        assert!(set_bits >= 7, "Expected at least 7 set bits, got {}", set_bits);

        // Also test: after stepping, does the viewport still work?
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step();
        let mut bits2 = vec![0u8; (20 * 20 + 7) / 8];
        hf2.populate_viewport(95, 95, 20, 20, &mut bits2);
        let mut set_bits2 = 0;
        for &b in &bits2 {
            set_bits2 += b.count_ones() as usize;
        }
        println!("After step: {} set bits", set_bits2);
        assert!(set_bits2 >= 7, "Expected at least 7 set bits after step, got {}", set_bits2);
    }

    #[test]
    fn test_multi_gen_advance() {
        // Test that multi-gen advance matches single-gen steps
        let pattern = [
            (100, 100), (101, 100), (102, 100),
            (100, 101), (102, 101),
            (100, 102), (101, 102),
        ];
        let cells: Vec<u64> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance using single-gen steps
        let mut hf1 = HashLife::from_flat(&cells);
        for _ in 0..8 {
            hf1.step();
        }
        let cells1 = hf1.to_flat();

        // Advance using multi-gen (step_n with advance_depth=1)
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step_n(8);
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Multi-gen advance doesn't match single-gen steps");
    }

    #[test]
    fn test_multi_gen_heptamino() {
        // Test multi-gen advance on pi heptamino
        let pattern = [
            (100, 100), (101, 100),
            (100, 101), (101, 101), (102, 101),
            (100, 102), (101, 102),
        ];
        let cells: Vec<u64> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance 16 gens using single-gen steps
        let mut hf1 = HashLife::from_flat(&cells);
        for _ in 0..16 {
            hf1.step();
        }
        let cells1 = hf1.to_flat();

        // Advance 16 gens using multi-gen
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step_n(16);
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Multi-gen heptamino doesn't match single-gen steps");
    }
}
