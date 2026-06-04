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
    pub alive: LifeHashSet<u64>,
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
    // Background thread handle for async alive_vec collection (only when population > 100K)
    pub(crate) collect_handle: Option<std::thread::JoinHandle<()>>,
}

impl Grid {
    pub fn new() -> Self {
        Self {
            alive: LifeHashSet::with_capacity_and_hasher(256, LifeBuildHasher),
            generation: 0,
            births: 0,
            deaths: 0,
            heap: 0,
            active_tiles: std::collections::HashSet::with_hasher(LifeBuildHasher),
            active_count: 0,
            apply_new_active: LifeHashSet::with_capacity_and_hasher(256, LifeBuildHasher),
            alive_vec: Vec::new(),
            collect_handle: None,
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
    #[inline]
    pub fn tile(x: u32) -> u32 {
        x - x % STATIC_SIZE
    }

    /// tile
    #[inline]
    pub fn mod_tile(k: u64) -> (u32, u32) {
       let (x, y) = Coord::unpack(k);
       ( x % STATIC_SIZE,  y % STATIC_SIZE)
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

    fn init_active(&mut self) {
        self.active_tiles.clear();
        for k in &self.alive {
            Self::mark_active(*k, &mut self.active_tiles);
        }
    }

    pub fn randomize(&mut self, cx: i64, cy: i64, size: i64, density: f64) {
        let half = (size / 2) as u32;
        let mut rng = fastrand::Rng::new();
        self.alive.clear();
        for dx in -(half as i32)..=(half as i32) {
            for dy in -(half as i32)..=(half as i32) {
                if rng.f64() < density {
                    let x = (cx + dx as i64) as u32;
                    let y = (cy + dy as i64) as u32;
                    self.alive.insert(Self::k(x, y));
                }
            }
        }
        self.generation = 0;
        self.init_active();
    }

    pub fn clear(&mut self) {
        self.alive.clear();
        self.active_tiles.clear();
        self.active_count = 0;
        self.generation = 0;
        self.births = 0;
        self.deaths = 0;
        self.heap = 0;
    }

    pub fn toggle(&mut self, x: i64, y: i64) {
        let k = Self::k(x as u32, y as u32);
        if self.alive.contains(&k) {
            self.alive.remove(&k);
        } else {
            self.alive.insert(k);
        }
        Self::mark_active(k, &mut self.active_tiles);
    }

    pub fn load_pattern(&mut self, cells: &[(i64, i64)], anchor_x: i64, anchor_y: i64) {
        for &(cx, cy) in cells {
            let x = (cx + anchor_x) as u32;
            let y = (cy + anchor_y) as u32;
            self.alive.insert(Self::k(x, y));
        }
        self.init_active();
    }
}
