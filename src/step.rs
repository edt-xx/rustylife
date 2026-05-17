use rayon::prelude::*;
use crate::grid::*;

const PROC_COUNT: usize = 8;

fn neighbor_count_worker(
    chunk: &[u64],
    active_tiles: &LifeHashSet<u64>,
) -> (LifeHashMap<u64, u32>, u32) {
    let mut local_nc: LifeHashMap<u64, u32> = std::collections::HashMap::with_hasher(LifeBuildHasher);
    let mut work: u32 = 0;

    for k in chunk {
        let (x, y) = Coord::unpack(*k);
        let tx = Grid::tile(x);
        let ty = Grid::tile(y);
        let ak = Coord::pack(tx, ty);

        if active_tiles.contains(&ak) {
            // Active cell: full processing (self +10, neighbors +1)
            *local_nc.entry(*k).or_insert(0) += 10;
            for &(dx, dy) in &NEIGHBOR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let n = Coord::pack(nx as u32, ny as u32);
                *local_nc.entry(n).or_insert(0) += 1;
            }
            work += 1;
        } else {
            // Static cell: propagate +1 ONLY to neighbors whose tile is active
            for &(dx, dy) in &NEIGHBOR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let n_tile = Coord::pack(Grid::tile(nx as u32), Grid::tile(ny as u32));
                if active_tiles.contains(&n_tile) {
                    let n = Coord::pack(nx as u32, ny as u32);
                    *local_nc.entry(n).or_insert(0) += 1;
                }
            }
        }
    }

    (local_nc, work)
}

impl Grid {
    pub fn step(&mut self) {
        let n_procs = if self.alive.len() > 200 { PROC_COUNT } else { 1 };
        let prev_active_tiles = self.active_tiles.clone();

        // Strided split: chunk[i] gets every nth element starting at i
        let chunks: Vec<Vec<u64>> = (0..n_procs)
            .map(|i| self.alive.iter().enumerate().filter(|(idx, _)| *idx % n_procs == i).map(|(_, &v)| v).collect())
            .collect();

        if n_procs == 1 {
            let (nc_dict, work) = neighbor_count_worker(&chunks[0], &prev_active_tiles);
            Self::apply_rules(self, &nc_dict, work, &prev_active_tiles);
        } else {
            let (nc_dict, work) = chunks.par_iter()
                .map(|chunk| neighbor_count_worker(chunk, &prev_active_tiles))
                .reduce_with(
                    |(mut nc1, w1), (nc2, w2)| {
                        for (&k, &v) in &nc2 {
                            *nc1.entry(k).or_insert(0) += v;
                        }
                        (nc1, w1 + w2)
                    },
                )
                .unwrap();

            Self::apply_rules(self, &nc_dict, work, &prev_active_tiles);
        }
    }

    fn apply_rules(grid: &mut Grid, nc_dict: &LifeHashMap<u64, u32>, work: u32, prev_active_tiles: &LifeHashSet<u64>) {
        let mut birth_list = Vec::new();
        let mut death_set: LifeHashSet<u64> = std::collections::HashSet::with_hasher(LifeBuildHasher);
        let mut new_active: LifeHashSet<u64> = std::collections::HashSet::with_hasher(LifeBuildHasher);

        for (&k, &c) in nc_dict {
            if c < 10 {
                if c == 3 {
                    // Births only in active tiles
                    let (x, y) = Coord::unpack(k);
                    let ak = Coord::pack(Grid::tile(x), Grid::tile(y));
                    if prev_active_tiles.contains(&ak) {
                        Self::mark_active(k, &mut new_active);
                        birth_list.push(k);
                    }
                }
            } else if c < 12 || c > 13 {
                death_set.insert(k);
                Self::mark_active(k, &mut new_active);
            }
        }

        let births_count = birth_list.len();
        grid.alive.retain(|e| !death_set.contains(e));
        grid.alive.extend(birth_list);
        grid.active_tiles = new_active;
        grid.active_count = work;
        grid.generation += 1;
        grid.births = births_count as u32;
        grid.deaths = death_set.len() as u32;
        grid.heap = nc_dict.len() as u32;
    }
}
