//! Quadtree build/walk/export helpers.
use crate::hashlife::{cache::*, coords::*};

pub fn empty_tree(cache: &mut HashLifeCache, level: u32) -> u32 {
    if level <= 0 { return FALSE_NODE; }
    let child = empty_tree(cache, level - 1);
    cache.find_or_create(child, child, child, child)
}

// Expand node — GOLDE-style shift-up.
// Places each child in a corner with empty padding, creating a node one level deeper.
pub fn expand_node(cache: &mut HashLifeCache, node_idx: u32, level: u32) -> u32 {
    if node_idx == FALSE_NODE {
        return empty_tree(cache, level + 1);
    }
    if node_idx == TRUE_NODE {
        return cache.find_or_create(FALSE_NODE, FALSE_NODE, FALSE_NODE, TRUE_NODE);
    }
    let empty = if level > 0 { empty_tree(cache, level - 1) } else { FALSE_NODE };
    let node = cache.get_node(node_idx);
    let expanded_nw = cache.find_or_create(empty, empty, empty, node.north_west);
    let expanded_ne = cache.find_or_create(empty, empty, node.north_east, empty);
    let expanded_sw = cache.find_or_create(empty, node.south_west, empty, empty);
    let expanded_se = cache.find_or_create(node.south_east, empty, empty, empty);
    cache.find_or_create(expanded_nw, expanded_ne, expanded_sw, expanded_se)
}


pub fn build_quadtree(cache: &mut HashLifeCache, grid: &[Vec<u8>], x0: usize, y0: usize, w: usize, h: usize) -> u32 {
    if w == 0 || h == 0 { return FALSE_NODE; }
    if w == 1 && h == 1 {
        if grid[y0][x0] == 1 { TRUE_NODE } else { FALSE_NODE }
    } else {
        let hw = w / 2; let hh = h / 2;
        let nw = build_quadtree(cache, grid, x0, y0, hw, hh);
        let ne = build_quadtree(cache, grid, x0 + hw, y0, w - hw, hh);
        let sw = build_quadtree(cache, grid, x0, y0 + hh, hw, h - hh);
        let se = build_quadtree(cache, grid, x0 + hw, y0 + hh, w - hw, h - hh);
        cache.find_or_create(nw, ne, sw, se)
    }
}

pub fn count_cells(cache: &mut HashLifeCache, node_idx: u32, depth: u32) -> usize {
    if node_idx == FALSE_NODE { return 0; }
    // A depth-d block is 2^d x 2^d = 4^d cells (leaves are 1x1 sentinels).
    if node_idx == TRUE_NODE { return 1usize << (2 * depth); }
    if depth == 0 { return 1; }
    // Check memoization cache
    if let Some(&c) = cache.count_cache.get(&(node_idx, depth)) {
        return c;
    }
    let node = cache.get_node(node_idx);
    let count = count_cells(cache, node.north_west, depth - 1)
        + count_cells(cache, node.north_east, depth - 1)
        + count_cells(cache, node.south_west, depth - 1)
        + count_cells(cache, node.south_east, depth - 1);
    cache.count_cache.insert((node_idx, depth), count);
    count
}

/// Export a tree node to MC format — child-first ordering
/// Returns the node ID assigned to this node
pub fn export_mc_node(cache: &HashLifeCache, node_idx: u32, depth: u32, counter: &mut u32, out: &mut String, leaf_depth: u32) -> u32 {
    // FALSE_NODE and TRUE_NODE are static — don't emit them
    if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
        return node_idx;
    }

    // Leaf node at leaf_depth (8x8 grid)
    if depth <= leaf_depth {
        *counter += 1;
        let id = *counter;
        // Emit 8x8 grid — one line per node, newline at end
        let size = 1u32 << depth;
        for y in 0..8 {
            for x in 0..8 {
                let gx = if size > 0 { x / (8 / size) } else { x };
                let gy = if size > 0 { y / (8 / size) } else { y };
                let alive = get_cell_at(cache, node_idx, depth, gx, gy);
                out.push(if alive { '*' } else { '.' });
            }
            out.push('$');
        }
        out.push('\n');
        return id;
    }

    // Non-leaf node — emit children first (child-first ordering)
    let node = cache.get_node(node_idx);
    let nw_id = export_mc_node(cache, node.north_west, depth - 1, counter, out, leaf_depth);
    let ne_id = export_mc_node(cache, node.north_east, depth - 1, counter, out, leaf_depth);
    let sw_id = export_mc_node(cache, node.south_west, depth - 1, counter, out, leaf_depth);
    let se_id = export_mc_node(cache, node.south_east, depth - 1, counter, out, leaf_depth);

    // Now emit this node
    *counter += 1;
    let id = *counter;
    out.push_str(&format!("{} {} {} {} {}\n", depth, nw_id, ne_id, sw_id, se_id));
    id
}

pub fn collect_alive(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u64, oy: u64, result: &mut Vec<u128>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        // Uniform block — fill entire region
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push(coord_pack(ox + x as u64, oy + y as u64));
            }
        }
        return;
    }
    if depth == 0 {
        // Leaf cell
        result.push(coord_pack(ox, oy));
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    collect_alive(cache, node.north_west, depth - 1, ox, oy, result);
    collect_alive(cache, node.north_east, depth - 1, ox.wrapping_add(half as u64), oy, result);
    collect_alive(cache, node.south_west, depth - 1, ox, oy.wrapping_add(half as u64), result);
    collect_alive(cache, node.south_east, depth - 1, ox.wrapping_add(half as u64), oy.wrapping_add(half as u64), result);
}

pub fn get_cell_at(cache: &HashLifeCache, node_idx: u32, depth: u32, x: u32, y: u32) -> bool {
    if node_idx == FALSE_NODE { return false; }
    if node_idx == TRUE_NODE { return true; }
    if depth == 0 { return true; }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    if x < half {
        if y < half {
            get_cell_at(cache, node.north_west, depth - 1, x, y)
        } else {
            get_cell_at(cache, node.south_west, depth - 1, x, y - half)
        }
    } else {
        if y < half {
            get_cell_at(cache, node.north_east, depth - 1, x - half, y)
        } else {
            get_cell_at(cache, node.south_east, depth - 1, x - half, y - half)
        }
    }
}

pub fn set_cell_at(cache: &mut HashLifeCache, node_idx: u32, depth: u32, x: u32, y: u32, alive: bool) -> u32 {
    if depth == 0 {
        return if alive { TRUE_NODE } else { FALSE_NODE };
    }
    // Short-circuit: no-op when setting to current value
    if node_idx == FALSE_NODE && !alive { return FALSE_NODE; }
    if node_idx == TRUE_NODE && alive { return TRUE_NODE; }
    // FALSE_NODE → alive=true: need to split and set
    // TRUE_NODE → alive=false: need to split and clear
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    let (nw, ne, sw, se);
    if x < half {
        if y < half {
            nw = set_cell_at(cache, node.north_west, depth - 1, x, y, alive);
            ne = node.north_east;
            sw = node.south_west;
            se = node.south_east;
        } else {
            nw = node.north_west;
            ne = node.north_east;
            sw = set_cell_at(cache, node.south_west, depth - 1, x, y - half, alive);
            se = node.south_east;
        }
    } else {
        if y < half {
            nw = node.north_west;
            ne = set_cell_at(cache, node.north_east, depth - 1, x - half, y, alive);
            sw = node.south_west;
            se = node.south_east;
        } else {
            nw = node.north_west;
            ne = node.north_east;
            sw = node.south_west;
            se = set_cell_at(cache, node.south_east, depth - 1, x - half, y - half, alive);
        }
    }
    cache.find_or_create(nw, ne, sw, se)
}
