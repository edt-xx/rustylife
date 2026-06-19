use rayon::prelude::*;
use std::sync::OnceLock;
use std::time::Instant;
use crate::grid::*;

/// Set to true to enable per-generation timing output to stderr
const TIMING: bool = false;

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

/// Compute tile key directly from packed u64 — masks bottom 2 bits of both x and y
#[inline]
fn tile_key(k: u64) -> u64 {
    k & 0xFFFF_FFFC_FFFF_FFFC
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
        let ct_x: u32 = x - mx;
        let ct_y: u32 = y - my;

        if !active_tiles.contains(&Coord::pack(ct_x, ct_y)) {
            // Static cell (hot path): grouped neighbor table — one active_tiles check per unique tile
            let info = &TILE_NBR_MASK[mx as usize][my as usize];
            if info.num_groups == 0 {
                continue; // center — nothing to propagate
            }

            for gi in 0..info.num_groups as usize {
                let g = &info.groups[gi];
                let n_tile_x = ct_x.wrapping_add(g.tdx as u32);
                let n_tile_y = ct_y.wrapping_add(g.tdy as u32);
                if active_tiles.contains(&Coord::pack(n_tile_x, n_tile_y)) {
                    for ci in 0..g.count as usize {
                        let (cpx, cpy) = g.cells[ci];
                        let nx = n_tile_x.wrapping_add(cpx as u32);
                        let ny = n_tile_y.wrapping_add(cpy as u32);
                        *local_nc.entry(Coord::pack(nx, ny)).or_insert(0) += 1;
                    }
                }
            }
        } else {
            // Active cell (cold path): full processing (self +10, neighbors +1)
            // this can create local_nc entries in static areas - filtered out later
            *local_nc.entry(*k).or_insert(0) += 10;
            for &(dx, dy) in &NEIGHBOR_OFFSETS {
                let nx = x.wrapping_add(dx as u32);
                let ny = y.wrapping_add(dy as u32);
                let n = Coord::pack(nx, ny);
                *local_nc.entry(n).or_insert(0) += 1;
            }
            work += 1;
        }
    }

    (local_nc, work)
}

impl Grid {
    pub fn step(&mut self) {
        #[allow(unused)]
        let t_start = if TIMING { Some(Instant::now()) } else { None };

        // Wait for background alive_vec collection from previous step
        if let Some(handle) = self.collect_handle.take() {
            handle.join().unwrap();
        }

        // Filter alive_vec using bloom filter (filled from previous step)
        #[allow(unused)]
        let t_filter = if TIMING { Some(Instant::now()) } else { None };
        let bloom = &self.expanded_bloom;
        self.alive_vec = self.alive.par_iter()
            .filter(|k| bloom.contains(tile_key(**k)))
            .copied()
            .collect();
        #[allow(unused)]
        let filter_us = if TIMING { t_filter.unwrap().elapsed().as_micros() } else { 0 };

        // Bloom filter will be resized (and cleared) at end of apply_rules

        let n_procs = if self.alive.len() > 10 * max_procs() { max_procs() } else { 1 };

        // Split into n_procs contiguous slices
        let total = self.alive_vec.len();
        let chunk_size = total / n_procs;

        #[allow(unused)]
        let t_nc = if TIMING { Some(Instant::now()) } else { None };
        if n_procs == 1 {
            let (nc_dict, work) = {
                let chunk = &self.alive_vec[..];
                let hint = chunk.len().saturating_mul(11);
                neighbor_count_worker(chunk, &self.active_tiles, hint)
            };
            #[allow(unused)]
            let nc_us = if TIMING { t_nc.unwrap().elapsed().as_micros() } else { 0 };
            Self::apply_rules(self, &nc_dict, work, filter_us, nc_us, t_start);
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
            #[allow(unused)]
            let nc_us = if TIMING { t_nc.unwrap().elapsed().as_micros() } else { 0 };
            Self::apply_rules(self, &nc_dict, work, filter_us, nc_us, t_start);
        }
    }

    #[allow(unused)]
    fn apply_rules(grid: &mut Grid, nc_dict: &LifeHashMap<u64, u8>, work: u32, filter_us: u128, nc_us: u128, t_start: Option<Instant>) {
        #[allow(unused)]
        let t_ar = if TIMING { Some(Instant::now()) } else { None };
        grid.apply_new_active.clear();
        grid.deaths = 0;
        grid.births = 0;

        let n_procs = max_procs();

        if n_procs <= 1 || nc_dict.len() < 10000 {
            // Sequential path for small dicts
            let new_active = &mut grid.apply_new_active;
            for (&k, &c) in nc_dict {
                if c < 10 {
                    if c == 3 {
                        if grid.active_tiles.contains(&tile_key(k)) {
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
        } else {
 // Parallel path with crossbeam channel — i64: negative=birth, positive=death
            let (tx, rx) = crossbeam_channel::bounded::<i64>(16384);
            let active_tiles_ref = &grid.active_tiles;

            rayon::scope(|s| {
                // Single feeder thread — sequential iter is fast enough
                s.spawn(move |_| {
                    for (&k, &c) in nc_dict {
                        if c < 10 {
                            if c == 3 {
                                if active_tiles_ref.contains(&tile_key(k)) {
                                    tx.send(-(k as i64)).unwrap();
                                }
                            }
                        } else if c < 12 || c > 13 {
                            tx.send(k as i64).unwrap();
                        }
                    }
                });

                // Main thread receives and applies concurrently
                let new_active = &mut grid.apply_new_active;
                for val in rx.iter() {
                    let k = val.unsigned_abs();
                    if val < 0 {
                        grid.alive.insert(k);
                        grid.births += 1;
                    } else {
                        grid.alive.remove(&k);
                        grid.deaths += 1;
                    }
                    Self::mark_active(k, new_active);
                }
            });
        }

        std::mem::swap(&mut grid.active_tiles, &mut grid.apply_new_active);
        grid.active_count = work;
        grid.generation += 1;
        grid.heap = nc_dict.len() as u32;

        // Expand active_tiles by 1 tile in all directions for next step's alive_vec filter
        #[allow(unused)]
        let t_expand = if TIMING { Some(Instant::now()) } else { None };
        // Resize bloom filter for optimal size (~3% FP rate)
        grid.expanded_bloom.resize(grid.active_tiles.len() * 9);
        let ss = STATIC_SIZE as i32;
        for &tk in &grid.active_tiles {
            let (tx_t, ty_t) = Coord::unpack(tk);
            for dtx in [-ss, 0, ss] {
                for dty in [-ss, 0, ss] {
                    let ntx = (tx_t as i32 + dtx) as u32;
                    let nty = (ty_t as i32 + dty) as u32;
                    grid.expanded_bloom.insert(Coord::pack(ntx, nty));
                }
            }
        }
        #[allow(unused)]
        let expand_us = if TIMING { t_expand.unwrap().elapsed().as_micros() } else { 0 };

        if TIMING {
            let ar_us = t_ar.unwrap().elapsed().as_micros();
            let total_us = t_start.unwrap().elapsed().as_micros();
            eprintln!("gen={} filter={}us nc={}us ar={}us expand={}us total={}us bloom_bits={}",
                grid.generation, filter_us, nc_us, ar_us, expand_us, total_us,
                grid.expanded_bloom.size_bits);
        }

        // Spawn background thread to collect alive into alive_vec for next step
        // Only when population > 250K — thread overhead not worth it for small populations
        if grid.alive.len() > 250_000 {
            let grid_ptr = grid as *mut Grid as usize;
            let handle = std::thread::spawn(move || {
                let grid = unsafe { &mut *(grid_ptr as *mut Grid) };
                let alive = &grid.alive;
                let bloom = &grid.expanded_bloom;
                let vec = &mut grid.alive_vec;
                vec.clear();
                for &k in alive {
                    if bloom.contains(tile_key(k)) {
                        vec.push(k);
                    }
                }
            });
            grid.collect_handle = Some(handle);
        } else {
            grid.collect_handle = None;
        }
    }
}
