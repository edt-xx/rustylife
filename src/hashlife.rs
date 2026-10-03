//! GOLDE-style HashLife implementation.
//!
//! Core design:
//! - Node identity = u32 index. FALSE_NODE=0, TRUE_NODE=1.
//! - Arena = HashMap<[u32;4], u32> (children tuple -> idx)
//! - Nodes = HashMap<u32, LifeNode> (idx -> node data)
//! - Center-based tracking (like GOLDE's m_SeedOffset), NOT origin-based
//! - 65536-entry rule table: maps 16-bit 4x4 patterns -> 4-bit 2x2 center results
//! - AdvanceFast: recursive multi-gen advance (3x3 grid of overlapping sub-nodes)
//! - AdvanceSlow: recursive exact-gen advance (8x8 grid of segments)
//!
//! Modules: coords, cache, rules, tree, viewport (see src/hashlife/).

pub mod coords;
pub mod cache;
pub mod rules;
pub mod tree;
pub mod viewport;
#[cfg(test)] mod tests;
#[cfg(test)] mod converter_tests;

pub use cache::*;
pub use coords::*;
pub use rules::*;
pub use tree::*;
pub use viewport::*;

use ahash::AHashMap;

pub struct HashLife {
    pub cache: HashLifeCache,
    root: u32,
    center: (i64, i64),
    depth: u32,
    /// Two-tier slow cache: generational eviction.
    /// n = old tier (survives one cycle), n1 = current tier.
    /// On hit in n, promote to n1. New keys go in n1.
    /// Rotated with arena GC.
    pub slow_cache_n: AHashMap<u64, u32>,
    pub slow_cache_n1: AHashMap<u64, u32>,
    /// Steps remaining until arena GC (triggers GC + both cache rotations).
    /// Cache hit/miss counters (reset each step)
    pub slow_cache_hits: u64,
    pub slow_cache_misses: u64,
/// Last step's cache stats for display
    pub last_cache_size: u32,
    /// Last step's cache hit rate * 10 (e.g., 45.2% → 452)
    pub last_cache_hit_rate: u32,
    /// Live node count from the last gc() (rotate_caches runs after every
    /// step/step_n). Feeds the snapshot header "active" field — the dense
    /// nodes Vec len is the high-water mark, not the live count.
    pub last_gc_live_len: u32,
    /// Active steps (slow-cache hits+misses > 0) since the last rotation.
    pub active_since_rotate: u32,
    /// Active steps until the next rotation. Adapted at each rotation from
    /// the relative interval hit-rate delta (banded step size, see
    /// rotate_caches), clamped to [ROTATE_STEPS_MIN, ROTATE_STEPS_MAX].
    pub rotate_steps: u32,
    /// Slow-cache hits/misses accumulated since the last rotation (reset on
    /// rotation).
    pub interval_hits: u64,
    pub interval_misses: u64,
    /// Hit rate (per-mille) of the interval ending at the previous rotation.
    pub prev_interval_rate: u32,
    /// True once the first rotation has recorded a baseline interval rate.
    pub interval_rate_set: bool,
}

/// Step-count controller tuning (see rotate_caches):
const ROTATE_STEPS_INIT: u32 = 64;   // initial steps between rotations
const ROTATE_STEPS_MIN: u32 = 4;     // floor for the adaptive step count
const ROTATE_STEPS_MAX: u32 = 96;    // ceiling for the adaptive step count
impl HashLife {
    pub fn new() -> Self {
        let cache = HashLifeCache::new();
        Self {
            cache,
            root: FALSE_NODE,
            center: (0, 0),
            depth: 0,
            slow_cache_n: AHashMap::new(),
            slow_cache_n1: AHashMap::new(),
            slow_cache_hits: 0,
            slow_cache_misses: 0,
            active_since_rotate: 0,
            rotate_steps: ROTATE_STEPS_INIT,
            interval_hits: 0,
            interval_misses: 0,
            prev_interval_rate: 0,
            interval_rate_set: false,
            last_cache_size: 0,
            last_cache_hit_rate: 0,
            last_gc_live_len: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root == FALSE_NODE
    }

    pub fn center(&self) -> (i64, i64) {
        self.center
    }

    pub fn cell_count(&mut self) -> usize {
        count_cells(&mut self.cache, self.root, self.depth)
    }

    pub fn alive_count(&mut self) -> usize {
        count_cells(&mut self.cache, self.root, self.depth)
    }

    /// Collect all alive cells from the quadtree as packed u128 coords.
    pub fn collect_alive(&self) -> Vec<u128> {
        let mut alive = Vec::new();
        collect_alive(&self.cache, self.root, self.depth, 0, 0, &mut alive);
        // Offset from quadtree-local (0,0) to global coords
        let cx = self.center.0;
        let cy = self.center.1;
        let half = self.size() as i64 / 2;
        for cell in &mut alive {
            let (x, y) = coord_unpack(*cell);
            let gx = x as i64 + cx - half;
            let gy = y as i64 + cy - half;
            *cell = coord_pack(gx as u64, gy as u64);
        }
        alive
    }

    pub fn from_flat(data: &[u128]) -> Self {
        if data.is_empty() { return Self::new(); }

        let mut min_x = u64::MAX;
        let mut min_y = u64::MAX;
        let mut max_x = u64::MIN;
        let mut max_y = u64::MIN;
        for &cell in data {
            let (x, y) = coord_unpack(cell);
            min_x = min_x.min(x); min_y = min_y.min(y);
            max_x = max_x.max(x); max_y = max_y.max(y);
        }

        let span_x = (max_x - min_x + 1) as usize;
        let span_y = (max_y - min_y + 1) as usize;
        let size = next_power_of2(span_x.max(span_y).max(1));

        // For large spans, grid allocation would OOM. Build quadtree directly from cells.
        if size > 4096 {
            let depth = size.trailing_zeros();
            let mut cache = HashLifeCache::new();
            let tree = build_quadtree_from_cells(&mut cache, data, min_x, min_y, size as u32, depth);
            let mut hf = HashLife {
                cache,
                root: tree,
                center: (min_x as i64 + (size as i64 / 2), min_y as i64 + (size as i64 / 2)),
                depth: depth.max(3),
                slow_cache_n: AHashMap::new(),
                slow_cache_n1: AHashMap::new(),
                slow_cache_hits: 0,
                slow_cache_misses: 0,
                active_since_rotate: 0,
                rotate_steps: ROTATE_STEPS_INIT,
                interval_hits: 0,
                interval_misses: 0,
                prev_interval_rate: 0,
                interval_rate_set: false,
                last_cache_size: 0,
                last_cache_hit_rate: 0,
                last_gc_live_len: 0,
            };
            hf.shrink_caches();
            return hf;
        }

        // Center the pattern in the grid.
        let ox = (size - span_x) / 2;
        let oy = (size - span_y) / 2;

        let mut grid_2d = vec![vec![0u8; size]; size];
        for &cell in data {
            let (x, y) = coord_unpack(cell);
            grid_2d[(y - min_y + oy as u64) as usize][(x - min_x + ox as u64) as usize] = 1;
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

        let mut hf = Self {
            cache,
            root: tree,
            center: (origin_x + size as i64 / 2, origin_y + size as i64 / 2),
            depth,
            slow_cache_n: AHashMap::new(),
            slow_cache_n1: AHashMap::new(),
            slow_cache_hits: 0,
            slow_cache_misses: 0,
            active_since_rotate: 0,
            rotate_steps: ROTATE_STEPS_INIT,
            interval_hits: 0,
            interval_misses: 0,
            prev_interval_rate: 0,
            interval_rate_set: false,
            last_cache_size: 0,
            last_cache_hit_rate: 0,
            last_gc_live_len: 0,
        };
        hf.shrink_caches();
        hf
    }

    /// Shrink all caches to fit. Called after loading/pasting a new pattern to
    /// reclaim excess capacity from initial over-allocation. NOT called during
    /// stepping — during stepping, caches grow as needed without shrinking.
    pub fn shrink_caches(&mut self) {
        self.cache.nodes.shrink_to_fit();
        self.cache.arena.shrink_to_fit();
        self.cache.count_cache.shrink_to_fit();
        self.cache.fast_cache.shrink_to_fit();
        self.slow_cache_n.shrink_to_fit();
        self.slow_cache_n1.shrink_to_fit();
    }

    pub fn to_flat(&self) -> Vec<u128> {
        let mut result = Vec::new();
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        collect_alive(&self.cache, self.root, self.depth, origin_x as u64, origin_y as u64, &mut result);
        result
    }

    /// Export tree to MC (macrocell) format — child-first quadtree
    pub fn export_mc(&self) -> String {
        let mut out = String::new();
        out.push_str("[M2]\n");

        if self.root == FALSE_NODE || self.depth == 0 {
            out.push_str("Cells=0\n");
            return out;
        }

        let mut node_counter: u32 = 0;
        let pop = self.to_flat().len();
        out.push_str(&format!("Cells={}\n", pop));

        // Emit origin (0,0) — loadPattern handles centering via anchor offset
        out.push_str("# origin = 0 0\n");

        // Child-first traversal: children first, then parent
        let root_id = export_mc_node(&self.cache, self.root, self.depth, &mut node_counter, &mut out, 3);
        out.push_str(&format!("# root = {}\n", root_id));

        out
    }

    pub fn size(&self) -> usize { 1usize << self.depth }

    /// Return the viewport to actually fill, byte-aligned when the parallel
    /// 4-quadrant path will be used (bitmap > 64KB and depth >= 3).
    ///
    /// The parallel path races on seam bytes unless the quadrant seams land on
    /// byte boundaries, so we align the viewport (see `align_viewport`). When
    /// the single-threaded path is used the viewport is returned unchanged.
    /// The caller must size the output buffer from the returned dimensions and
    /// report them in the response so the frontend renders the (slightly
    /// larger) bitmap at the correct scale.
    pub fn aligned_viewport(
        &self,
        hvx: u64,
        hvy: u64,
        vw: u32,
        vh: u32,
        scale: u32,
    ) -> (u64, u64, u32, u32) {
        let bitmap_len = if scale > 1 {
            let agg_w = (vw as usize + scale as usize - 1) / scale as usize;
            let agg_h = (vh as usize + scale as usize - 1) / scale as usize;
            (agg_w * agg_h + 7) / 8
        } else {
            (vw as usize * vh as usize + 7) / 8
        };
        if bitmap_len > 64 * 1024 && self.depth >= 3 {
            align_viewport(self.center, hvx, hvy, vw, vh, scale)
        } else {
            (hvx, hvy, vw, vh)
        }
    }

    pub fn populate_viewport(&self, vx: u64, vy: u64, vw: u32, vh: u32, bits: &mut Vec<u8>) {
        let bits_len = (vw as usize * vh as usize + 7) / 8;
        if bits.len() < bits_len { bits.resize(bits_len, 0); }
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        // Parallelize when bitmap > 64KB and depth >= 3
        if bits_len > 64 * 1024 && self.depth >= 3 {
            let node = self.cache.get_node(self.root);
            let half = 1u32 << (self.depth - 1);
            let bits_ptr = bits.as_mut_ptr() as usize;
            let bits_len = bits_len;
            std::thread::scope(|s| {
                s.spawn(|| fill_viewport(&self.cache, node.north_west, self.depth - 1,
                    origin_x as u64, origin_y as u64, vx, vy, vw, vh, unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u8, bits_len) }));
                s.spawn(|| fill_viewport(&self.cache, node.north_east, self.depth - 1,
                    origin_x as u64 + half as u64, origin_y as u64, vx, vy, vw, vh, unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u8, bits_len) }));
                s.spawn(|| fill_viewport(&self.cache, node.south_west, self.depth - 1,
                    origin_x as u64, origin_y as u64 + half as u64, vx, vy, vw, vh, unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u8, bits_len) }));
                s.spawn(|| fill_viewport(&self.cache, node.south_east, self.depth - 1,
                    origin_x as u64 + half as u64, origin_y as u64 + half as u64, vx, vy, vw, vh, unsafe { std::slice::from_raw_parts_mut(bits_ptr as *mut u8, bits_len) }));
            });
        } else {
            fill_viewport(&self.cache, self.root, self.depth,
                origin_x as u64, origin_y as u64, vx, vy, vw, vh, &mut bits[..]);
        }
    }

    /// Populate an aggregated bitmap directly from the quadtree.
    /// Each raw cell maps to aggregated pixel ((x-vx)/scale, (y-vy)/scale).
    pub fn populate_aggregated_viewport(&self, vx: u64, vy: u64, vw: u32, vh: u32, scale: u32, agg: &mut Vec<u8>) {
        let agg_w = (vw as usize + scale as usize - 1) / scale as usize;
        let agg_h = (vh as usize + scale as usize - 1) / scale as usize;
        let agg_len = (agg_w * agg_h + 7) / 8;
        if agg.len() < agg_len { agg.resize(agg_len, 0); }
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        // Parallelize when bitmap > 64KB and depth >= 3
        if agg_len > 64 * 1024 && self.depth >= 3 {
            let node = self.cache.get_node(self.root);
            let half = 1u32 << (self.depth - 1);
            let agg_ptr = agg.as_mut_ptr() as usize;
            let agg_len = agg_len;
            std::thread::scope(|s| {
                s.spawn(|| fill_aggregated_viewport(&self.cache, node.north_west, self.depth - 1,
                    origin_x as u64, origin_y as u64, vx, vy, vw, vh, scale, agg_w as u32, agg_h as u32, unsafe { std::slice::from_raw_parts_mut(agg_ptr as *mut u8, agg_len) }));
                s.spawn(|| fill_aggregated_viewport(&self.cache, node.north_east, self.depth - 1,
                    origin_x as u64 + half as u64, origin_y as u64, vx, vy, vw, vh, scale, agg_w as u32, agg_h as u32, unsafe { std::slice::from_raw_parts_mut(agg_ptr as *mut u8, agg_len) }));
                s.spawn(|| fill_aggregated_viewport(&self.cache, node.south_west, self.depth - 1,
                    origin_x as u64, origin_y as u64 + half as u64, vx, vy, vw, vh, scale, agg_w as u32, agg_h as u32, unsafe { std::slice::from_raw_parts_mut(agg_ptr as *mut u8, agg_len) }));
                s.spawn(|| fill_aggregated_viewport(&self.cache, node.south_east, self.depth - 1,
                    origin_x as u64 + half as u64, origin_y as u64 + half as u64, vx, vy, vw, vh, scale, agg_w as u32, agg_h as u32, unsafe { std::slice::from_raw_parts_mut(agg_ptr as *mut u8, agg_len) }));
            });
        } else {
            fill_aggregated_viewport(&self.cache, self.root, self.depth,
                origin_x as u64, origin_y as u64, vx, vy, vw, vh, scale, agg_w as u32, agg_h as u32, &mut agg[..]);
        }
    }

    pub fn get_cell(&self, x: u64, y: u64) -> bool {
        let origin_x = self.center.0 - (self.size() as i64 / 2);
        let origin_y = self.center.1 - (self.size() as i64 / 2);
        let rel_x = x as i64 - origin_x;
        let rel_y = y as i64 - origin_y;
        let size = self.size() as i64;
        if rel_x < 0 || rel_y < 0 || rel_x >= size || rel_y >= size { return false; }
        get_cell_at(&self.cache, self.root, self.depth, rel_x as u32, rel_y as u32)
    }

    pub fn set_cell(&mut self, x: u64, y: u64, alive: bool) {
        // Handle empty tree: no-op for clearing, rebuild for setting
        if self.is_empty() {
            if !alive { return; }
            let cell = coord_pack(x, y);
            *self = HashLife::from_flat(&[cell]);
            return;
        }
        // Expand tree until cell is in bounds and depth >= 3
        loop {
            let origin_x = self.center.0 - (self.size() as i64 / 2);
            let origin_y = self.center.1 - (self.size() as i64 / 2);
            let rel_x = x as i64 - origin_x;
            let rel_y = y as i64 - origin_y;
            let size = self.size() as i64;
            if rel_x >= 0 && rel_y >= 0 && rel_x < size && rel_y < size && self.depth >= 3 {
                self.root = set_cell_at(&mut self.cache, self.root, self.depth, rel_x as u32, rel_y as u32, alive);
                return;
            }
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
    }

pub fn step(&mut self) {
        if self.is_empty() { return; }
        // Arena is append-only — nodes are immutable, no rebuild needed.
        // Slow_cache persists across steps for GOLDE-style performance.
        // Fast_cache also persists — same canonical node at same level always
        // advances to the same result. Cleared on GC only.

        // Expand until tree is large enough for advance_slow base case
        while needs_expansion(&self.cache, self.root, self.depth) || self.depth < 3 {
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
        // advance_node with target_depth=0 → always uses advance_slow (single-gen)
        self.root = advance_node(&mut self.cache, &mut self.slow_cache_n, &mut self.slow_cache_n1,
                                  self.root, self.depth, 0,
                                  &mut self.slow_cache_hits, &mut self.slow_cache_misses);
        self.depth -= 1;

        // Store cache stats for display
        let total = self.slow_cache_hits + self.slow_cache_misses;
        let cache_size = self.slow_cache_n.len() + self.slow_cache_n1.len();
        //eprintln!("[step] single-gen  depth={} total={}", self.depth + 1, total);
        if total > 0 {
            self.last_cache_size = cache_size as u32;
            self.last_cache_hit_rate = ((self.slow_cache_hits as f64 / total as f64) * 1000.0).round() as u32;
        } else {
            self.last_cache_size = 0;
            self.last_cache_hit_rate = 0;
        }
        // Compress after every step (slow-cache activity is passed in before
        // the per-step counters are reset below)
        let slow_active = self.slow_cache_hits + self.slow_cache_misses > 0;
        self.interval_hits += self.slow_cache_hits;
        self.interval_misses += self.slow_cache_misses;
        self.rotate_caches(slow_active);
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
        // GOLDE-style multi-gen advance using AdvanceNode dispatcher.
        // AdvanceFast drops 1 level per call, advancing 2^(level-2) generations.
        // GOLDE: expand tree, call AdvanceNode, result is at depth-1.
        // Falls back to single-gen steps for the remainder.
        let mut remaining = n;
        let mut active = false;

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

            let target_depth = k;

            self.root = advance_node(&mut self.cache, &mut self.slow_cache_n, &mut self.slow_cache_n1,
                                      self.root, self.depth, target_depth,
                                      &mut self.slow_cache_hits, &mut self.slow_cache_misses);
            // GOLDE always drops 1 level per DoOneJump, regardless of advance depth.
            // advance_fast returns a node at (level - 1), not (level - 2).
            self.depth -= 1;

            remaining -= advance_gens;

            // Store cache stats
            let total = self.slow_cache_hits + self.slow_cache_misses;
            active = total > 0;
            let cache_size = self.slow_cache_n.len() + self.slow_cache_n1.len();
            if total > 0 {
                self.last_cache_size = cache_size as u32;
                self.last_cache_hit_rate = ((self.slow_cache_hits as f64 / total as f64) * 1000.0).round() as u32;
            }
            self.interval_hits += self.slow_cache_hits;
            self.interval_misses += self.slow_cache_misses;
            self.slow_cache_hits = 0;
            self.slow_cache_misses = 0;

            // Expand after shrinking
            while needs_expansion(&self.cache, self.root, self.depth) {
                self.root = expand_node(&mut self.cache, self.root, self.depth);
                self.depth += 1;
            }
        }

        // Compress after step_n
        if remaining == 0 {
            self.rotate_caches(active);
        }

        // Fall back to single-gen steps for remainder (these create many more new nodes)
        // Each self.step() calls rotate_caches() internally
        for _ in 0..remaining {
            self.step();
        }

    }

    /// Compress caches: run GC, then rotate the slow-cache tiers when needed.
    /// The fast cache is pruned by gc, not rotated here. Called after every
    /// step() and step_n() with whether that step used the slow cache.
    ///
    /// Trigger: rotate after `rotate_steps` active steps. The step count is
    /// adapted at each rotation from the relative interval hit-rate delta
    /// d% = (rate-prev)*100/prev (per-mille rate over the steps since the
    /// last rotation): the first rotation only records the baseline; a later
    /// rotation steps N down if the rate rose and up if it fell, with banded
    /// step size: stable (|d|<5%) tightens by 1 only when d>=3, 5-15% -> 1,
    /// 15-30% -> 2, 30-60% -> 4, >=60% -> 8 (clamped to
    /// [ROTATE_STEPS_MIN, ROTATE_STEPS_MAX]). Idle steps count
    /// toward none of it.
    pub fn rotate_caches(&mut self, slow_active: bool) {
        // Always run GC
        self.last_gc_live_len = self.cache.gc(self.root, &mut self.slow_cache_n1, &mut self.slow_cache_n);

        // Activity gate: this step used the slow cache not at all.
        if !slow_active { return; }
        self.active_since_rotate += 1;

        // Rotate after rotate_steps active steps.
        if self.active_since_rotate < self.rotate_steps { return; }

        // Interval hit rate (per-mille) over the steps since the last rotation.
        let total = self.interval_hits + self.interval_misses;
        let rate = if total > 0 { (self.interval_hits * 1000 / total) as u32 } else { 0 };
        let prev = self.prev_interval_rate;

        // First rotation (or a zero baseline) only sets the baseline. After
        // that, the relative delta d% picks the step size: stable (|d|<5%)
        // tightens by 1 only when d>=3, 5-15% -> 1, 15-30% -> 2, 30-60% -> 4,
        // >=60% -> 8. Rate up tightens N, rate down loosens it.
        let d = if self.interval_rate_set && prev > 0 {
               ((rate as i64 - prev as i64) * 100) / (prev as i64)
            } else { 0 };
        if self.interval_rate_set && prev > 0 {
            let step:i32 = match d {
                 60..    =>  8,
                 30..60  =>  4,
                 15..30  =>  2,
                  5..15  =>  1,
                 -5..5   =>  0,
                -15..-5  => -1,
                -30..-15 => -2,
                -60..-30 => -4,
                   ..-60 => -8,
            };
            self.rotate_steps = self.rotate_steps.checked_add_signed(step).expect("Underflow").max(ROTATE_STEPS_MIN).min(ROTATE_STEPS_MAX);
        }
        self.prev_interval_rate = rate;
        self.interval_rate_set = true;

        std::mem::swap(&mut self.slow_cache_n, &mut self.slow_cache_n1);
        self.slow_cache_n1.clear();
        self.interval_hits = 0;
        self.interval_misses = 0;
        self.active_since_rotate = 0;
    }
}

fn needs_expansion(cache: &HashLifeCache, node_idx: u32, level: u32) -> bool {
    if node_idx == FALSE_NODE { return false; }
    if node_idx == TRUE_NODE { return false; }
    if level <= 3 { return true; }

    let not_empty = |idx: u32| -> bool {
        idx != FALSE_NODE && !cache.is_empty_check(idx)
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

fn next_power_of2(n: usize) -> usize {
    if n <= 1 { return 1; }
    n.next_power_of_two()
}
