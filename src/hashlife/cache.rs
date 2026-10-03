//! Node arena, freelist, and HashLifeCache (find_or_create, walks, gc).
use ahash::AHashMap;
use std::collections::VecDeque;
use std::sync::{mpsc, atomic::{AtomicU8, Ordering}};

/// `Zeroed`), so `vec![...; n]` / `repeat().take()` / `repeat_n` can't fill
/// one — write them in place.
pub fn extend_zeros(v: &mut Vec<AtomicU8>, n: usize) {
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

/// Bitmasks for extracting 2x2 quadrants from a 16-bit 4x4 grid.
pub const MASK_NW: u16 = 0xCC00;
pub const MASK_NE: u16 = 0x3300;
pub const MASK_SW: u16 = 0x00CC;
pub const MASK_SE: u16 = 0x0033;


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
pub enum AllocState { Index, Reuse }


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
