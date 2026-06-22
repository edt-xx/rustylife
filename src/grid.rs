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

pub struct Grid {
    /// Contiguous Vec for parallel chunked iteration
    pub alive: Vec<u64>,
    /// Maps key -> Vec index for O(1) swap-remove deletes
    pub alive_index: LifeHashMap<u64, usize>,
    pub generation: u32,
    pub births: u32,
    pub deaths: u32,
    pub heap: u32,
    pub active_tiles: LifeHashSet<u64>,
    pub active_count: u32,
    // Pre-allocated buffer for step() — reused across generations to avoid allocations
    pub(crate) apply_new_active: LifeHashSet<u64>,
    // Pre-allocated flat buffer for step() — contiguous slices distributed to workers
    pub(crate) alive_vec: Vec<u64>,
    // Pre-allocated buffers for births/deaths — cleared and reused each generation
    pub(crate) births_buf: Vec<u64>,
    pub(crate) deaths_buf: Vec<u64>,
    // Bloom filter for expanded active tiles — active_tiles + 1-tile neighborhood
    pub(crate) expanded_bloom: BloomFilter,
    // Bloom filter for active tile keys — replaces HashSet.contains for speed
    pub(crate) active_bloom: BloomFilter,
}

impl Grid {
    pub fn new() -> Self {
        Self {
            alive: Vec::with_capacity(256),
            alive_index: LifeHashMap::with_capacity_and_hasher(256, LifeBuildHasher),
            generation: 0,
            births: 0,
            deaths: 0,
            heap: 0,
            active_tiles: std::collections::HashSet::with_hasher(LifeBuildHasher),
            active_count: 0,
            apply_new_active: LifeHashSet::with_capacity_and_hasher(256, LifeBuildHasher),
            alive_vec: Vec::new(),
            births_buf: Vec::with_capacity(256),
            deaths_buf: Vec::with_capacity(256),
            expanded_bloom: BloomFilter { bits: Vec::new(), size_bits: 0, mask: 0 },
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
        self.expanded_bloom.resize(self.active_tiles.len() * 15);
        for &tk in &self.active_tiles {
            let (tx, ty) = Self::unpack(tk);

            self.active_bloom.insert(tk);

            // Top border (y = ty-1, x = tx-1 .. tx+ss)
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_sub(1)));
            self.expanded_bloom.insert(Coord::pack(tx, ty.wrapping_sub(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(1), ty.wrapping_sub(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(2), ty.wrapping_sub(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(3), ty.wrapping_sub(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_sub(1)));

            // Bottom border (y = ty+ss, x = tx-1 .. tx+ss)
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(ss)));
            self.expanded_bloom.insert(Coord::pack(tx, ty.wrapping_add(ss)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(1), ty.wrapping_add(ss)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(2), ty.wrapping_add(ss)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(3), ty.wrapping_add(ss)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(ss)));

            // Left border (x = tx-1, y = ty .. ty+ss-1)
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(2)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_sub(1), ty.wrapping_add(3)));

            // Right border (x = tx+ss, y = ty .. ty+ss-1)
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(1)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(2)));
            self.expanded_bloom.insert(Coord::pack(tx.wrapping_add(ss), ty.wrapping_add(3)));
        }
    }

    pub fn randomize(&mut self, cx: i64, cy: i64, size: i64, density: f64) {
        let half = (size / 2) as u32;
        let mut rng = fastrand::Rng::new();
        self.alive.clear();
        self.alive_index.clear();
        self.active_tiles.clear();
        self.expanded_bloom.resize(0);
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
        self.generation = 0;
        self.init_active();
    }

    pub fn clear(&mut self) {
        self.alive.clear();
        self.alive_index.clear();
        self.active_tiles.clear();
        self.expanded_bloom.resize(0);
        self.active_bloom.resize(0);
        self.active_count = 0;
        self.generation = 0;
        self.births = 0;
        self.deaths = 0;
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
}
