use std::hash::{Hasher, BuildHasher};

/// Union: same 8 bytes viewed as packed key OR two u32 coords.
/// Extracts x,y from a single register read — no shifts/masks needed.
#[repr(C)]
pub union Coord {
    pub packed: u64,
    pub xy: (u32, u32),
}

impl Coord {
    #[inline]
    pub fn unpack(packed: u64) -> (u32, u32) {
        unsafe { Coord { packed }.xy }
    }

    #[inline]
    pub fn pack(x: u32, y: u32) -> u64 {
        let c = Coord { xy: (x, y) };
        unsafe { c.packed }
    }
}

/// Zig-style hash: (x*x >> shift) ^ (y*y >> shift << order_bits)
/// Fast multiplication-based hash that spreads u32 coordinates well.
const LIFE_HASH_SHIFT: u32 = 19; // matches typical Zig table sizes (~1000 active tiles)

#[derive(Clone)]
pub struct LifeHasher {
    buf: u64,
}

impl Hasher for LifeHasher {
    fn write(&mut self, _bytes: &[u8]) {} // not used with integer keys

    fn write_u32(&mut self, n: u32) {
        self.buf = n as u64;
    }

    fn write_u64(&mut self, n: u64) {
        self.buf = n;
    }

    fn finish(&self) -> u64 {
        let (x, y) = Coord::unpack(self.buf);
        // Zig-style hash: multiplication spreads bits, shift extracts high bits
        ((x.wrapping_mul(x) >> LIFE_HASH_SHIFT) as u64) ^
        (((y.wrapping_mul(y) >> LIFE_HASH_SHIFT) as u64) << 13)
    }
}

#[derive(Clone, Default)]
pub struct LifeBuildHasher;

impl BuildHasher for LifeBuildHasher {
    type Hasher = LifeHasher;

    fn build_hasher(&self) -> LifeHasher {
        LifeHasher { buf: 0 }
    }
}

/// HashSet using Zig-style hash (much faster than SipHash)
pub type LifeHashSet<K> = std::collections::HashSet<K, LifeBuildHasher>;

/// HashMap using Zig-style hash
pub type LifeHashMap<K, V> = std::collections::HashMap<K, V, LifeBuildHasher>;

pub const STATIC_SIZE: u32 = 4;
pub const NEIGHBOR_OFFSETS: [(i32, i32); 8] =
    [(-1,-1),(-1,0),(-1,1),(0,-1),(0,1),(1,-1),(1,0),(1,1)];

/// Resizable bloom filter — power-of-2 size, 2 hashes, ~3% FP rate
/// Power-of-2 eliminates division (uses bitwise AND). Single hash + split saves multiplies.
pub struct BloomFilter {
    pub bits: Vec<u64>,
    pub size_bits: usize,
    mask: usize, // size_bits - 1 (power of 2)
}

impl BloomFilter {
    /// Round up to next power of 2
    #[inline]
    fn next_power_of_2(n: usize) -> usize {
        if n <= 1 { return 1; }
        1usize.saturating_mul(2).pow(64 - n.leading_zeros())
    }

    /// Resize bloom filter for `expected_elements` with ~3% false positive rate
    /// With k=2 hashes: m ≈ 11 * n gives ~3% FP rate
    pub fn resize(&mut self, expected_elements: usize) {
        if expected_elements == 0 {
            self.bits.clear();
            self.size_bits = 0;
            self.mask = 0;
            return;
        }
        // m = 8 * n for ~5% FP with k=2, rounded up to power of 2
        let raw_bits = expected_elements.saturating_mul(8);
        let size_bits = Self::next_power_of_2(raw_bits);
        let size_u64 = size_bits / 64;

        if self.bits.len() != size_u64 {
            self.bits = vec![0u64; size_u64];
        } else {
            self.bits.iter_mut().for_each(|w| *w = 0);
        }
        self.size_bits = size_bits;
        self.mask = size_bits - 1;
    }

    #[inline]
    pub fn insert(&mut self, key: u64) {
        if self.bits.is_empty() { return; }
        let (x, y) = Coord::unpack(key);
        let h = ((x.wrapping_mul(x) >> 19) as u32) ^ (((y.wrapping_mul(y) >> 19) as u32) << 13);
        let h1 = h as usize;
        let h2 = (h >> 16) as usize;
        let m = self.mask;

        let bit1 = h1 & m;
        self.bits[bit1 >> 6] |= 1u64 << (bit1 & 63);

        let bit2 = h2 & m;
        self.bits[bit2 >> 6] |= 1u64 << (bit2 & 63);
    }

    #[inline]
    pub fn contains(&self, key: u64) -> bool {
        if self.bits.is_empty() { return false; }
        let (x, y) = Coord::unpack(key);
        let h = ((x.wrapping_mul(x) >> 19) as u32) ^ (((y.wrapping_mul(y) >> 19) as u32) << 13);
        let h1 = h as usize;
        let h2 = (h >> 16) as usize;
        let m = self.mask;

        let bit1 = h1 & m;
        if self.bits[bit1 >> 6] & (1u64 << (bit1 & 63)) == 0 {
            return false;
        }

        let bit2 = h2 & m;
        self.bits[bit2 >> 6] & (1u64 << (bit2 & 63)) != 0
    }
}

/// Groups cells by target tile — one active_tiles.contains() check per unique tile.
#[derive(Clone, Copy)]
pub struct TileNbrGroup {
    pub tdx: i16,
    pub tdy: i16,
    pub count: u8,
    pub cells: [(u8, u8); 3],
}

const TILE_NBR_GROUP_ZERO: TileNbrGroup = TileNbrGroup { tdx: 0, tdy: 0, count: 0, cells: [(0,0), (0,0), (0,0)] };

#[derive(Clone, Copy)]
pub struct TileNbrInfo {
    pub num_groups: u8,
    pub groups: [TileNbrGroup; 3],
}

const TILE_NBR_INFO_ZERO: TileNbrInfo = TileNbrInfo { num_groups: 0, groups: [TILE_NBR_GROUP_ZERO; 3] };

// Compile-time guard: TABLE is hardcoded for STATIC_SIZE == 4
const _: () = assert!(STATIC_SIZE == 4);

pub const TILE_NBR_MASK: [[TileNbrInfo; 4]; 4] = [
    // mx=0 (left edge)
    [
        TileNbrInfo{num_groups:3,groups:[
            TileNbrGroup{tdx:-4,tdy:-4,count:1,cells:[(3,3),(0,0),(0,0)]},
            TileNbrGroup{tdx:-4,tdy: 0,count:2,cells:[(3,0),(3,1),(0,0)]},
            TileNbrGroup{tdx: 0,tdy:-4,count:2,cells:[(0,3),(1,3),(0,0)]}]},
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx:-4,tdy: 0,count:3,cells:[(3,0),(3,1),(3,2)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx:-4,tdy: 0,count:3,cells:[(3,1),(3,2),(3,3)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TileNbrInfo{num_groups:3,groups:[
            TileNbrGroup{tdx:-4,tdy: 0,count:2,cells:[(3,2),(3,3),(0,0)]},
            TileNbrGroup{tdx:-4,tdy: 4,count:1,cells:[(3,0),(0,0),(0,0)]},
            TileNbrGroup{tdx: 0,tdy: 4,count:2,cells:[(0,0),(1,0),(0,0)]}]},
    ],
    // mx=1
    [
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 0,tdy:-4,count:3,cells:[(0,3),(1,3),(2,3)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TILE_NBR_INFO_ZERO,
        TILE_NBR_INFO_ZERO,
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 0,tdy: 4,count:3,cells:[(0,0),(1,0),(2,0)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
    ],
    // mx=2
    [
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 0,tdy:-4,count:3,cells:[(1,3),(2,3),(3,3)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TILE_NBR_INFO_ZERO,
        TILE_NBR_INFO_ZERO,
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 0,tdy: 4,count:3,cells:[(1,0),(2,0),(3,0)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
    ],
    // mx=3 (right edge)
    [
        TileNbrInfo{num_groups:3,groups:[
            TileNbrGroup{tdx: 0,tdy:-4,count:2,cells:[(2,3),(3,3),(0,0)]},
            TileNbrGroup{tdx: 4,tdy:-4,count:1,cells:[(0,3),(0,0),(0,0)]},
            TileNbrGroup{tdx: 4,tdy: 0,count:2,cells:[(0,0),(0,1),(0,0)]}]},
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 4,tdy: 0,count:3,cells:[(0,0),(0,1),(0,2)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TileNbrInfo{num_groups:1,groups:[
            TileNbrGroup{tdx: 4,tdy: 0,count:3,cells:[(0,1),(0,2),(0,3)]},
            TILE_NBR_GROUP_ZERO, TILE_NBR_GROUP_ZERO]},
        TileNbrInfo{num_groups:3,groups:[
            TileNbrGroup{tdx: 0,tdy: 4,count:2,cells:[(2,0),(3,0),(0,0)]},
            TileNbrGroup{tdx: 4,tdy: 0,count:2,cells:[(0,2),(0,3),(0,0)]},
            TileNbrGroup{tdx: 4,tdy: 4,count:1,cells:[(0,0),(0,0),(0,0)]}]},
    ],
];

/// Conventional (cell-list) engine state — bit-accurate vs the HashLife quadtree.
pub struct Classic {
    /// Contiguous Vec for parallel chunked iteration
    pub alive: Vec<u64>,
    /// Maps key -> Vec index for O(1) swap-remove deletes
    pub alive_index: LifeHashMap<u64, usize>,
    pub heap: u32,
    pub active_tiles: LifeHashSet<u64>,
    pub active_count: u32,
    /// alive_vec.len() / active_tiles.len() — FP rate proxy
    pub active_ratio: f64,
    // Pre-allocated buffer for step() — reused across generations to avoid allocations
    pub(crate) apply_new_active: LifeHashSet<u64>,
    // Pre-allocated flat buffer for step() — contiguous slices distributed to workers
    pub(crate) alive_vec: Vec<u64>,
    // Pre-allocated buffers for births/deaths — cleared and reused each generation
    pub(crate) births_buf: Vec<u64>,
    pub(crate) deaths_buf: Vec<u64>,
    // Bloom filter for active tile keys — replaces HashSet.contains for speed
    pub(crate) active_bloom: BloomFilter,
}

/// The two stepping engines. Grid holds exactly one at a time.
pub enum Engine {
    Classic(Classic),
    HashLife(game_of_life::hashlife::HashLife),
}

/// Shared server state: the generation counter + header stats both engines
/// write, plus the live engine.
pub struct Grid {
    pub generation: u32,
    pub births: u32,
    pub deaths: u32,
    pub engine: Engine,
}

impl Grid {
    pub fn new() -> Self {
        Self {
            generation: 0,
            births: 0,
            deaths: 0,
            engine: Engine::HashLife(game_of_life::hashlife::HashLife::new()),
        }
    }

    #[inline]
    pub fn is_hashlife(&self) -> bool {
        self.engine.is_hashlife()
    }

    /// Switch engines, converting state in whichever direction the toggle goes.
    pub fn toggle_engine(&mut self) {
        match std::mem::replace(&mut self.engine, Engine::Classic(Classic::new())) {
            Engine::Classic(c) => self.engine = Engine::HashLife(c.to_hashlife()),
            Engine::HashLife(hf) => self.engine = Engine::Classic(Classic::from_hashlife(&hf)),
        }
    }

    /// Clear the grid (both engines reset generation/births/deaths).
    pub fn clear(&mut self) {
        self.generation = 0;
        self.births = 0;
        self.deaths = 0;
        self.engine.clear();
    }

    /// Randomize a box around (cx, cy). Classic resets generation (old
    /// behavior); HashLife does not.
    pub fn randomize(&mut self, cx: i64, cy: i64, size: i64, density: f64) {
        if !self.is_hashlife() {
            self.generation = 0;
        }
        self.engine.randomize(cx, cy, size, density);
    }
}

impl Classic {
    pub fn new() -> Self {
        Self {
            alive: Vec::with_capacity(256),
            alive_index: LifeHashMap::with_capacity_and_hasher(256, LifeBuildHasher),
            heap: 0,
            active_tiles: std::collections::HashSet::with_hasher(LifeBuildHasher),
            active_count: 0,
            active_ratio: 0.0,
            apply_new_active: LifeHashSet::with_capacity_and_hasher(256, LifeBuildHasher),
            alive_vec: Vec::new(),
            births_buf: Vec::with_capacity(256),
            deaths_buf: Vec::with_capacity(256),
            active_bloom: BloomFilter { bits: Vec::new(), size_bits: 0, mask: 0 },
        }
    }

    /// Pack x,y (as u32) into a single u64 key.
    #[inline]
    pub fn k(x: u32, y: u32) -> u64 {
        Coord::pack(x, y)
    }

    /// Unpack a u64 key back to (x, y) as u32s — no shifts/masks.
    #[inline]
    pub fn unpack(k: u64) -> (u32, u32) {
        Coord::unpack(k)
    }

    /// Tile coordinate: round down to nearest STATIC_SIZE boundary.
    //#[inline]
    //pub fn tile(x: u32) -> u32 {
    //    x - x % STATIC_SIZE
    //}

    /// tile
    #[inline]
    pub fn mod_tile(k: u64) -> (u32, u32) {
       let (x, y) = Coord::unpack(k);
       ( x % STATIC_SIZE,  y % STATIC_SIZE)
    }

    /// Insert into alive Vec + index HashMap
    pub fn insert_alive(&mut self, k: u64) {
        self.alive.push(k);
        self.alive_index.insert(k, self.alive.len() - 1);
    }

    /// Remove from alive Vec (swap-remove) + index HashMap
    pub fn remove_alive(&mut self, k: u64) {
        let idx = self.alive_index.remove(&k).unwrap();
        let last = self.alive.len() - 1;
        if idx != last {
            let swapped = self.alive[last];
            self.alive[idx] = swapped;
            self.alive_index.insert(swapped, idx);
        }
        self.alive.pop();
    }

    /// Check if cell is alive
    pub fn contains_alive(&self, k: u64) -> bool {
        self.alive_index.contains_key(&k)
    }

    pub fn mark_active(k: u64, tiles: &mut LifeHashSet<u64>) {
        let (x, y) = Self::unpack(k);
        let (ox, oy) = Self::mod_tile(k);
        let tx = x - ox; 
        let ty = y - oy;

        let ss = STATIC_SIZE;
        let m1 = ss - 1; // SSM1 for u32

        tiles.insert(Coord::pack(tx, ty));

        if ox == m1 {
            tiles.insert(Coord::pack(tx + ss, ty));
        }
        if ox == m1 && oy == m1 {
            tiles.insert(Coord::pack(tx + ss, ty + ss));
        }
        if oy == m1 {
            tiles.insert(Coord::pack(tx, ty + ss));
        }
        if ox == 0 && oy == m1 {
            tiles.insert(Coord::pack(tx - ss, ty + ss));
        }
        if ox == 0 {
            tiles.insert(Coord::pack(tx - ss, ty));
        }
        if ox == 0 && oy == 0 {
            tiles.insert(Coord::pack(tx - ss, ty - ss));
        }
        if oy == 0 {
            tiles.insert(Coord::pack(tx, ty - ss));
        }
        if ox == m1 && oy == 0 {
            tiles.insert(Coord::pack(tx + ss, ty - ss));
        }
    }

    pub(crate) fn init_active(&mut self) {
        self.active_tiles.clear();
        for &k in &self.alive {
            Self::mark_active(k, &mut self.active_tiles);
        }
        // Populate active_bloom from active_tiles
        self.active_bloom.resize(self.active_tiles.len());

        // Populate expanded_bloom with cell-level border coordinates
        let ss = STATIC_SIZE as u32;
        // self.expanded_bloom.resize(self.active_tiles.len() * 15);
        for &tk in &self.active_tiles {
            let (tx, ty) = Self::unpack(tk);

            self.active_bloom.insert(tk);
            self.active_bloom.insert(Coord::pack(tx.wrapping_sub(ss), ty.wrapping_sub(ss)));
            self.active_bloom.insert(Coord::pack(tx.wrapping_sub(ss), ty.wrapping_add(ss)));
            self.active_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_sub(ss)));
            self.active_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(ss)));
            self.active_bloom.insert(Coord::pack(tx, ty.wrapping_sub(ss)));
            self.active_bloom.insert(Coord::pack(tx, ty.wrapping_add(ss)));
            self.active_bloom.insert(Coord::pack(tx.wrapping_sub(ss), ty));
            self.active_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty));

            // Top border (y = ty-1, x = tx-1 .. tx+ss)
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_sub(1)));
            // self.expanded_bloom.insert(Coord::pack(tx, ty.wrapping_sub(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(1), ty.wrapping_sub(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(2), ty.wrapping_sub(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(3), ty.wrapping_sub(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_sub(1)));

            // Bottom border (y = ty+ss, x = tx-1 .. tx+ss)
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(ss)));
            // self.expanded_bloom.insert(Coord::pack(tx, ty.wrapping_add(ss)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(1), ty.wrapping_add(ss)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(2), ty.wrapping_add(ss)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(3), ty.wrapping_add(ss)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(ss)));

            // Left border (x = tx-1, y = ty .. ty+ss-1)
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(2)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(3)));

            // Right border (x = tx+ss, y = ty .. ty+ss-1)
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(1)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(2)));
            // self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(3)));
        }
    }

    pub fn randomize(&mut self, cx: i64, cy: i64, size: i64, density: f64) {
        let half = (size / 2) as u32;
        let mut rng = fastrand::Rng::new();
        self.alive.clear();
        self.alive_index.clear();
        self.active_tiles.clear();
        //self.expanded_bloom.resize(0);
        self.active_bloom.resize(0);
        for dx in -(half as i32)..=(half as i32) {
            for dy in -(half as i32)..=(half as i32) {
                if rng.f64() < density {
                    let x = (cx + dx as i64) as u32;
                    let y = (cy + dy as i64) as u32;
                    self.insert_alive(Self::k(x, y));
                }
            }
        }
        self.init_active();
    }

    pub fn clear(&mut self) {
        self.alive.clear();
        self.alive_index.clear();
        self.active_tiles.clear();
        //self.expanded_bloom.resize(0);
        self.active_bloom.resize(0);
        self.active_count = 0;
        self.active_ratio = 0.0;
        self.heap = 0;
    }

    pub fn toggle(&mut self, x: i64, y: i64) {
        let k = Self::k(x as u32, y as u32);
        if self.contains_alive(k) {
            self.remove_alive(k);
        } else {
            self.insert_alive(k);
        }
        // Sync bloom filters when transitioning from empty/stopped to running
        self.init_active();
    }

    pub fn load_pattern(&mut self, cells: &[(i64, i64)], anchor_x: i64, anchor_y: i64) {
        for &(cx, cy) in cells {
            let x = (cx + anchor_x) as u32;
            let y = (cy + anchor_y) as u32;
            self.insert_alive(Self::k(x, y));
        }
        // Sync bloom filters when transitioning from empty/stopped to running
        self.init_active();
    }

    /// Build a HashLife quadtree from this classic alive set
    /// (classic -> hashlife engine switch).
    pub fn to_hashlife(&self) -> game_of_life::hashlife::HashLife {
        eprintln!("TO_HASHLIFE: building quadtree from {} cells", self.alive.len());
        let cells: Vec<u128> = self.alive.iter().map(|&k| {
            let (x, y) = Coord::unpack(k);
            let (hx, hy) = game_of_life::hashlife::grid_to_hashlife(x as u64, y as u64);
            game_of_life::hashlife::coord_pack(hx, hy)
        }).collect();
        game_of_life::hashlife::HashLife::from_flat(&cells)
    }

    /// Rebuild a classic state from a hashlife quadtree
    /// (hashlife -> classic engine switch).
    pub fn from_hashlife(hf: &game_of_life::hashlife::HashLife) -> Self {
        let alive_u128 = hf.collect_alive();
        let alive: Vec<u64> = alive_u128.iter().filter_map(|&cell| {
            let (hx, hy) = game_of_life::hashlife::coord_unpack(cell);
            let (gx, gy) = game_of_life::hashlife::hashlife_to_grid(hx, hy);
            // Only include cells that fit in u32
            if gx <= u32::MAX as u64 && gy <= u32::MAX as u64 {
                Some(Coord::pack(gx as u32, gy as u32))
            } else {
                None
            }
        }).collect();
        eprintln!("FROM_HASHLIFE: rebuilt {} cells from quadtree", alive.len());
        let mut c = Self::new();
        c.alive = alive;
        for (i, &cell) in c.alive.iter().enumerate() {
            c.alive_index.insert(cell, i);
        }
        c.init_active();
        c
    }
}

impl Engine {
    #[inline]
    pub fn is_hashlife(&self) -> bool {
        matches!(self, Engine::HashLife(_))
    }

    pub fn alive_count(&mut self) -> u32 {
        match self {
            Engine::Classic(c) => c.alive.len() as u32,
            Engine::HashLife(hf) => hf.alive_count() as u32,
        }
    }

    /// (active, heap, tiles) — the three mode-dependent header fields.
    pub fn header_stats(&self) -> (u32, u32, u32) {
        match self {
            Engine::Classic(c) => (c.active_count, c.heap, c.active_tiles.len() as u32),
            Engine::HashLife(hf) => (hf.last_gc_live_len, hf.last_cache_size, hf.last_cache_hit_rate),
        }
    }

    /// Fill the viewport bitmap (+ overlay) for /state. The request is in
    /// frontend space; each engine interprets the coords in its own client
    /// space. Returns (bits, overlay, final_vw, final_vh).
    pub fn snapshot(&self, vx: u64, vy: u64, vw: u32, vh: u32, scale: u32) -> (Vec<u8>, Vec<u8>, u32, u32) {
        match self {
            Engine::HashLife(hf) => {
                let (hvx, hvy) = game_of_life::hashlife::frontend_to_hashlife(vx, vy);
                let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(hvx, hvy, vw, vh, scale);
                let scale_usize = scale as usize;
                let agg_w = (vw_a as usize + scale_usize - 1) / scale_usize;
                let agg_h = (vh_a as usize + scale_usize - 1) / scale_usize;
                let len = (agg_w * agg_h + 7) / 8;
                let mut bits = vec![0u8; len];
                if scale > 1 {
                    hf.populate_aggregated_viewport(hvx_a, hvy_a, vw_a, vh_a, scale, &mut bits);
                } else {
                    hf.populate_viewport(hvx_a, hvy_a, vw_a, vh_a, &mut bits);
                }
                (bits, vec![0u8; len], agg_w as u32, agg_h as u32)
            }
            Engine::Classic(c) => {
                if scale > 1 {
                    let scale_usize = scale as usize;
                    let agg_w = (vw as usize + scale_usize - 1) / scale_usize;
                    let agg_h = (vh as usize + scale_usize - 1) / scale_usize;
                    let agg_len = (agg_w * agg_h + 7) / 8;
                    let mut agg_bits = vec![0u8; agg_len];
                    let mut agg_overlay = vec![0u8; agg_len];

                    for &k in &c.alive {
                        let (ax, ay) = Coord::unpack(k);
                        let ax = ax as u64;
                        let ay = ay as u64;
                        if ax >= vx && ay >= vy {
                            let rx = ax - vx;
                            let ry = ay - vy;
                            if rx < vw as u64 && ry < vh as u64 {
                                let aggx = (rx / scale as u64) as usize;
                                let aggy = (ry / scale as u64) as usize;
                                let aidx = aggy * agg_w + aggx;
                                agg_bits[aidx >> 3] |= 1 << (aidx & 7);
                            }
                        }
                    }

                    let ss = STATIC_SIZE as u64;
                    for &tkey in &c.active_tiles {
                        let (tx, ty) = Coord::unpack(tkey);
                        let tx = tx as u64;
                        let ty = ty as u64;
                        if tx < vx + vw as u64 && tx + ss > vx
                            && ty < vy + vh as u64 && ty + ss > vy {
                            let x_lo = tx.max(vx) - vx;
                            let x_hi = (tx + ss).min(vx + vw as u64) - vx;
                            let y_lo = ty.max(vy) - vy;
                            let y_hi = (ty + ss).min(vy + vh as u64) - vy;
                            let ax_lo = (x_lo / scale as u64) as usize;
                            let ax_hi = ((x_hi - 1) / scale as u64) as usize;
                            let ay_lo = (y_lo / scale as u64) as usize;
                            let ay_hi = ((y_hi - 1) / scale as u64) as usize;
                            for ay in ay_lo..=ay_hi {
                                for ax in ax_lo..=ax_hi {
                                    let idx = ay * agg_w + ax;
                                    agg_overlay[idx >> 3] |= 1 << (idx & 7);
                                }
                            }
                        }
                    }

                    (agg_bits, agg_overlay, agg_w as u32, agg_h as u32)
                } else {
                    let bits_len = (vw as usize * vh as usize + 7) / 8;
                    let mut bits = vec![0u8; bits_len];
                    for &k in &c.alive {
                        let (ax, ay) = Coord::unpack(k);
                        let ax = ax as u64;
                        let ay = ay as u64;
                        if ax >= vx && ay >= vy {
                            let rx = ax - vx;
                            let ry = ay - vy;
                            if rx < vw as u64 && ry < vh as u64 {
                                let idx = (ry * vw as u64 + rx) as usize;
                                bits[idx >> 3] |= 1 << (idx & 7);
                            }
                        }
                    }

                    let ss = STATIC_SIZE as u64;
                    let mut overlay = vec![0u8; bits_len];
                    for &tkey in &c.active_tiles {
                        let (tx, ty) = Coord::unpack(tkey);
                        let tx = tx as u64;
                        let ty = ty as u64;
                        if tx < vx + vw as u64 && tx + ss > vx
                            && ty < vy + vh as u64 && ty + ss > vy {
                            let x_lo = tx.max(vx) - vx;
                            let x_hi = (tx + ss).min(vx + vw as u64) - vx;
                            let y_lo = ty.max(vy) - vy;
                            let y_hi = (ty + ss).min(vy + vh as u64) - vy;
                            for y in y_lo..y_hi {
                                let row_offset = (y * vw as u64) as usize;
                                for x in x_lo..x_hi {
                                    let idx = row_offset + x as usize;
                                    overlay[idx >> 3] |= 1 << (idx & 7);
                                }
                            }
                        }
                    }
                    (bits, overlay, vw, vh)
                }
            }
        }
    }

    /// Toggle a single cell (frontend-space coords).
    pub fn toggle_cell(&mut self, x: i64, y: i64) {
        match self {
            Engine::Classic(c) => c.toggle(x, y),
            Engine::HashLife(hf) => {
                let (hx, hy) = game_of_life::hashlife::frontend_to_hashlife(x as u64, y as u64);
                hf.set_cell(hx, hy, !hf.get_cell(hx, hy));
            }
        }
    }

    /// Randomize a box around (cx, cy).
    pub fn randomize(&mut self, cx: i64, cy: i64, size: i64, density: f64) {
        match self {
            Engine::Classic(c) => c.randomize(cx, cy, size, density),
            Engine::HashLife(hf) => {
                let (hc_x, hc_y) = game_of_life::hashlife::frontend_to_hashlife(cx as u64, cy as u64);
                let mut new = game_of_life::hashlife::HashLife::new();
                let half = size / 2;
                for dx in -half..=half {
                    for dy in -half..=half {
                        if fastrand::f64() < density {
                            new.set_cell(
                                hc_x.wrapping_add(dx as i64 as u64),
                                hc_y.wrapping_add(dy as i64 as u64),
                                true,
                            );
                        }
                    }
                }
                *hf = new;
            }
        }
    }

    /// Clear the engine state (generation/births/deaths are reset by Grid).
    pub fn clear(&mut self) {
        match self {
            Engine::Classic(c) => c.clear(),
            Engine::HashLife(hf) => *hf = game_of_life::hashlife::HashLife::new(),
        }
    }

    /// Load a pattern (frontend-space cells, anchored at the origin).
    pub fn load_pattern(&mut self, cells: &[(i64, i64)], anchor_x: i64, anchor_y: i64) {
        match self {
            Engine::Classic(c) => c.load_pattern(cells, anchor_x, anchor_y),
            Engine::HashLife(hf) => {
                let (ha_x, ha_y) = game_of_life::hashlife::frontend_to_hashlife(anchor_x as u64, anchor_y as u64);
                if hf.is_empty() {
                    let flat: Vec<u128> = cells.iter().map(|&(ccx, ccy)| {
                        let gx = ha_x.wrapping_add(ccx as i64 as u64);
                        let gy = ha_y.wrapping_add(ccy as i64 as u64);
                        game_of_life::hashlife::coord_pack(gx, gy)
                    }).collect();
                    *hf = game_of_life::hashlife::HashLife::from_flat(&flat);
                } else {
                    for &(ccx, ccy) in cells {
                        let x = ha_x.wrapping_add(ccx as i64 as u64);
                        let y = ha_y.wrapping_add(ccy as i64 as u64);
                        hf.set_cell(x, y, true);
                    }
                }
            }
        }
    }

    /// All alive cells in frontend-space coords.
    pub fn frontend_cells(&self) -> Vec<(u64, u64)> {
        match self {
            Engine::Classic(c) => c.alive.iter().map(|&k| {
                let (x, y) = Coord::unpack(k);
                (x as u64, y as u64)
            }).collect(),
            Engine::HashLife(hf) => hf.to_flat().iter().map(|&p| {
                let (hx, hy) = game_of_life::hashlife::coord_unpack(p);
                game_of_life::hashlife::hashlife_to_frontend(hx, hy)
            }).collect(),
        }
    }

    /// Export as MC for large HashLife patterns, RLE otherwise.
    pub fn export_text(&mut self) -> String {
        if let Engine::HashLife(hf) = self {
            if hf.alive_count() > 5000 {
                return hf.export_mc();
            }
        }
        export_rle(&self.frontend_cells())
    }
}

/// Export cells to RLE format.
fn export_rle(cells: &[(u64, u64)]) -> String {
    if cells.is_empty() {
        return "b!".to_string();
    }

    // Find bounds
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (u64::MAX, 0, u64::MAX, 0);
    for &(x, y) in cells {
        if x < min_x { min_x = x; }
        if x > max_x { max_x = x; }
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    // Build a set for O(1) lookup
    let alive: std::collections::HashSet<(u64, u64)> = cells.iter().copied().collect();

    // Encode header
    let mut out = String::new();
    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    out.push_str(&format!("x = {}, y = {}, rule = B3/S23\n", width, height));

    // Encode row by row
    let mut x = min_x;
    let mut y = min_y;
    let mut run_len = 0;
    let mut run_alive = alive.contains(&(min_x, min_y));

    while y <= max_y {
        while x <= max_x {
            let is_alive = alive.contains(&(x, y));
            if is_alive == run_alive {
                run_len += 1;
            } else {
                // Flush current run
                if run_len > 0 {
                    if run_len > 1 { out.push_str(&run_len.to_string()); }
                    out.push(if run_alive { 'o' } else { 'b' });
                }
                run_len = 1;
                run_alive = is_alive;
            }
            x += 1;
        }
        // Flush remaining run for this row
        if run_len > 0 {
            if run_len > 1 { out.push_str(&run_len.to_string()); }
            out.push(if run_alive { 'o' } else { 'b' });
            run_len = 0;
        }
        out.push('$');
        y += 1;
        x = min_x;
    }

    out.push('!');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use game_of_life::hashlife::{coord_unpack, FRONTEND_OFFSET, GRID_OFFSET, HASHLIFE_OFFSET, HashLife};

    fn classic_of(g: &Grid) -> &Classic {
        match &g.engine {
            Engine::Classic(c) => c,
            _ => panic!("expected classic engine"),
        }
    }

    fn hl_of(g: &Grid) -> &HashLife {
        match &g.engine {
            Engine::HashLife(hf) => hf,
            _ => panic!("expected hashlife engine"),
        }
    }

    /// Classic alive set, normalized to origin-centered i64 coords, sorted.
    fn classic_cells(c: &Classic) -> Vec<(i64, i64)> {
        let mut v: Vec<(i64, i64)> = c.alive_index.keys().map(|&k| {
            let (x, y) = Classic::unpack(k);
            (x as i64 - GRID_OFFSET as i64, y as i64 - GRID_OFFSET as i64)
        }).collect();
        v.sort();
        v
    }

    /// HashLife alive set, normalized to origin-centered i64 coords, sorted.
    fn hl_cells(hf: &HashLife) -> Vec<(i64, i64)> {
        let mut v: Vec<(i64, i64)> = hf.to_flat().into_iter().map(|c| {
            let (hx, hy) = coord_unpack(c);
            (hx as i64 - HASHLIFE_OFFSET as i64, hy as i64 - HASHLIFE_OFFSET as i64)
        }).collect();
        v.sort();
        v
    }

    /// Blinker + glider + pi heptamino, spaced far enough apart that they do
    /// not interact within the test's generation span.
    fn engine_test_pattern() -> Vec<(i64, i64)> {
        vec![
            // blinker
            (0, 0), (1, 0), (2, 0),
            // glider
            (40, 0), (41, 1), (39, 2), (40, 2), (41, 2),
            // pi heptamino
            (0, 40), (1, 40), (2, 40), (0, 41), (0, 42), (1, 42), (2, 42),
        ]
    }

    /// Load the same pattern into a fresh Grid; the anchor places the pattern
    /// centered on the origin of the u32 (GRID_OFFSET) space.
    fn make_grid(hashlife_mode: bool) -> Grid {
        let mut c = Classic::new();
        c.load_pattern(&engine_test_pattern(), GRID_OFFSET as i64, GRID_OFFSET as i64);
        if hashlife_mode {
            Grid { generation: 0, births: 0, deaths: 0, engine: Engine::HashLife(c.to_hashlife()) }
        } else {
            Grid { generation: 0, births: 0, deaths: 0, engine: Engine::Classic(c) }
        }
    }

    #[test]
    fn test_engines_agree() {
        const N: u32 = 8;
        let mut classic = make_grid(false);
        let mut hl = make_grid(true);
        // The quadtree was built from the identical classic alive set in
        // make_grid — both engines provably start from the same state.

        for i in 0..N {
            classic.step();
            hl.step();
            assert_eq!(
                classic_cells(classic_of(&classic)),
                hl_cells(hl_of(&hl)),
                "engines diverged at gen {}",
                i + 1
            );
        }
    }

    #[test]
    fn test_engines_agree_with_toggle() {
        const HALF: u32 = 4;
        // Path A: 4 classic steps, toggle to hashlife, 4 hashlife steps.
        let mut a = make_grid(false);
        for _ in 0..HALF {
            a.step();
        }
        a.toggle_engine();
        for _ in 0..HALF {
            a.step();
        }

        // Control: 8 hashlife steps from the same starting state.
        let mut b = make_grid(true);
        for _ in 0..HALF * 2 {
            b.step();
        }

        assert_eq!(hl_cells(hl_of(&a)), hl_cells(hl_of(&b)), "toggle path diverged");
    }

    /// Blinker: period 2 — horizontal at gen 0, vertical at gen 1, back at gen 2.
    #[test]
    fn test_classic_blinker_period() {
        let mut c = Classic::new();
        c.load_pattern(&vec![(0, 0), (1, 0), (2, 0)], GRID_OFFSET as i64, GRID_OFFSET as i64);
        let mut g = Grid { generation: 0, births: 0, deaths: 0, engine: Engine::Classic(c) };
        let gen0 = classic_cells(classic_of(&g));
        assert_eq!(gen0, vec![(0, 0), (1, 0), (2, 0)]);
        g.step();
        assert_eq!(classic_cells(classic_of(&g)), vec![(1, -1), (1, 0), (1, 1)],
            "blinker must be vertical at gen 1");
        g.step();
        assert_eq!(classic_cells(classic_of(&g)), gen0, "blinker period-2 broken");
    }

    /// Glider in this orientation: period 4, translates (+1, +1) per period.
    #[test]
    fn test_classic_glider_translation() {
        let mut c = Classic::new();
        let glider = vec![(40, 0), (41, 1), (39, 2), (40, 2), (41, 2)];
        c.load_pattern(&glider, GRID_OFFSET as i64, GRID_OFFSET as i64);
        let mut g = Grid { generation: 0, births: 0, deaths: 0, engine: Engine::Classic(c) };
        for _ in 0..4 { g.step(); }
        assert_eq!(classic_cells(classic_of(&g)),
            vec![(40, 3), (41, 1), (41, 3), (42, 2), (42, 3)],
            "glider must translate (+1, +1) per 4 generations");
    }

    /// Classic direct manipulation: toggle on/off, clear, and randomize at
    /// the density extremes (1.0 = full box, 0.0 = empty).
    #[test]
    fn test_classic_toggle_clear_randomize() {
        let mut c = Classic::new();
        assert_eq!(c.alive.len(), 0);
        c.toggle(GRID_OFFSET as i64, GRID_OFFSET as i64);
        assert_eq!(c.alive.len(), 1);
        c.toggle(GRID_OFFSET as i64, GRID_OFFSET as i64);
        assert_eq!(c.alive.len(), 0, "toggling the same cell off must empty the grid");
        c.clear();
        assert_eq!(c.alive.len(), 0);
        c.randomize(GRID_OFFSET as i64, GRID_OFFSET as i64, 11, 1.0);
        assert_eq!(c.alive.len(), 11 * 11, "density 1.0 fills the whole box");
        c.randomize(GRID_OFFSET as i64, GRID_OFFSET as i64, 11, 0.0);
        assert_eq!(c.alive.len(), 0, "density 0.0 produces an empty grid");
        c.randomize(GRID_OFFSET as i64, GRID_OFFSET as i64, 3, 1.0);
        assert_eq!(c.alive.len(), 9);
    }

    /// Classic: load_pattern then frontend_cells must return the exact
    /// pattern in its u32 (GRID_OFFSET) space — including negative offsets.
    #[test]
    fn test_classic_load_frontend_cells_roundtrip() {
        let cells = vec![(10, -5), (11, 5), (-7, 9)];
        let mut e = Engine::Classic(Classic::new());
        e.load_pattern(&cells, GRID_OFFSET as i64, GRID_OFFSET as i64);
        let mut got: Vec<(i64, i64)> = e.frontend_cells().iter()
            .map(|&(x, y)| (x as i64 - GRID_OFFSET as i64, y as i64 - GRID_OFFSET as i64))
            .collect();
        got.sort();
        let mut want = cells;
        want.sort();
        assert_eq!(got, want);
    }

    /// HashLife engine: the same round trip in frontend (2e15) space —
    /// exercises the production frontend_to_hashlife / hashlife_to_frontend
    /// path taken by Engine::load_pattern / Engine::frontend_cells.
    #[test]
    fn test_hashlife_engine_load_frontend_cells_roundtrip() {
        let cells = vec![(10, -5), (11, 5), (-7, 9)];
        let mut e = Engine::HashLife(HashLife::new());
        e.load_pattern(&cells, FRONTEND_OFFSET as i64, FRONTEND_OFFSET as i64);
        let mut got: Vec<(i64, i64)> = e.frontend_cells().iter()
            .map(|&(x, y)| (x as i64 - FRONTEND_OFFSET as i64, y as i64 - FRONTEND_OFFSET as i64))
            .collect();
        got.sort();
        let mut want = cells;
        want.sort();
        assert_eq!(got, want);
    }

    /// Classic -> hashlife -> classic conversion must preserve every cell
    /// (the engine-switch round trip through to_hashlife / from_hashlife).
    #[test]
    fn test_classic_hashlife_roundtrip() {
        let mut c = Classic::new();
        c.load_pattern(&engine_test_pattern(), GRID_OFFSET as i64, GRID_OFFSET as i64);
        let hf = c.to_hashlife();
        let back = Classic::from_hashlife(&hf);
        assert_eq!(classic_cells(&back), classic_cells(&c),
            "classic -> hashlife -> classic must preserve every cell");
    }

    /// Classic-engine stepping benchmark, gated behind the CLASSIC_BENCH env var
    /// (a no-op test unless set) so it never runs in the regular suite.
    ///
    ///   CLASSIC_BENCH=/path/to/pattern.lif CLASSIC_BENCH_GENS=500000 \
    ///   cargo test --release -- --test-threads=1 --nocapture classic_bench
    #[test]
    fn classic_bench() {
        let path = match std::env::var("CLASSIC_BENCH") {
            Ok(p) => p,
            Err(_) => return,
        };
        let max_gens: u64 = std::env::var("CLASSIC_BENCH_GENS")
            .ok().and_then(|s| s.parse().ok()).unwrap_or(500_000);

        // Replicates frontend.rs::parseRLE (relative coords, x=col, y=row).
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
        let mut cells: Vec<(i64, i64)> = Vec::new();
        let mut x: u64 = 0;
        let mut y: u64 = 0;
        let mut count: String = String::new();
        let cleaned: String = text
            .lines()
            .filter(|l| !l.starts_with('#')
                && !l.trim_start().to_ascii_lowercase().starts_with('x')
                && !l.trim_start().to_ascii_lowercase().starts_with('y'))
            .collect::<Vec<_>>()
            .concat()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for c in cleaned.chars() {
            if c.is_ascii_digit() { count.push(c); continue; }
            let n: u64 = if count.is_empty() { 1 } else { count.parse().unwrap_or(1) };
            count.clear();
            match c {
                'o' => { for _ in 0..n { cells.push((x as i64, y as i64)); x += 1; } }
                'b' => x += n,
                '$' => { x = 0; y += n; }
                '!' => break,
                _ => {}
            }
        }

        let mut g = Grid::new();
        let mut c = Classic::new();
        c.load_pattern(&cells, GRID_OFFSET as i64, GRID_OFFSET as i64);
        let mut g = Grid { generation: 0, births: 0, deaths: 0, engine: Engine::Classic(c) };
        eprintln!("[classic_bench] loaded {} cells from {}", cells.len(), path);
        eprintln!("[classic_bench] alive={} active_tiles={}", classic_of(&g).alive.len(), classic_of(&g).active_tiles.len());

        let ts = std::time::Instant::now();
        let mut gen_done: u64 = 0;
        let mut last_mark: u64 = 0;
        while gen_done < max_gens {
            g.step();
            gen_done += 1;
            let mark = gen_done / 50_000;
            if mark > last_mark {
                let el = ts.elapsed().as_secs_f64();
                eprintln!("[classic_bench] gen={} alive={} active_tiles={} elapsed={:.1}s rate={:.0}/s",
                    gen_done, classic_of(&g).alive.len(), classic_of(&g).active_tiles.len(), el, gen_done as f64 / el);
                last_mark = mark;
            }
        }
        let el = ts.elapsed().as_secs_f64();
        eprintln!("[classic_bench] DONE gen={} in {:.1}s ({:.0} gens/sec overall) alive={} active_tiles={}",
            gen_done, el, gen_done as f64 / el, classic_of(&g).alive.len(), classic_of(&g).active_tiles.len());
    }
}
