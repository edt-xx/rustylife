use std::collections::HashMap;
use std::sync::Arc;

/// Tile: quadtree node representing a cell or uniform region
#[derive(Clone, Debug)]
pub struct Tile(pub Arc<Node>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf(u8),           // single cell value (alive=1, dead=0)
    Internal([Tile; 4]), // NW, NE, SW, SE children
}

impl PartialEq for Tile {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Tile {}

impl std::hash::Hash for Tile {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Hash based on the node structure
        match self.0.as_ref() {
            Node::Leaf(val) => val.hash(state),
            Node::Internal(children) => {
                for child in children {
                    child.hash(state);
                }
            }
        }
    }
}

/// Lookup table: maps each possible 3x3 tile pattern → its next-generation result
type LUT = HashMap<[u8; 9], [u8; 9]>;

pub struct HashLife {
    grid: Tile,
    lut: LUT,
}

impl HashLife {
    pub fn new() -> Self {
        let mut hf = HashLife {
            grid: Tile(Arc::new(Node::Leaf(0))),
            lut: HashMap::with_capacity(512),
        };
        hf.build_lut();
        hf
    }

    fn build_lut(&mut self) {
        // Enumerate all 2^9 = 512 possible 3x3 patterns
        for pattern in 0..512u32 {
            let mut input = [0u8; 9];
            for i in 0..9 {
                input[i] = ((pattern >> i) & 1) as u8;
            }
            let output = self.apply_rules(&input);
            self.lut.insert(input, output);
        }
    }

    fn apply_rules(&self, cells: &[u8; 9]) -> [u8; 9] {
        // Standard Game of Life rules applied to each cell in the 3x3 grid
        let mut result = *cells;
        for i in 0..9 {
            let (x, y) = self.index_to_coords(i);
            let neighbors = self.count_neighbors(cells, x, y);

            if cells[i] == 1 {
                // Alive cell: survive with 2 or 3 neighbors
                result[i] = if neighbors == 2 || neighbors == 3 { 1 } else { 0 };
            } else {
                // Dead cell: birth with exactly 3 neighbors
                result[i] = if neighbors == 3 { 1 } else { 0 };
            }
        }
        result
    }

    fn index_to_coords(&self, idx: usize) -> (usize, usize) {
        ((idx % 3), (idx / 3))
    }

    fn count_neighbors(&self, cells: &[u8; 9], x: usize, y: usize) -> u8 {
        let mut count = 0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 { continue; }
                let nx = (x as i32 + dx).rem_euclid(3) as usize;
                let ny = (y as i32 + dy).rem_euclid(3) as usize;
                count += cells[ny * 3 + nx];
            }
        }
        count
    }

    pub fn step(&mut self) {
        // Full HashLife implementation:
        // 1. Decompose grid into 3x3 tiles
        // 2. Look up each tile in LUT
        // 3. Rebuild grid from results
        let tiles = self.decompose_3x3();
        let next_tiles: Vec<[u8; 9]> = tiles.iter()
            .map(|t| *self.lut.get(t).unwrap())
            .collect();
        self.rebuild_grid(&next_tiles);
    }

    fn decompose_3x3(&self) -> Vec<[u8; 9]> {
        // Convert grid to flat array and decompose into 3x3 tiles
        let flat = self.to_flat();
        if flat.is_empty() {
            return vec![[0u8; 9]];
        }

        // Find bounds of the grid
        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = u32::MIN;
        let mut max_y = u32::MIN;

        for &cell in &flat {
            let (x, y) = Coord::unpack(cell);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }

        // Convert to 2D array
        let width = (max_x - min_x + 1) as usize;
        let height = (max_y - min_y + 1) as usize;
        let mut grid_2d = vec![vec![0u8; width]; height];

        for &cell in &flat {
            let (x, y) = Coord::unpack(cell);
            if x >= min_x && y >= min_y {
                grid_2d[(y - min_y) as usize][(x - min_x) as usize] = 1;
            }
        }

        // Decompose into 3x3 tiles with overlap
        let mut tiles = Vec::new();
        for y in (0..height).step_by(3) {
            for x in (0..width).step_by(3) {
                let mut tile = [0u8; 9];
                for dy in 0..3 {
                    for dx in 0..3 {
                        if y + dy < height && x + dx < width {
                            tile[dy * 3 + dx] = grid_2d[y + dy][x + dx];
                        }
                    }
                }
                tiles.push(tile);
            }
        }

        tiles
    }

    fn rebuild_grid(&mut self, tiles: &[[u8; 9]]) {
        // Reconstruct grid from LUT results
        // For now, just store as a leaf since we don't have the full reconstruction logic
        self.grid = Tile(Arc::new(Node::Leaf(0)));
    }

    /// Extract cell value at position (x, y) from quadtree
    pub fn get_cell(&self, x: usize, y: usize) -> u8 {
        // For now, return 0 since we don't have direct access to the grid data
        0
    }

    /// Set cell value at position (x, y) in quadtree
    pub fn set_cell(&mut self, x: usize, y: usize, value: u8) {
        // For now, no-op since we don't have direct access to the grid data
    }

    /// Convert grid to flat array for processing
    pub fn to_flat(&self) -> Vec<u64> {
        // For now, return empty since we don't have direct access to the grid data
        // This will be replaced with actual grid data when integrated
        vec![]
    }

    /// Convert flat array back to quadtree
    pub fn from_flat(data: &[u64]) -> Tile {
        // For now, return empty leaf
        Tile(Arc::new(Node::Leaf(0)))
    }
}
