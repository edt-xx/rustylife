mod frontend;
mod grid;
mod server;
mod step;

use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::sync::{Arc, Mutex};
use grid::Grid;

fn main() {
    println!("Starting Game of Life...");

    let grid = Arc::new(Mutex::new(Grid::new()));

    server::run(grid);
}
