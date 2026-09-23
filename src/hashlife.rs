#![allow(dead_code)]
//! GOLDE-style HashLife implementation.
//!
//! Core design:
//! - Node identity = u32 index. FALSE_NODE=0, TRUE_NODE=1.
//! - Arena = HashMap<[u32;4], u32> (children tuple → idx)
//! - Nodes = HashMap<u32, LifeNode> (idx → node data)
//! - Center-based tracking (like GOLDE's m_SeedOffset), NOT origin-based
//! - 65536-entry rule table: maps 16-bit 4x4 patterns → 4-bit 2x2 center results
//! - AdvanceFast: recursive multi-gen advance (3x3 grid of overlapping sub-nodes)
//! - AdvanceSlow: recursive exact-gen advance (8x8 grid of segments)

use ahash::AHashMap;
use std::collections::VecDeque;
use std::sync::{mpsc, atomic::{AtomicU8, Ordering}};

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
fn pad_to_agg_multiple(dim: u64, scale: u32) -> u64 {
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

// ============================================================================
// Constants
// ============================================================================

/// Append `n` zeroed `AtomicU8`s to `v`. Atomics are not `Clone` (and not
/// `Zeroed`), so `vec![...; n]` / `repeat().take()` / `repeat_n` can't fill
/// one — write them in place.
fn extend_zeros(v: &mut Vec<AtomicU8>, n: usize) {
    if n == 0 { return; }
    let start = v.len();
    v.reserve(n);
    unsafe {
        for i in start..start + n {
            *v.as_mut_ptr().add(i) = AtomicU8::new(0);
        }
        v.set_len(start + n);
    }
}

/// FALSE_NODE = index 0 (all children 0 = empty)
pub const FALSE_NODE: u32 = 0;

/// TRUE_NODE = index 1 (static alive leaf, children all TRUE_NODE)
pub const TRUE_NODE: u32 = 1;

/// Sentinel for advance_result meaning "no cached fast result"
const NO_FAST_CACHE: u32 = u32::MAX;

/// Bitmasks for extracting 2x2 quadrants from a 16-bit 4x4 grid.
const MASK_NW: u16 = 0xCC00;
const MASK_NE: u16 = 0x3300;
const MASK_SW: u16 = 0x00CC;
const MASK_SE: u16 = 0x0033;

// ============================================================================
// LifeNode (value type, returned by get_node)
// ============================================================================

#[derive(Clone, Copy)]
pub struct LifeNode {
    pub north_west: u32,
    pub north_east: u32,
    pub south_west: u32,
    pub south_east: u32,
    pub is_empty: bool,
}

/// Zeroed node: all children FALSE, marked empty. Fills unallocated slots of
/// the dense `nodes` Vec and any slot zeroed at GC, so a slot never reads as
/// a live node once freed.
pub const EMPTY_NODE: LifeNode = LifeNode {
    north_west: 0, north_east: 0, south_west: 0, south_east: 0, is_empty: true,
};

// ============================================================================
// Freelist buffer (fixed-length) + handoff
// ============================================================================

/// Freelist allocation state, held by the main thread only.
///
/// `Index`: hand out fresh node indices from the monotonic `next_idx` counter
/// (there is no recovered buffer in hand to reuse). `Reuse`: hand out
/// previously-freed (recovered) indices from the `freed` buffer and the `reuse`
/// queue before falling back to fresh.
///
/// Only `gc()` can move us into `Reuse` (it alone produces recovered indices);
/// `find_or_create` only moves us back to `Index` once the recovered supply
/// (the `freed` buffer and the `reuse` queue) drains. This encodes the old
/// "recovered before fresh" invariant as a state transition, replacing the
/// background freelist worker and its buffer queue.
enum AllocState { Index, Reuse }

// ============================================================================
// Arena + Cache
// ============================================================================

/// HashLife arena and canonicalization cache.
/// Arena = HashMap<[u32;4], u32> (children → idx).
/// Nodes = HashMap<u32, LifeNode> (idx → node data).
pub struct HashLifeCache {
    /// Canonicalization map: children tuple → node index
    pub arena: ahash::AHashMap<[u32; 4], u32>,
    /// Node storage: dense Vec indexed by idx. Grows lazily to the fresh-index
    /// high-water mark (`next_idx`); a reused (recovered) index is always
    /// within it. Slots freed at GC are zeroed to EMPTY_NODE.
    pub nodes: Vec<LifeNode>,
    /// One bit per idx: tracks the CURRENTLY-allocated set. Set when the idx
    /// is handed out by find_or_create, cleared when GC frees it. A freed or
    /// never-allocated slot has its bit clear, so the GC scan never re-frees
    /// it. (A map's extract_if removes the entry — the Vec keeps the slot, so
    /// the bit must be cleared explicitly.) The `nodes` range is not the
    /// allocated set; this bit is.
    pub issued: Vec<u8>,
    /// Fast cache for advance_fast results: (node_idx, level) → advanced node.
    /// Grows with use; pruned (not rotated) by gc, which retains only entries
    /// whose referenced nodes are still live.
    pub fast_cache: ahash::AHashMap<(u32, u32), u32>,
    /// Count cache: (node_idx, depth) → alive cell count.
    /// Memoizes count_cells — same canonical node at same depth always has same count.
    /// Cleared on arena GC (deleted nodes leave stale entries).
    pub count_cache: ahash::AHashMap<(u32, u32), usize>,
    /// The main's current freelist buffer: a block of recovered indices being
    /// popped in `find_or_create`. Holds a buffer only while in
    /// `AllocState::Reuse`; empty while in `AllocState::Index`.
    freed: Vec<u32>,
    /// Queued recovered buffers awaiting reuse — filled by `gc()`, drained by
    /// `find_or_create`. Only consulted while in `AllocState::Reuse`.
    reuse: VecDeque<Vec<u32>>,
    /// Monotonic fresh-index counter (the dense `nodes` Vec high-water mark).
    /// Starts at 2 (sentinels 0/1 are pre-allocated); only advanced while in
    /// `AllocState::Index`. Never reset across Index↔Reuse transitions.
    next_idx: u32,
    /// Freelist allocation state (Index vs Reuse). Only `gc()` can set
    /// `Reuse`; `find_or_create` only moves back to `Index` when the recovered
    /// supply (freed buffer + reuse queue) drains.
    alloc: AllocState,
}


impl HashLifeCache {
    /// Create an empty cache. The freelist is owned by the main thread: the
    /// fresh-index counter (`next_idx`) and the recovered-buffer queue
    /// (`reuse`). Starts in `AllocState::Index` (allocate from `next_idx`);
    /// the first GC with freed indices moves it to `AllocState::Reuse`.
    pub fn new() -> Self {
        let mut arena = ahash::AHashMap::with_capacity(65536);
        // Dense node storage: sentinels at idx 0/1. find_or_create resizes
        // `nodes` lazily as new high-water indices are allocated and sets
        // the matching `issued` bit — the bit IS the allocated set.
        let nodes = vec![
            EMPTY_NODE,
            LifeNode { north_west: 1, north_east: 1, south_west: 1, south_east: 1, is_empty: false },
        ];
        let mut issued = vec![0u8; 2];
        issued[0] = 1;
        issued[1] = 1;
        // Index 0 = FALSE_NODE
        arena.insert([0,0,0,0], 0);
        // Index 1 = TRUE_NODE
        arena.insert([1,1,1,1], 1);
        Self {
            arena,
            nodes,
            issued,
            fast_cache: ahash::AHashMap::new(),
            count_cache: ahash::AHashMap::new(),
            // No freelist worker: the fresh-index counter and the recovered
            // buffer queue live in the struct, owned by the main thread. Start
            // in Index state (allocate from next_idx); the first GC moves us
            // to Reuse.
            freed: Vec::new(),
            reuse: VecDeque::new(),
            next_idx: 2,
            alloc: AllocState::Index,
        }
    }

    pub fn get_node(&self, idx: u32) -> LifeNode {
        self.nodes.get(idx as usize).copied().unwrap_or(EMPTY_NODE)
    }

    #[inline] pub fn nw(&self, idx: u32) -> u32 { self.get_node(idx).north_west }
    #[inline] pub fn ne(&self, idx: u32) -> u32 { self.get_node(idx).north_east }
    #[inline] pub fn sw(&self, idx: u32) -> u32 { self.get_node(idx).south_west }
    #[inline] pub fn se(&self, idx: u32) -> u32 { self.get_node(idx).south_east }
    #[inline] pub fn is_empty_check(&self, idx: u32) -> bool { self.get_node(idx).is_empty }

    /// Find existing canonical node or create new one in arena.
    pub fn find_or_create(&mut self, nw: u32, ne: u32, sw: u32, se: u32) -> u32 {
        // Keep FALSE_NODE and TRUE_NODE as sentinels

        if nw == FALSE_NODE && ne == FALSE_NODE && sw == FALSE_NODE && se == FALSE_NODE {
            return FALSE_NODE;
        }
        if nw == TRUE_NODE && ne == TRUE_NODE && sw == TRUE_NODE && se == TRUE_NODE {
            return TRUE_NODE;
        }

	let key = [nw, ne, sw, se];

        if let Some(&idx) = self.arena.get(&key) {
            return idx;
        }

        let is_empty = {
            let is_e = |i: u32| -> bool {
                if i == FALSE_NODE { true }
                else if let Some(node) = self.nodes.get(i as usize) { node.is_empty }
                else { true }
            };
            is_e(nw) && is_e(ne) && is_e(sw) && is_e(se)
        };

        // Allocation (freelist state machine):
        //   Index: take the next fresh index from the monotonic counter.
        //   Reuse: pop from the current recovered buffer; when it drains, take
        //          the next queued recovered buffer; when the queue is empty,
        //          fall back to Index (fresh). Only gc() re-enters Reuse.
        let idx = match self.alloc {
            AllocState::Index => {
                let i = self.next_idx;
                self.next_idx += 1;
                i
            }
            AllocState::Reuse => match self.freed.pop() {
                Some(i) => i,
                None => match self.reuse.pop_front() {
                    Some(b) => {
                        self.freed = b;
                        self.freed.pop().expect("recovered buffer was empty")
                    }
                    None => {
                        self.alloc = AllocState::Index;
                        self.nodes.reserve(262144);
                        self.issued.reserve(262144);
                        let i = self.next_idx;
                        self.next_idx += 1;
                        i
                    }
                },
            },
        };
        let node = LifeNode { north_west: nw, north_east: ne, south_west: sw, south_east: se, is_empty };
        // Dense storage: lazily extend `nodes` on a new high-water idx (a fresh
        // index can jump ahead of the Vec end) and set that idx's `issued` bit
        // (GC clears it on free) so the GC scan can tell currently-allocated
        // (bit set) from freed/never-allocated (bit clear). This runs the same
        // for fresh (Index) and reused (Recovered) indices.
        let i = idx as usize;
        if i >= self.nodes.len() {
            self.nodes.resize(i+1, EMPTY_NODE);
            self.issued.resize(i+1, 0);
        }
        self.issued[i] = 1;
        self.nodes[i] = node;
        self.arena.insert(key, idx);
        idx
    }

    /// Collect all reachable node indices from root.
    pub fn walk_tree(&self, root: u32, live: &mut ahash::AHashSet<u32>) {
        let mut stack = vec![root];
        while let Some(idx) = stack.pop() {
            if idx == FALSE_NODE || idx == TRUE_NODE {
                live.insert(idx);
                continue;
            }
            if live.contains(&idx) { continue; }
            live.insert(idx);
            if let Some(node) = self.nodes.get(idx as usize) {
                if node.is_empty { 
                    continue; 
                }
                for &child in &[node.north_west, node.north_east, node.south_west, node.south_east] {
                    if !live.contains(&child) {
                        stack.push(child);
                    }
                }
            }
        }
    }

    /// Collect all reachable node indices from root using 5 threads (4 quadrants + slow cache).
    pub fn walk_tree_threaded(&self, root: u32, slow_cache_n1: &AHashMap<u64, u32>, slow_cache_n: &AHashMap<u64, u32>) -> Vec<AtomicU8> {
        let root_node = self.get_node(root);
        let children = [
            root_node.north_west,
            root_node.north_east,
            root_node.south_west,
            root_node.south_east,
        ];

        // Live bitvector: one byte per idx, sized to the node range; marked
        // idempotently so overlapping walks can mark the same byte. nodes
        // never shrinks (freed slots are zeroed, not removed) and does not
        // grow during the walk, so the size holds for the whole gc.
        let live_size = self.nodes.len().max(2);
        let mut live: Vec<AtomicU8> = Vec::with_capacity(live_size);
        extend_zeros(&mut live, live_size);
        live[root as usize].store(1, Ordering::SeqCst);

        // Walk scope: 4 quadtree threads + one thread per slow cache, all
        // pruning on live concurrently. No dedup set: the mark is idempotent
        // (load-check + store), so a node reached by any walk (quadrant or
        // cache) is expanded once and the rest prune on the live bit.
        std::thread::scope(|scope| {
            let live = &live;
            // Shared reference to the issued bitvector so the `move` closures
            // borrow (not move) it: `let issued = self.issued` moves out of
            // `&self` (E0507); the `&` is Copy and each closure copies it.
            let issued = &self.issued;
            // 4 threads for tree quadrants
            for &child in &children {
                scope.spawn(move || {
                    let mut stack = vec![ child ];
                    while let Some(idx) = stack.pop() {
                        if live[idx as usize].load(Ordering::Relaxed) != 0 {
                            continue;
                        }
                        live[idx as usize].store(1, Ordering::SeqCst);
                        if let Some(node) = self.nodes.get(idx as usize) {
                            if node.is_empty {
                                continue;
                            }
                            stack.push(node.north_west);
                            stack.push(node.north_east);
                            stack.push(node.south_west);
                            stack.push(node.south_east);
                        }
                    }
                });
            }

            // slow_cache_n walk: iterate the map, push each referenced node,
            // walk its subtree. Overlaps the n1 thread; live dedups.
            scope.spawn(move || {
                let mut stack = Vec::<u32>::new();
                for (&key, _) in slow_cache_n.iter() {
                    stack.push((key >> 32) as u32);
                    while let Some(idx) = stack.pop() {
                        let i = idx as usize;
                        // Skip nodes not currently issued: a stale cache key
                        // may reference an idx GC freed (possibly already
                        // reissued to another node). Plain byte load, put
                        // before the atomic live check.
                        if issued[i] == 0 {
                            continue;
                        }
                        if live[i].load(Ordering::Relaxed) != 0 {
                            continue;
                        }
                        live[i].store(1, Ordering::SeqCst);
                        if let Some(node) = self.nodes.get(i) {
                            if node.is_empty {
                                continue;
                            }
                            stack.push(node.north_west);
                            stack.push(node.north_east);
                            stack.push(node.south_west);
                            stack.push(node.south_east);
                        }
                    }
                }
            });

            // slow_cache_n1 walk: same shape, other cache.
            scope.spawn(move || {
                let mut stack = Vec::<u32>::new();
                for (&key, _) in slow_cache_n1.iter() {
                    stack.push((key >> 32) as u32);
                    while let Some(idx) = stack.pop() {
                        let i = idx as usize;
                        // Same issued guard as the slow_cache_n thread.
                        if issued[i] == 0 {
                            continue;
                        }
                        if live[i].load(Ordering::Relaxed) != 0 {
                            continue;
                        }
                        live[i].store(1, Ordering::SeqCst);
                        if let Some(node) = self.nodes.get(i) {
                            if node.is_empty {
                                continue;
                            }
                            stack.push(node.north_west);
                            stack.push(node.north_east);
                            stack.push(node.south_west);
                            stack.push(node.south_east);
                        }
                    }
                }
            });
        });

        live
    }

    /// Garbage collect: walk tree from root, retain only live nodes.
    /// Also walks subtrees of slow_cache_n1 and fast_cache referenced nodes.
    pub fn gc(&mut self, root: u32, slow_cache_n1: &mut AHashMap<u64, u32>, slow_cache_n: &mut AHashMap<u64, u32>) -> u32 {
        let live = self.walk_tree_threaded(root, slow_cache_n1, slow_cache_n);

        // Thread: computes the freed indices and hands them back; the actual
        // freelist fill (append + fresh top-up) is done by the background
        // worker so it does not block the main thread. Threads 2-4 do the
        // cache retains concurrently; thread 5 sums the live bitvector,
        // which used to run sequentially here before the scope.
        // Shared reference to the live bitvector so the closures borrow (not
        // move) it; the `move` closure copies this `&Vec` (Copy) cheaply.
        // Plain loads are fine: every mark happened before this scope
        // spawned (both walk scopes joined), so no atomic ordering is needed
        // at read time.
        //eprintln!("gc n {} n1 {}",slow_cache_n.len(), slow_cache_n1.len());
        let live_ref: &Vec<AtomicU8> = &live;
        let (freed_vec, live_len): (Vec<u32>, u64) = std::thread::scope(|s| {
            let nodes = &mut self.nodes;
            let issued = &mut self.issued;
            // Sending Gc msg to worker directly (modified to just add to front) is 3-4% slower
            // It is critical to performance to use freed nodes asap avoiding 'new' nodes...
            let (tx1, rx1) = mpsc::channel::<Vec<u32>>();
            s.spawn(move || {
                // Dense Vec: no extract (removing an element would shift
                // every index). Scan instead. A slot is freeable ONLY if
                // issued (handed out of a buffer) and not live. Never-issued
                // slots (the Vec's range always outruns the allocated set)
                // are skipped: freeing them would double-allocate an index
                // still held in its original buffer. Dead slots are zeroed
                // so a freed slot never reads as a live node (the map's
                // extract_if made freed slots simply absent).
                let mut fv: Vec<u32> = Vec::new();
                for i in 0..nodes.len() {
                    if issued[i] == 0 { continue; }
                    // Bitvector: direct byte load (the old shared-AHashSet
                    // contains was a hash probe per slot). Guarded get in case
                    // the Vec ever grew past the bitvector (shouldn't happen
                    // mid-gc; out-of-range reads as dead).
                    if live_ref.get(i).map_or(true, |b| b.load(Ordering::Relaxed) == 0) {
                        // Free: zero the slot AND clear the issued bit. The
                        // clear is the key — the Vec keeps the slot (a map's
                        // extract_if removed it), so without clearing, the
                        // next GC would see issued still set and free the
                        // same idx again → double-allocation → clobbered
                        // children → corruption.
                        nodes[i] = EMPTY_NODE;
                        issued[i] = 0;
                        fv.push(i as u32);
                    }
                }
                let _ = tx1.send(fv);
            });

            // Thread: remove dead entries from fast_cache
            let fast_cache = &mut self.fast_cache;
            s.spawn(|| {
                fast_cache.retain(|&(nidx, _), ridx| live_ref.get(nidx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Thread: drop slow_cache_n entries whose key node or result is
            // not live (restore of the f6d4df2 retain; dca4da7 dropped it).
            // Pairs with the walk's issued guard: stale keys are never marked
            // live, so their entries are pruned here. The `move` moves the
            // `&mut` param into this one thread; the scope owns its lifetime.
            s.spawn(move || {
                slow_cache_n.retain(|&key, ridx| live_ref.get((key >> 32) as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // and n1
            s.spawn(move || {
                slow_cache_n1.retain(|&key, ridx| live_ref.get((key >> 32) as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Thread: remove dead entries from arena, add sentinels
            let arena = &mut self.arena;
            s.spawn(|| {
                arena.retain(|_, idx| live_ref.get(*idx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
                arena.entry([0,0,0,0]).or_insert(0);
                arena.entry([1,1,1,1]).or_insert(1);
            });

            // Thread: retain live entries in count_cache
            let count_cache = &mut self.count_cache;
            s.spawn(|| {
                count_cache.retain(|&(nidx, _), _| live_ref.get(nidx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Thread: sum the live bitvector (plain Relaxed loads — all
            // marks are done). Overlaps with the retains instead of running
            // sequentially before them.
            let (tx2, rx2) = mpsc::channel::<u64>();
            s.spawn(move || {
                let len: u64 = live_ref.iter().map(|b| b.load(Ordering::Relaxed) as u64).sum();
                let _ = tx2.send(len);
            });

            (rx1.recv().unwrap(), rx2.recv().unwrap())
        });
        //eprintln!("slow n {} n1 {}",slow_cache_n.len(), slow_cache_n1.len());
        // gc() returns the recovered indices; from here the GC is main-thread
        // only. Feed them to the freelist state machine:
        //   Index: no recovered buffer in hand (freed is empty) — take this GC's
        //          recovered buffer as `freed` and move to Reuse.
        //   Reuse: queue it after the buffer(s) already in hand, for reuse
        //          before fresh.
        // Only gc() can enter Reuse; an empty freed_vec changes nothing.
        if !freed_vec.is_empty() {
            match self.alloc {
                AllocState::Index => {
                    self.freed = freed_vec;
                    self.alloc = AllocState::Reuse;
                }
                AllocState::Reuse => {
                    self.reuse.push_back(freed_vec);
                }
            }
        }

        live_len as u32
    }
}

// ============================================================================
// Utility
// ============================================================================

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

/// Inline encode_level3 — fetches all nodes in a tight loop, computes bits inline.
/// Eliminates 20 function calls per base case vs. the old encode_quadrant_* chain.
#[inline]
fn encode_level3(cache: &HashLifeCache, node_idx: u32) -> LeafQuadrants {
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
fn encode_level2_inline(cache: &HashLifeCache, node_idx: u32) -> u16 {
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
fn decode_level2(cache: &mut HashLifeCache, bits: u16) -> u32 {
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

/// Decode 16-bit value into a level-1 node (2x2 grid).
/// GOLDE's DecodeLevel2 does this — creates 4 leaf children from
/// the center 2x2 of the 4x4 result. Used by advance_base_two_gen.
fn decode_level1(cache: &mut HashLifeCache, bits: u16) -> u32 {
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
fn ensure_level1(cache: &mut HashLifeCache, idx: u32) -> u32 {
    if idx == FALSE_NODE {
        cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, FALSE_NODE)
    } else if idx == TRUE_NODE {
        cache.find_or_create(TRUE_NODE, TRUE_NODE, TRUE_NODE, TRUE_NODE)
    } else {
        idx // already a proper level-1 node
    }
}

/// Ensure a node is at exactly level 2 (4x4 grid)
fn ensure_level2(cache: &mut HashLifeCache, idx: u32) -> u32 {
    if idx == FALSE_NODE {
        let l1 = cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, FALSE_NODE);
        cache.find_or_create(l1, l1, l1, l1)
    } else if idx == TRUE_NODE {
        let l1 = cache.find_or_create(TRUE_NODE, TRUE_NODE, TRUE_NODE, TRUE_NODE);
        cache.find_or_create(l1, l1, l1, l1)
    } else {
        // Check if grandchildren are at level 1
        let c = cache.get_node(idx);
        let nw = ensure_level1(cache, c.north_west);
        let ne = ensure_level1(cache, c.north_east);
        let sw = ensure_level1(cache, c.south_west);
        let se = ensure_level1(cache, c.south_east);
        cache.find_or_create(nw, ne, sw, se)
    }
}

/// Build quadtree directly from packed cell list (no 2D grid allocation).
/// Partitions cells into quadrants recursively, creating leaf nodes at 8x8 blocks.
fn build_quadtree_from_cells(cache: &mut HashLifeCache, cells: &[u128], ox: u64, oy: u64, size: u32, depth: u32) -> u32 {
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

/// Ensure a node is at exactly level 3 (8x8 grid) by expanding collapsed children.
/// When find_or_create collapses identical children, a node that should be
/// at level 3 may actually be at a lower level. This function rebuilds the
/// node to ensure proper level-3 structure.
fn ensure_level3(cache: &mut HashLifeCache, node_idx: u32) -> u32 {
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

    let node = cache.get_node(node_idx);
    let nw = ensure_level2(cache, node.north_west);
    let ne = ensure_level2(cache, node.north_east);
    let sw = ensure_level2(cache, node.south_west);
    let se = ensure_level2(cache, node.south_east);
    cache.find_or_create(nw, ne, sw, se)
}

fn advance_base_one_gen(cache: &mut HashLifeCache, node_idx: u32) -> u32 {
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
fn advance_node(cache: &mut HashLifeCache, slow_cache_n: &mut AHashMap<u64, u32>,
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
fn advance_fast(cache: &mut HashLifeCache,
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
fn centered_horizontal(cache: &mut HashLifeCache, west: &LifeNode, east: &LifeNode) -> u32 {
    cache.find_or_create(west.north_east, east.north_west, west.south_east, east.south_west)
}

/// CenteredVertical: extract inner 2x2 from north+south nodes
fn centered_vertical(cache: &mut HashLifeCache, north: &LifeNode, south: &LifeNode) -> u32 {
    cache.find_or_create(north.south_west, north.south_east, south.north_west, south.north_east)
}

/// CenteredSubNode: extract central 2x2 from 4 child nodes
fn centered_subnode(cache: &mut HashLifeCache, nw: &LifeNode, ne: &LifeNode, sw: &LifeNode, se: &LifeNode) -> u32 {
    cache.find_or_create(nw.south_east, ne.south_west, sw.north_east, se.north_west)
}

/// Advance 2 generations at level 3 (8x8 base case)
/// Returns a level-2 node (4x4 grid) — drops 1 level, matching GOLDE's AdvanceBase.
fn advance_base_two_gen(cache: &mut HashLifeCache, node_idx: u32) -> u32 {
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

fn advance_slow(cache: &mut HashLifeCache, slow_cache_n: &mut AHashMap<u64, u32>,
                 slow_cache_n1: &mut AHashMap<u64, u32>,
                 node_idx: u32, level: u32, target_depth: u32,
                 hits: &mut u64, misses: &mut u64) -> u32 {
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
fn fetch_segments(cache: &HashLifeCache, node_idx: u32) -> [u32; 64] {
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

// ============================================================================
// Empty tree at a given level (all FALSE_NODE)
fn empty_tree(cache: &mut HashLifeCache, level: u32) -> u32 {
    if level <= 0 { return FALSE_NODE; }
    let child = empty_tree(cache, level - 1);
    cache.find_or_create(child, child, child, child)
}

// Expand node — GOLDE-style shift-up.
// Places each child in a corner with empty padding, creating a node one level deeper.
fn expand_node(cache: &mut HashLifeCache, node_idx: u32, level: u32) -> u32 {
    if node_idx == FALSE_NODE {
        return empty_tree(cache, level + 1);
    }
    if node_idx == TRUE_NODE {
        return cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, TRUE_NODE);
    }
    let empty = if level > 0 { empty_tree(cache, level - 1) } else { FALSE_NODE };
    let node = cache.get_node(node_idx);
    let expanded_nw = cache.find_or_create(empty, empty, empty, node.north_west);
    let expanded_ne = cache.find_or_create(empty, empty, node.north_east, empty);
    let expanded_sw = cache.find_or_create(empty, node.south_west, empty, empty);
    let expanded_se = cache.find_or_create(node.south_east, empty, empty, empty);
    cache.find_or_create(expanded_nw, expanded_ne, expanded_sw, expanded_se)
}

// ============================================================================
// Quadtree build / walk helpers
// ============================================================================

fn build_quadtree(cache: &mut HashLifeCache, grid: &[Vec<u8>], x0: usize, y0: usize, w: usize, h: usize) -> u32 {
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

fn count_cells(cache: &mut HashLifeCache, node_idx: u32, depth: u32) -> usize {
    if node_idx == FALSE_NODE { return 0; }
    // A depth-d block is 2^d x 2^d = 4^d cells (leaves are 1x1 sentinels).
    if node_idx == TRUE_NODE { return 1usize << (2 * depth); }
    if depth == 0 { return 1; }
    // Check memoization cache
    if let Some(&c) = cache.count_cache.get(&(node_idx, depth)) {
        return c;
    }
    let node = cache.get_node(node_idx);
    let count = count_cells(cache, node.north_west, depth - 1)
        + count_cells(cache, node.north_east, depth - 1)
        + count_cells(cache, node.south_west, depth - 1)
        + count_cells(cache, node.south_east, depth - 1);
    cache.count_cache.insert((node_idx, depth), count);
    count
}

/// Collect alive cells from a node (relative coordinates, no packing)
fn collect_alive_helper(cache: &HashLifeCache, node_idx: u32, depth: u32) -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    collect_alive_rel(cache, node_idx, depth, 0, 0, &mut result);
    result
}

fn collect_alive_rel(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u32, oy: u32, result: &mut Vec<(u32, u32)>) {
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
fn rebuild_tree(new_cache: &mut HashLifeCache, old_idx: u32, old_cache: &HashLifeCache) -> u32 {
    if old_idx == FALSE_NODE { return FALSE_NODE; }
    if old_idx == TRUE_NODE { return TRUE_NODE; }
    let old_node = old_cache.get_node(old_idx);
    let nw = rebuild_tree(new_cache, old_node.north_west, old_cache);
    let ne = rebuild_tree(new_cache, old_node.north_east, old_cache);
    let sw = rebuild_tree(new_cache, old_node.south_west, old_cache);
    let se = rebuild_tree(new_cache, old_node.south_east, old_cache);
    new_cache.find_or_create(nw, ne, sw, se)
}

/// Export a tree node to MC format — child-first ordering
/// Returns the node ID assigned to this node
fn export_mc_node(cache: &HashLifeCache, node_idx: u32, depth: u32, counter: &mut u32, out: &mut String, leaf_depth: u32) -> u32 {
    // FALSE_NODE and TRUE_NODE are static — don't emit them
    if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
        return node_idx;
    }

    // Leaf node at leaf_depth (8x8 grid)
    if depth <= leaf_depth {
        *counter += 1;
        let id = *counter;
        // Emit 8x8 grid — one line per node, newline at end
        let size = 1u32 << depth;
        for y in 0..8 {
            for x in 0..8 {
                let gx = if size > 0 { x / (8 / size) } else { x };
                let gy = if size > 0 { y / (8 / size) } else { y };
                let alive = get_cell_at(cache, node_idx, depth, gx, gy);
                out.push(if alive { '*' } else { '.' });
            }
            out.push('$');
        }
        out.push('\n');
        return id;
    }

    // Non-leaf node — emit children first (child-first ordering)
    let node = cache.get_node(node_idx);
    let nw_id = export_mc_node(cache, node.north_west, depth - 1, counter, out, leaf_depth);
    let ne_id = export_mc_node(cache, node.north_east, depth - 1, counter, out, leaf_depth);
    let sw_id = export_mc_node(cache, node.south_west, depth - 1, counter, out, leaf_depth);
    let se_id = export_mc_node(cache, node.south_east, depth - 1, counter, out, leaf_depth);

    // Now emit this node
    *counter += 1;
    let id = *counter;
    out.push_str(&format!("{} {} {} {} {}\n", depth, nw_id, ne_id, sw_id, se_id));
    id
}

fn collect_alive(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, result: &mut Vec<u128>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        // Uniform block — fill entire region
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push(coord_pack(ox + x as u64, oy + y as u64));
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
    collect_alive(cache, node.north_east, depth - 1, ox.wrapping_add(half as u64), oy, result);
    collect_alive(cache, node.south_west, depth - 1, ox, oy.wrapping_add(half as u64), result);
    collect_alive(cache, node.south_east, depth - 1, ox.wrapping_add(half as u64), oy.wrapping_add(half as u64), result);
}

fn get_cell_at(cache: &HashLifeCache, node_idx: u32, depth: u32, x: u32, y: u32) -> bool {
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

fn set_cell_at(cache: &mut HashLifeCache, node_idx: u32, depth: u32, x: u32, y: u32, alive: bool) -> u32 {
    if depth == 0 {
        return if alive { TRUE_NODE } else { FALSE_NODE };
    }
    // Short-circuit: no-op when setting to current value
    if node_idx == FALSE_NODE && !alive { return FALSE_NODE; }
    if node_idx == TRUE_NODE && alive { return TRUE_NODE; }
    // FALSE_NODE → alive=true: need to split and set
    // TRUE_NODE → alive=false: need to split and clear
    let node = cache.get_node(node_idx);
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

// Profiling hook (candidate 4, SSE2 TRUE fill): cumulative bytes written by
// the TRUE_NODE fill branch of fill_viewport, across all threads. Cheap: one
// relaxed atomic add per TRUE visit, no clock reads.
static TRUE_FILL_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
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

fn fill_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, bits: &mut [u8]) {
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
fn fill_aggregated_viewport(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, vx: u64, vy: u64, vw: u32, vh: u32, scale: u32, agg_w: u32, agg_h: u32, agg: &mut [u8]) {
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

// ============================================================================
// HashLife main struct
// ============================================================================

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
    /// Step count used in last step_n() call (for freelist pre-grow scaling)
    pub last_step_count: u32,
    /// Total number of rotate_caches() calls since construction
    pub rotate_count: u32,
}

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
            last_cache_size: 0,
            last_cache_hit_rate: 0,
            last_gc_live_len: 0,
            last_step_count: 0,
            rotate_count: 0,
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
                last_cache_size: 0,
                last_cache_hit_rate: 0,
                last_gc_live_len: 0,
                last_step_count: 0,
                rotate_count: 0,
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
            last_cache_size: 0,
            last_cache_hit_rate: 0,
            last_gc_live_len: 0,
            last_step_count: 0,
            rotate_count: 0,
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
        self.slow_cache_hits = 0;
        self.slow_cache_misses = 0;

        // Compress after every step
        self.rotate_caches();

        // After shrinking, pattern may touch rim — expand again if needed
        while needs_expansion(&self.cache, self.root, self.depth) {
            self.root = expand_node(&mut self.cache, self.root, self.depth);
            self.depth += 1;
        }
    }

    pub fn step_n(&mut self, n: u32) {
        if self.is_empty() || n == 0 { return; }
        self.last_step_count = n;
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

            let target_depth = k;
            //let dbg_depth = self.depth;

            self.root = advance_node(&mut self.cache, &mut self.slow_cache_n, &mut self.slow_cache_n1,
                                      self.root, self.depth, target_depth,
                                      &mut self.slow_cache_hits, &mut self.slow_cache_misses);
            // GOLDE always drops 1 level per DoOneJump, regardless of advance depth.
            // advance_fast returns a node at (level - 1), not (level - 2).
            self.depth -= 1;

            remaining -= advance_gens;

            // Store cache stats
            let total = self.slow_cache_hits + self.slow_cache_misses;
            let cache_size = self.slow_cache_n.len() + self.slow_cache_n1.len();
            //eprintln!("[step_n] multi-gen  depth={} k={} advance_gens={} routed={} total={}",
            //          dbg_depth, k, advance_gens,
            //          if dbg_depth - 2 > k { "slow(top)->mixed" } else { "fast" },
            //          total);
            if total > 0 {
                self.last_cache_size = cache_size as u32;
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

        // Compress after step_n
        if remaining == 0 {
            self.rotate_caches();
        }

        // Fall back to single-gen steps for remainder (these create many more new nodes)
        // Each self.step() calls rotate_caches() internally
        for _ in 0..remaining {
            self.step();
        }

    }

    /// Compress caches: run GC, then rotate the slow-cache tiers when needed.
    /// The fast cache is pruned by gc, not rotated here. Called after every
    /// step() and step_n().
    pub fn rotate_caches(&mut self) {
        self.rotate_count += 1;
        // Always run GC
        self.last_gc_live_len = self.cache.gc(self.root, &mut self.slow_cache_n1, &mut self.slow_cache_n);

        // Rotate the slow-cache tiers when the n tier has shrunk well below
        // n1 (after a few rotations) or at the rotation cap.
        if (self.slow_cache_n.len() < self.slow_cache_n1.len()*3 && self.rotate_count > 16) || self.rotate_count > 32 {
            std::mem::swap(&mut self.slow_cache_n, &mut self.slow_cache_n1);
            self.slow_cache_n1.clear();
            self.rotate_count = 0;
        }
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

// ============================================================================
// Test: compare HashLife against flat-grid stepping
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use ahash::AHashMap;

    // Inline Coord for test context — must match module-level coord_pack/coord_unpack
    fn pack(x: u32, y: u32) -> u128 { (y as u128) << 64 | x as u128 }
    fn unpack(cell: u128) -> (u64, u64) { ((cell & ((1u128 << 64) - 1)) as u64, (cell >> 64) as u64) }

    fn step_flat(alive: &HashSet<u128>) -> HashSet<u128> {
        let mut counts = AHashMap::new();
        for &cell in alive {
            let (x, y) = unpack(cell);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = x.wrapping_add(dx as u64);
                    let ny = y.wrapping_add(dy as u64);
                    let key = pack(nx as u32, ny as u32);
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
        let flat_initial: HashSet<u128> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();
        let mut flat: HashSet<u128> = flat_initial.clone();
        let mut hf = HashLife::from_flat(&flat_initial.iter().copied().collect::<Vec<_>>());

        for i in 0..gens {
            // Check to_flat matches BEFORE stepping
            let hf_before: HashSet<u128> = hf.to_flat().into_iter().collect();
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
            let hf_result: HashSet<u128> = hf.to_flat().into_iter().collect();
            if flat_next != hf_result {
                let only_flat: Vec<_> = flat_next.difference(&hf_result).collect();
                let only_hf: Vec<_> = hf_result.difference(&flat_next).collect();

                panic!(
                    "{} STEP MISMATCH gen {}->{}: flat={} hf={}\n\
                     depth={} center={:?} nodes={}\n\
                     flat_only({}): {:?}\n\
                     hf_only({}): {:?}",
                    name, i, i+1, flat_next.len(), hf_result.len(),
                    hf.depth, hf.center, hf.last_gc_live_len,
                    only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                    only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>()
                );
            }
            println!("{} gen {} OK ({} cells) depth={} nodes={}",
                name, i+1, flat_next.len(), hf.depth, hf.last_gc_live_len);
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
        let mut flat: HashSet<u128> = HashSet::new();
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
        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), tree, 4, 0, &mut 0, &mut 0);

        // Result should be a level-3 node (8x8), centered
        // The center 8x8 covers cells (4,4) to (11,11) in the 16x16 grid
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 3, 4, 4, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        // Compare: flat_next should match hf_set for the center 8x8 region
        let flat_center: HashSet<u128> = flat_next.iter()
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

        let mut flat: HashSet<u128> = HashSet::new();
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

        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), tree, 5, 0, &mut 0, &mut 0);

        // Result is level-4 (16x16), centered at (8,8) of the 32x32
        // Covers cells (8,8) to (23,23)
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 4, 8, 8, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        let flat_center: HashSet<u128> = flat_next.iter()
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
        let initial: HashSet<u128> = [
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
        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), root, depth, 0, &mut 0, &mut 0);

        // Result is at level depth-1
        let result_depth = depth - 1;
        let result_size = 1u32 << result_depth;
        
        // The result covers the center of the expanded tree
        let tree_size = 1u32 << depth;
        let offset = (tree_size - result_size) / 2;
        let origin_x = hf.center.0 - (tree_size as i64 / 2) + offset as i64;
        let origin_y = hf.center.1 - (tree_size as i64 / 2) + offset as i64;
        
        let mut hf_result = Vec::new();
        collect_alive(&cache, result, result_depth, origin_x as u64, origin_y as u64, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.into_iter().collect();

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
        node_idx: u32,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(u32, i64, i64)>,
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
        node_idx: u32,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(u32, i64, i64)>,
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
    fn encode_4x4_from_grid(flat: &HashSet<u128>, tx: u32, ty: u32) -> u16 {
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
        let flat_initial: HashSet<u128> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();

        // Step 22 times using flat stepping (the correct reference)
        let mut flat = flat_initial.clone();
        for _ in 0..22 {
            flat = step_flat(&flat);
        }
        println!("\n=== Pi heptamino at generation 22 (flat reference) ===");
        println!("Alive cells ({}):", flat.len());
        let mut cells: Vec<(u64, u64)> = flat.iter().map(|&c| unpack(c)).collect();
        cells.sort();
        for (x, y) in &cells {
            println!("  ({}, {})", x, y);
        }

        // Build a fresh HashLife from gen-22 flat state
        let flat_vec: Vec<u128> = flat.iter().copied().collect();
        let hf = HashLife::from_flat(&flat_vec);
        println!("\nHashLife: depth={} center={:?} size={}", hf.depth, hf.center, hf.size());

        // Find all level-3 nodes in the tree and their absolute positions
        let origin_x = hf.center.0 - (hf.size() as i64 / 2);
        let origin_y = hf.center.1 - (hf.size() as i64 / 2);
        let mut l3_nodes: Vec<(u32, i64, i64)> = Vec::new();
        find_level3_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l3_nodes);

        // Filter to level-3 nodes that contain at least one alive cell (non-empty)
        let nonempty: Vec<(u32, i64, i64)> = l3_nodes.iter().filter(|(idx, _, _)| {
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
        let mut l4_nodes: Vec<(u32, i64, i64)> = Vec::new();
        find_level4_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l4_nodes);
        let l4_nonempty: Vec<(u32, i64, i64)> = l4_nodes.iter().filter(|(idx, _, _)| {
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
            let result_idx = advance_node(&mut test_cache, &mut AHashMap::new(), &mut AHashMap::new(), copied, 4, 0, &mut 0, &mut 0);
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
        let hf2_result: HashSet<u128> = hf2.to_flat().into_iter().collect();
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
        let cells: Vec<u128> = vec![
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
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

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
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

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

    #[test]
    fn test_freelist_gc() {
        // Test that freelist doesn't corrupt patterns across GC boundaries
        // Acorn pattern RLE: bo5b$3bo3b$2o2b3o!
        let pattern = [
            (101, 100), (103, 101),
            (100, 102), (101, 102), (104, 102), (105, 102), (106, 102),
        ];
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance 5000 gens
        let mut hf1 = HashLife::from_flat(&cells);
        hf1.step_n(5000);
        let cells1 = hf1.to_flat();

        // Advance 5000 gens using single steps
        let mut hf2 = HashLife::from_flat(&cells);
        for _ in 0..5000 {
            hf2.step();
        }
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Freelist GC corruption");
    }

    #[test]
    fn test_block_population() {
        // An all-alive 2x2 ("block") compresses to a TRUE node at depth 1,
        // a full 8x8 root compresses to a TRUE node at depth 3. These
        // validate the 1 << (2*depth) formula (the old 1 << depth counted
        // the block as 2 and the 8x8 as 8).
        let pattern = [(100u64, 100u64), (101, 100), (100, 101), (101, 101)];
        let cells: Vec<u128> = pattern.iter().map(|&(x, y)| coord_pack(x, y)).collect();
        let mut hf = HashLife::from_flat(&cells);
        assert_eq!(hf.alive_count(), 4, "fresh block population");
        // The block is a still life: population must hold across gc
        for g in 1..=3u32 {
            hf.step();
            assert_eq!(hf.alive_count(), 4, "block population after gen {}", g);
        }
        // Full 8x8 (depth-3 TRUE root) = 64 cells
        let mut full: Vec<u128> = Vec::new();
        for y in 0..8u64 {
            for x in 0..8u64 {
                full.push(coord_pack(100 + x, 100 + y));
            }
        }
        let mut hf2 = HashLife::from_flat(&full);
        assert_eq!(hf2.alive_count(), 64, "full 8x8 population");
    }

    #[test]
    fn test_align_viewport_scale1() {
        let center = (100i64, 200i64);
        let (hvx_a, hvy_a, vw_a, vh_a) = align_viewport(center, 12345, 67890, 100, 100, 1);
        // Vertical seam byte-aligned: (center.0 - hvx_a) % 8 == 0
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(8), 0, "vertical seam not byte-aligned");
        // Row length multiple of 8
        assert_eq!(vw_a % 8, 0, "vw_a not multiple of 8");
        // Aligned viewport is a superset of the requested one
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvy_a <= 67890, "hvy_a should be <= hvy");
        assert!(hvx_a + vw_a as u64 >= 12345 + 100, "aligned viewport should cover requested x");
        assert!(hvy_a + vh_a as u64 >= 67890 + 100, "aligned viewport should cover requested y");
    }

    #[test]
    fn test_align_viewport_scale2() {
        let center = (100i64, 200i64);
        let (hvx_a, hvy_a, vw_a, vh_a) = align_viewport(center, 12345, 67890, 100, 100, 2);
        // Vertical seam: (center.0 - hvx_a) % (8*scale) == 0
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(16), 0, "vertical seam not aligned (scale=2)");
        // Horizontal seam: (center.1 - hvy_a) % scale == 0
        assert_eq!((center.1 - hvy_a as i64).rem_euclid(2), 0, "horizontal seam not aligned (scale=2)");
        // Agg dimensions multiple of 8
        assert_eq!(((vw_a as u64 + 1) / 2) % 8, 0, "agg_w not multiple of 8 (scale=2)");
        assert_eq!(((vh_a as u64 + 1) / 2) % 8, 0, "agg_h not multiple of 8 (scale=2)");
        // Aligned viewport is a superset
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvy_a <= 67890, "hvy_a should be <= hvy");
        assert!(hvx_a + vw_a as u64 >= 12345 + 100, "aligned viewport should cover requested x");
        assert!(hvy_a + vh_a as u64 >= 67890 + 100, "aligned viewport should cover requested y");
    }

    #[test]
    fn test_aligned_viewport_small() {
        let cells: Vec<u128> = vec![coord_pack(100, 100)];
        let hf = HashLife::from_flat(&cells);
        // 100x100 = 10000 cells = 1250 bytes < 64KB → no alignment
        let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(12345, 67890, 100, 100, 1);
        assert_eq!((hvx_a, hvy_a, vw_a, vh_a), (12345, 67890, 100, 100), "small viewport should be unchanged");
    }

    #[test]
    fn test_aligned_viewport_large() {
        // 16x16 area → depth 4, center (8, 8)
        let mut cells = Vec::new();
        for x in 0..16u64 {
            for y in 0..16u64 {
                cells.push(coord_pack(x, y));
            }
        }
        let hf = HashLife::from_flat(&cells);
        assert!(hf.depth >= 3, "tree should be depth >= 3, got {}", hf.depth);
        // 1000x1000 = 1M cells = 125000 bytes > 64KB → aligned
        let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(12345, 67890, 1000, 1000, 1);
        let center = hf.center;
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(8), 0, "vertical seam not byte-aligned");
        assert_eq!(vw_a % 8, 0, "vw_a not multiple of 8");
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvx_a + vw_a as u64 >= 12345 + 1000, "aligned viewport should cover requested x");
    }

    // ---- Candidate 4: exact-bitmap tests for the TRUE_NODE fill path ----

    #[test]
    fn test_true_fill_exact_small_aligned() {
        // Full 32x32 block -> root (depth 5) is a TRUE node; 128-byte buffer
        // -> single path. The whole viewport must be all ones.
        let mut cells = Vec::new();
        for y in 0..32u64 { for x in 0..32u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 32, 32, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        assert_eq!(bits, vec![0xFFu8; bits.len()], "32x32 true block must fill viewport fully");
    }

    #[test]
    fn test_true_fill_exact_offset_head_tail() {
        // 32x32 block offset by (3,5) in a 30x30 viewport: exercises the
        // head/tail byte masks and scalar middle remainder.
        let mut cells = Vec::new();
        for y in 0..32u64 { for x in 0..32u64 { cells.push(coord_pack(x + 3, y + 5)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 30, 30, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        let mut exp = vec![0u8; bits.len()];
        // Block occupies tree coords [3,35)x[5,35); expected region is the
        // intersection with the viewport, relative to the returned origin.
        // Note: vw=30 here (no alignment under 64KB) so rows use a 30-bit
        // stride — the expected bit position is (y*vw+x) absolute, not
        // (x & 7) within its row byte.
        let x0 = 3u64.max(vx) - vx;
        let x1 = 35u64.min(vx + vw as u64) - vx;
        let y0 = 5u64.max(vy) - vy;
        let y1 = 35u64.min(vy + vh as u64) - vy;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = y * vw as u64 + x;
                exp[(p as usize) >> 3] |= 1 << (p & 7);
            }
        }
        assert_eq!(bits, exp, "offset true block: head/tail byte masks wrong");
    }

    #[test]
    fn test_true_fill_exact_single_path_sse2() {
        // 256x256 full block in a 256x256 viewport: 8KB buffer (single path),
        // 32 bytes per row -> middle run of 30 bytes >= 16 -> SSE2 executes.
        let mut cells = Vec::new();
        for y in 0..256u64 { for x in 0..256u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 256, 256, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        assert_eq!(bits, vec![0xFFu8; bits.len()], "256x256 true block (SSE2 middle) must be all 0xFF");
    }

    #[test]
    fn test_true_fill_exact_parallel_sse2() {
        // 256x256 full block in a 1024x1024 viewport: 128KB buffer ->
        // 4-thread parallel path; the block region must be all ones and the
        // rest of the viewport must stay zero.
        let mut cells = Vec::new();
        for y in 0..256u64 { for x in 0..256u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 1024, 1024, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        let mut exp = vec![0u8; bits.len()];
        let x0 = 0u64.max(vx) - vx;
        let x1 = 256u64.min(vx + vw as u64) - vx;
        let y0 = 0u64.max(vy) - vy;
        let y1 = 256u64.min(vy + vh as u64) - vy;
        for y in y0..y1 {
            for x in x0..x1 {
                exp[((y * vw as u64 + x) as usize) >> 3] |= 1 << (x as u32 & 7);
            }
        }
        assert_eq!(bits, exp, "parallel-path true fill: block must be 0xFF, rest zero");
    }
}
