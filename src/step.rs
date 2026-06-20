use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;
use crate::grid::*;

/// Runtime toggle for per-generation timing output to stderr
static TIMING_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn toggle_timing() {
    let current = TIMING_ENABLED.load(Ordering::Relaxed);
    TIMING_ENABLED.store(!current, Ordering::Relaxed);
}

fn timing_on() -> bool {
    TIMING_ENABLED.load(Ordering::Relaxed)
}

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

        if !active_tiles.contains(&tile_key(*k)) {
            let (mx, my) = Grid::mod_tile(*k);
            // Static cell (hot path): grouped neighbor table — one active_tiles check per unique tile
            let info = &TILE_NBR_MASK[mx as usize][my as usize];
            if info.num_groups == 0 {
                continue; // center — nothing to propagate
            }

            let ct_x: u32 = x - mx;
            let ct_y: u32 = y - my;

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
        let t_start = Instant::now();

        // Initialize active_tiles and bloom filter on first step
        if self.expanded_bloom.bits.is_empty() {
            self.init_active();
        }

        // Filter alive_vec using bloom filter (filled from previous step)
        let t_filter = Instant::now();
        let bloom = &self.expanded_bloom;
        let n_procs = if self.alive.len() > 10 * max_procs() { max_procs() } else { 1 };
        let chunk_size = self.alive.len() / n_procs;
        let mut chunks: Vec<Vec<u64>> = self.alive.par_chunks(chunk_size)
            .map(|chunk| chunk.iter()
                .filter(|k| bloom.contains(tile_key(**k)))
                .copied()
                .collect())
            .collect();
        self.alive_vec.clear();
        fastrand::shuffle(&mut chunks);
        for chunk in chunks {
            self.alive_vec.extend(chunk);
        }
        let filter_us = t_filter.elapsed().as_micros();

        // Bloom filter will be resized (and cleared) at end of apply_rules

        let n_procs = if self.alive.len() > 10 * max_procs() { max_procs() } else { 1 };

        // Split into n_procs contiguous slices
        let total = self.alive_vec.len();
        let chunk_size = total / n_procs;

        let t_nc = Instant::now();
        if n_procs == 1 {
            let (nc_dict, work) = {
                let chunk = &self.alive_vec[..];
                let hint = chunk.len().saturating_mul(11);
                neighbor_count_worker(chunk, &self.active_tiles, hint)
            };
            let nc_us = t_nc.elapsed().as_micros();
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
            let nc_us = t_nc.elapsed().as_micros();
            Self::apply_rules(self, &nc_dict, work, filter_us, nc_us, t_start);
        }
    }

    fn apply_rules(grid: &mut Grid, nc_dict: &LifeHashMap<u64, u8>, work: u32, filter_us: u128, nc_us: u128, t_start: Instant) {
        let t_ar = Instant::now();
        grid.apply_new_active.clear();
        grid.deaths = 0;
        grid.births = 0;

        let n_procs = max_procs();

        if n_procs <= 1 || nc_dict.len() < 1000000 {
            // Sequential path for small dicts
            for (&k, &c) in nc_dict {
                if c < 10 {
                    if c == 3 {
                        if grid.active_tiles.contains(&tile_key(k)) {
                            grid.insert_alive(k);
                            grid.births += 1;
                            Self::mark_active(k, &mut grid.apply_new_active);
                        }
                    }
                } else if c < 12 || c > 13 {
                    grid.remove_alive(k);
                    grid.deaths += 1;
                    Self::mark_active(k, &mut grid.apply_new_active);
                }
            }
        } else {
 // Parallel path — collect mutations first, apply after
            let (tx, rx) = crossbeam_channel::bounded::<i64>(16384);
            let active_tiles_ref = &grid.active_tiles;

            rayon::scope(|s| {
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
            });

            // Apply mutations after scope ends (no borrows)
            for val in rx.iter() {
                let k = val.unsigned_abs();
                if val < 0 {
                    grid.insert_alive(k);
                    grid.births += 1;
                } else {
                    grid.remove_alive(k);
                    grid.deaths += 1;
                }
                Self::mark_active(k, &mut grid.apply_new_active);
            }
        }

        std::mem::swap(&mut grid.active_tiles, &mut grid.apply_new_active);
        grid.active_count = work;
        grid.generation += 1;
        grid.heap = nc_dict.len() as u32;

        // Expand active_tiles by 1 tile in all directions for next step's bloom filter
        let t_expand = Instant::now();
        // Resize bloom filter for optimal size (~5% FP rate)
        grid.expanded_bloom.resize(grid.active_tiles.len() * 9);
        let ss = STATIC_SIZE as i32;
        for &tk in &grid.active_tiles {
            let (tx_t, ty_t) = Coord::unpack(tk);
            for dtx in [-ss, 0, ss] {
                for dty in [-ss, 0, ss] {
                    let ntx = tx_t.wrapping_add(dtx as u32);
                    let nty = ty_t.wrapping_add(dty as u32);
                    grid.expanded_bloom.insert(Coord::pack(ntx, nty));
                }
            }
        }
        let expand_us = t_expand.elapsed().as_micros();

        if timing_on() {
            let ar_us = t_ar.elapsed().as_micros();
            let total_us = t_start.elapsed().as_micros();
            eprintln!("gen={} filter={}us ({}) nc={}us ({}) ar={}us ({}) expand={}us ({}) total={}us bloom_bits={}",
                grid.generation, filter_us, grid.apply_new_active.len(), nc_us, grid.alive_vec.len(), ar_us, nc_dict.len(), expand_us, grid.active_tiles.len(), total_us,
                grid.expanded_bloom.size_bits);
        }
    }
}
