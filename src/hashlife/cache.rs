//! Node arena, freelist, and HashLifeCache (find_or_create, walks, gc).
use ahash::AHashMap;
use std::collections::VecDeque;
use std::sync::{mpsc, atomic::{AtomicU8, Ordering}};

/// FALSE_NODE = index 0 (all children 0 = empty)
pub const FALSE_NODE: u32 = 0;

/// TRUE_NODE = index 1 (static alive leaf, children all TRUE_NODE)
pub const TRUE_NODE: u32 = 1;

/// Bitmasks for extracting 2x2 quadrants from a 16-bit 4x4 grid.
pub const MASK_NW: u16 = 0xCC00;
pub const MASK_NE: u16 = 0x3300;
pub const MASK_SW: u16 = 0x00CC;
pub const MASK_SE: u16 = 0x0033;

/// Worker queues for walk_tree_roundrobin. Tunable without a rebuild:
/// GOL_WALK_THREADS=n. Default is **physical cores - 1** (7 on an 8-core box),
/// from `hashlife::physical_cores()` — the same core count the classic engine
/// uses. Latched on first call (OnceLock) so it must be set before the process
/// starts; values < 1 are rejected and fall back to the default.
/// Expected optimum is 4..8 (8 physical cores; the walk's speedup plateaus
/// around 4 workers). Storage does NOT scale with n: each queue is reserved
/// `arena.len()/n + 8`, so total queue capacity is about the arena size at any
/// count — the variable changes only how many threads split the walk.
/// User-facing: see docs/environment.md.
pub fn walk_threads() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("GOL_WALK_THREADS").ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or_else(|| super::physical_cores().saturating_sub(1).max(1))
    })
}


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
    /// within it. Slots freed at GC are zeroed to EMPTY_NODE. A slot is freeable
    /// iff it is not live: gc() drops the previous GC's recovered buffers before
    /// scanning, so a dead slot can only be collected once.
    pub nodes: Vec<LifeNode>,
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
        // `nodes` lazily as new high-water indices are allocated.
        let nodes = vec![
            EMPTY_NODE,
            LifeNode { north_west: 1, north_east: 1, south_west: 1, south_east: 1, is_empty: false },
        ];
        // Index 0 = FALSE_NODE
        arena.insert([0,0,0,0], 0);
        // Index 1 = TRUE_NODE
        arena.insert([1,1,1,1], 1);
        Self {
            arena,
            nodes,
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
                        let i = self.next_idx;
                        self.next_idx += 1;
                        i
                    }
                },
            },
        };
        let node = LifeNode { north_west: nw, north_east: ne, south_west: sw, south_east: se, is_empty };
        // Dense storage: lazily extend `nodes` on a new high-water idx (a fresh
        // index can jump ahead of the Vec end). This runs the same for fresh
        // (Index) and reused (Recovered) indices.
        let i = idx as usize;
        if i >= self.nodes.len() {
            self.nodes.resize(i+1, EMPTY_NODE);
        }
        self.nodes[i] = node;
        self.arena.insert(key, idx);
        idx
    }

    /// C variant, worker half: walk one root's subtree with a private stack.
    /// Roots are tree quadrants and slow-cache keys, so one queue can mix tree
    /// and slow-cache roots (uniform code on every worker).
    fn walk_node(&self, root: u32, live: &[AtomicU8]) {
        let mut stack = vec![ root ];
        while let Some(idx) = stack.pop() {
            let i = idx as usize;
            if live[i].load(Ordering::Relaxed) != 0 { continue; }
            live[i].store(1, Ordering::SeqCst);
            if let Some(node) = self.nodes.get(i) {
                if node.is_empty { continue; }
                stack.push(node.north_west);
                stack.push(node.north_east);
                stack.push(node.south_west);
                stack.push(node.south_east);
            }
        }
    }

    /// C variant: one flat root list (4 quadrants + every slow_cache_n and
    /// slow_cache_n1 key), dealt round-robin into walk_threads() queues, one
    /// rayon job per queue. Balances by entry count instead of by tree shape.
    /// Round-robin is acceptable here because most roots are slow_cache
    /// entries whose arena indices are already scattered, so the quadrant
    /// split buys little locality anyway.
    /// NOTE: the queue storage (~|n|+|n1| u32) is allocated per call and is
    /// inside the measured time — reuse it if this variant wins.
    pub fn walk_tree_roundrobin(&self, root: u32, slow_cache_n1: &AHashMap<u64, u32>, slow_cache_n: &AHashMap<u64, u32>) -> Vec<AtomicU8> {
        let root_node = self.get_node(root);
        let children = [
            root_node.north_west,
            root_node.north_east,
            root_node.south_west,
            root_node.south_east,
        ];

        let live_size = self.nodes.len().max(2);
        // Zeroed live bitvector. AtomicU8 is not Clone (and is !From<Vec<u8>>),
        // so the fill goes through a u8 buffer: `vec![0u8; n]` lowers to a single
        // __rust_alloc_zeroed (fresh pages skip the fill entirely), and the
        // elementwise map is proven layout-preserving, so it folds away — one
        // allocation, no copy, no unsafe. The zeros are load-bearing: the gc free
        // scan and the live_len sum read every slot.
        let live: Vec<AtomicU8> = vec![0u8; live_size].into_iter().map(AtomicU8::from).collect();
        live[root as usize].store(1, Ordering::SeqCst);

        // Round-robin deal: root i -> queue i % nt. Counts stay within one of
        // each other, so no worker is left holding most of the work.
        let nt = walk_threads();
        let mut queues: Vec<Vec<u32>> = vec![ Vec::with_capacity(self.arena.len() / nt + 8); nt ];
        let mut rr = 0usize;
        {
            let mut push = |idx: u32| {
                queues[rr].push(idx);
                rr += 1;
                if rr == nt { rr = 0; }
            };
            for &c in &children { push(c); }
            for (&key, _) in slow_cache_n.iter() { push((key >> 32) as u32); }
            for (&key, _) in slow_cache_n1.iter() { push((key >> 32) as u32); }
        }

        rayon::scope(|scope| {
            let live = &live;
            for q in queues {
                scope.spawn(move |_| {
                    for &r in &q {
                        self.walk_node(r, live);
                    }
                });
            }
        });

        live
    }

    /// Garbage collect: walk tree from root, retain only live nodes.
    /// Also walks subtrees of slow_cache_n1 and fast_cache referenced nodes.
    pub fn gc(&mut self, root: u32, slow_cache_n1: &mut AHashMap<u64, u32>, slow_cache_n: &mut AHashMap<u64, u32>) -> u32 {
        // Walk: round-robin queues. Measured winner over both other variants
        // (Spiral growth, 400 gc calls: rr7 18.7 ms median vs threaded 52.3 and
        // serial 67.2; at >=2M nodes 19.9 vs 53.4 / 72.7), and confirmed by Ed
        // in the UI. p_acorn 5000 gens pinned serial vs rr produced identical
        // node/alive counts and identical bbox.
        // The serial and threaded variants these were measured against were
        // removed 2026-10-11; the numbers above are the record of that A/B.
        let live = self.walk_tree_roundrobin(root, slow_cache_n1, slow_cache_n);
        // Pending diag: per-gc walk timing. Measured 2026-10-10 on p_126m via the
        // browser path: ~19 ms total per 1M gens (61 calls at step 16384) at both
        // rr6 and rr12 = 0.2% of the segment, so the walk is not a throughput
        // lever at this pattern size. Uncomment to re-measure; t0 must stay ABOVE
        // the walk. The rr field is what proves GOL_WALK_THREADS=n reached the code.
        // let t0 = std::time::Instant::now();
        // eprintln!("[walk] note_idx={} arena={} ({:.1} MB) rr{} {:.1} us",
        //     self.nodes.len(), self.arena.len(),
        //     self.nodes.len() as f64 * std::mem::size_of::<LifeNode>() as f64 / 1e6,
        //     walk_threads(), t0.elapsed().as_nanos() as f64 / 1e3);

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
        // The `issued` bitvector is gone (2026-10-11): the scan treats every
        // non-live slot as freeable, so the recovered buffers left over from the
        // previous GC are dropped FIRST — a dead slot that was never reused would
        // otherwise be collected twice and the same index handed to two nodes.
        // Those buffers only ever hold dead slots, and a dead slot is re-discovered
        // by this scan, so nothing is lost. Resetting to Index also means this GC's
        // recovered buffer goes straight into `freed` for immediate reuse.
        self.freed.clear();
        self.reuse.clear();
        self.alloc = AllocState::Index;
        let live_ref: &Vec<AtomicU8> = &live;
        // Retain phase runs on the rayon pool, not fresh OS threads. It used to be
        // std::thread::scope, which paid 7 create+join pairs per gc call (~25-40 us
        // each, x 61 gc calls per 1M gens at step 16384) and oversubscribed the box:
        // the 16 pool workers are still alive after the walk, plus 7 fresh threads on
        // 16 logical CPUs. On the pool these tasks are also work-stolen, which matters
        // because their costs are unequal (full-arena free scan vs a count_cache
        // retain). INVARIANT: gc() must not be called from inside a rayon worker --
        // the recv() calls below block a thread while the pool drains these jobs.
        // gc() is main/server-thread only today.
        let (tx1, rx1) = mpsc::channel::<Vec<u32>>();
        let (tx2, rx2) = mpsc::channel::<u64>();
        let nodes = &mut self.nodes;
        let fast_cache = &mut self.fast_cache;
        let arena = &mut self.arena;
        let count_cache = &mut self.count_cache;
        rayon::scope(|s| {
            // Sending Gc msg to worker directly (modified to just add to front) is 3-4% slower
            // It is critical to performance to use freed nodes asap avoiding 'new' nodes...
            s.spawn(move |_| {
                // Dense Vec: no extract (removing an element would shift
                // every index). Scan instead. A slot is freeable iff it is NOT
                // live: the previous GC's recovered buffers were dropped before
                // this scope, so a dead slot is collected exactly once. Dead
                // slots are zeroed so a freed slot never reads as a live node
                // (the map's extract_if made freed slots simply absent).
                let mut fv: Vec<u32> = Vec::new();
                for i in 0..nodes.len() {
                    // Bitvector: direct byte load (the old shared-AHashSet
                    // contains was a hash probe per slot). Guarded get in case
                    // the Vec ever grew past the bitvector (shouldn't happen
                    // mid-gc; out-of-range reads as dead).
                    if live_ref.get(i).map_or(true, |b| b.load(Ordering::Relaxed) == 0) {
                        // Free: zero the slot. Double-collection is prevented by
                        // dropping the recovered buffers at the top of gc(), not
                        // by a per-slot bit.
                        nodes[i] = EMPTY_NODE;
                        fv.push(i as u32);
                    }
                }
                let _ = tx1.send(fv);
            });

            // Task: remove dead entries from fast_cache
            s.spawn(|_| {
                fast_cache.retain(|&(nidx, _), ridx| live_ref.get(nidx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Task: drop slow_cache_n entries whose key node or result is
            // not live (restore of the f6d4df2 retain; dca4da7 dropped it).
            // Stale keys are never marked live, so their entries are pruned
            // here. The `move` moves the `&mut` param into this one thread; the
            // scope owns its lifetime.
            s.spawn(move |_| {
                slow_cache_n.retain(|&key, ridx| live_ref.get((key >> 32) as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // and n1
            s.spawn(move |_| {
                slow_cache_n1.retain(|&key, ridx| live_ref.get((key >> 32) as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0) && live_ref.get(*ridx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Task: remove dead entries from arena, add sentinels
            s.spawn(|_| {
                arena.retain(|_, idx| live_ref.get(*idx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
                arena.entry([0,0,0,0]).or_insert(0);
                arena.entry([1,1,1,1]).or_insert(1);
            });

            // Task: retain live entries in count_cache
            s.spawn(|_| {
                count_cache.retain(|&(nidx, _), _| live_ref.get(nidx as usize).map_or(false, |b| b.load(Ordering::Relaxed) != 0));
            });

            // Task: sum the live bitvector (plain Relaxed loads — all
            // marks are done). Overlaps with the retains instead of running
            // sequentially before them.
            s.spawn(move |_| {
                let len: u64 = live_ref.iter().map(|b| b.load(Ordering::Relaxed) as u64).sum();
                let _ = tx2.send(len);
            });

        });
        let freed_vec = rx1.recv().unwrap();
        let live_len = rx2.recv().unwrap();
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
