use rayon::prelude::*;
use std::sync::OnceLock;
use crate::grid::*;

fn max_procs() -> usize {
    fn calc() -> usize {
        // Cross-platform physical core count via sysinfo (static method in 0.37)
        sysinfo::System::physical_core_count()
            .map(|c| c as usize)
            .unwrap_or_else(|| {
                // Fallback: query logical threads, assume SMT=2
                std::thread::available_parallelism().map(|p| p.get() / 2).unwrap_or(4)
            })
    }
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(calc)
}

fn neighbor_count_worker(
    chunk: &[u64],
    active_tiles: &LifeHashSet<u64>,
    hint: usize,
) -> (LifeHashMap<u64, u8>, u32) {
    let mut local_nc: LifeHashMap<u64, u8> = std::collections::HashMap::with_capacity_and_hasher(hint, LifeBuildHasher);
    let mut work: u32 = 0;

    for k in chunk {
        let (x, y) = Coord::unpack(*k);
        let (mx, my) = Grid::mod_tile(*k);
        let ak = Coord::pack(x-mx, y-my);

        if !active_tiles.contains(&ak) {
            // Static cell (hot path): grouped neighbor table — one active_tiles check per unique tile
            let info = &TILE_NBR_MASK[mx as usize][my as usize];
            if info.num_groups == 0 {
                continue; // center — nothing to propagate
            }

            // Current tile world-space origin
            let ct_x: i32 = (x - mx) as i32;
            let ct_y: i32 = (y - my) as i32;

            for gi in 0..info.num_groups as usize {
                let g = &info.groups[gi];
                let n_tile_x = ct_x + g.tdx as i32;
                let n_tile_y = ct_y + g.tdy as i32;
                if active_tiles.contains(&Coord::pack(n_tile_x as u32, n_tile_y as u32)) {
                    for ci in 0..g.count as usize {
                        let (cpx, cpy) = g.cells[ci];
                        let nx = n_tile_x + cpx as i32;
                        let ny = n_tile_y + cpy as i32;
                        *local_nc.entry(Coord::pack(nx as u32, ny as u32)).or_insert(0) += 1;
                    }
                }
            }
        } else {
            // Active cell (cold path): full processing (self +10, neighbors +1)
            // this can create local_nc entries in static areas - filtered out later
            *local_nc.entry(*k).or_insert(0) += 10;
            for &(dx, dy) in &NEIGHBOR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let n = Coord::pack(nx as u32, ny as u32);
                *local_nc.entry(n).or_insert(0) += 1;
            }
            work += 1;
        }
    }

    (local_nc, work)
}

impl Grid {
    pub fn step(&mut self) {
        let n_procs = if self.alive.len() > 25 * max_procs() { max_procs() } else { 1 };

        // Collect alive into flat contiguous buffer once per step
        self.alive_vec.clear();
        for &k in &self.alive {
            self.alive_vec.push(k);
        }

        // Split into n_procs contiguous slices
        let total = self.alive_vec.len();
        let chunk_size = total / n_procs;

        if n_procs == 1 {
            let (nc_dict, work) = {
                let chunk = &self.alive_vec[..];
                let hint = chunk.len().saturating_mul(11);
                neighbor_count_worker(chunk, &self.active_tiles, hint)
            };
            Self::apply_rules(self, &nc_dict, work);
        } else {
            let (nc_dict, work) = {
                self.alive_vec.par_chunks(chunk_size)
                    .map(|chunk| neighbor_count_worker(chunk, &self.active_tiles, chunk.len().saturating_mul(11)))
                    .reduce_with(
                        |(mut nc1, w1), (nc2, w2)| {
                            for (&k, &v) in &nc2 {
                                *nc1.entry(k).or_insert(0) += v;
                            }
                            (nc1, w1 + w2)
                        },
                    )
                    .unwrap()
            };

            Self::apply_rules(self, &nc_dict, work);
        }
    }

    fn apply_rules(grid: &mut Grid, nc_dict: &LifeHashMap<u64, u8>, work: u32) {
        // Use pre-allocated buffers from Grid struct to avoid per-step allocations
        grid.apply_new_active.clear();

        // Collect births/deaths/active using borrows to pre-allocated fields
        // Borrow checker: all mutable field refs drop before accessing other fields below
        {
            let new_active = &mut grid.apply_new_active;
            grid.deaths = 0;
            grid.births = 0;

            for (&k, &c) in nc_dict {
                if c < 10 {
                    if c == 3 {
                        let (x, y) = Coord::unpack(k);
                        let ak = Coord::pack(Grid::tile(x), Grid::tile(y));
                        if grid.active_tiles.contains(&ak) {
                            Self::mark_active(k, new_active);
                            grid.alive.insert(k);
                            grid.births += 1;
                        }
                    }
                } else if c < 12 || c > 13 {
                    Self::mark_active(k, new_active);
                    grid.alive.remove(&k);
                    grid.deaths += 1;
                }
            }
        }
        std::mem::swap(&mut grid.active_tiles, &mut grid.apply_new_active);
        grid.active_count = work;
        grid.generation += 1;
        grid.heap = nc_dict.len() as u32;
    }
}
