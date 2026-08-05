mod frontend;
mod grid;
mod server;
mod step;

#[cfg(not(target_os = "windows"))]
use jemallocator::Jemalloc;

#[cfg(not(target_os = "windows"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use std::sync::{Arc, Mutex};
use grid::Grid;

fn main() {
    println!("Starting Game of Life...");

    let grid = Arc::new(Mutex::new(Grid::new()));

    server::run(grid);
}
