use super::*;
    use std::collections::HashSet;
    use ahash::AHashMap;

/// Collect alive cells from a node (relative coordinates, no packing)
fn collect_alive_helper(cache: &HashLifeCache, node_idx: u32, depth: u32) -> Vec<(u32, u32)> {
    let mut result = Vec::new();
    collect_alive_rel(cache, node_idx, depth, 0, 0, &mut result);
    result
}

fn collect_alive_rel(cache: &HashLifeCache, node_idx: u32, depth: u32, ox: u32, oy: u32, result: &mut Vec<(u32, u32)>) {
    if node_idx == FALSE_NODE { return; }
    if node_idx == TRUE_NODE {
        let size = 1u32 << depth;
        for y in 0..size {
            for x in 0..size {
                result.push((ox + x, oy + y));
            }
        }
        return;
    }
    if depth == 0 {
        result.push((ox, oy));
        return;
    }
    let node = cache.get_node(node_idx);
    let half = 1u32 << (depth - 1);
    collect_alive_rel(cache, node.north_west, depth - 1, ox, oy, result);
    collect_alive_rel(cache, node.north_east, depth - 1, ox + half, oy, result);
    collect_alive_rel(cache, node.south_west, depth - 1, ox, oy + half, result);
    collect_alive_rel(cache, node.south_east, depth - 1, ox + half, oy + half, result);
}

/// Rebuild tree in new cache by walking old tree
fn rebuild_tree(new_cache: &mut HashLifeCache, old_idx: u32, old_cache: &HashLifeCache) -> u32 {
    if old_idx == FALSE_NODE { return FALSE_NODE; }
    if old_idx == TRUE_NODE { return TRUE_NODE; }
    let old_node = old_cache.get_node(old_idx);
    let nw = rebuild_tree(new_cache, old_node.north_west, old_cache);
    let ne = rebuild_tree(new_cache, old_node.north_east, old_cache);
    let sw = rebuild_tree(new_cache, old_node.south_west, old_cache);
    let se = rebuild_tree(new_cache, old_node.south_east, old_cache);
    new_cache.find_or_create(nw, ne, sw, se)
}



    // Inline Coord for test context — must match module-level coord_pack/coord_unpack
    fn pack(x: u32, y: u32) -> u128 { (y as u128) << 64 | x as u128 }
    fn unpack(cell: u128) -> (u64, u64) { ((cell & ((1u128 << 64) - 1)) as u64, (cell >> 64) as u64) }

    fn step_flat(alive: &HashSet<u128>) -> HashSet<u128> {
        let mut counts = AHashMap::new();
        for &cell in alive {
            let (x, y) = unpack(cell);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 { continue; }
                    let nx = x.wrapping_add(dx as u64);
                    let ny = y.wrapping_add(dy as u64);
                    let key = pack(nx as u32, ny as u32);
                    *counts.entry(key).or_insert(0u32) += 1;
                }
            }
        }
        let mut next = HashSet::new();
        for &cell in alive {
            let c = counts.get(&cell).copied().unwrap_or(0);
            if c == 2 || c == 3 { next.insert(cell); }
        }
        for (&cell, &c) in &counts {
            if !alive.contains(&cell) && c == 3 {
                next.insert(cell);
            }
        }
        next
    }

    fn test_pattern(name: &str, pattern: &[(u32, u32)], gens: usize) {
        let flat_initial: HashSet<u128> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();
        let mut flat: HashSet<u128> = flat_initial.clone();
        let mut hf = HashLife::from_flat(&flat_initial.iter().copied().collect::<Vec<_>>());

        for i in 0..gens {
            // Check to_flat matches BEFORE stepping
            let hf_before: HashSet<u128> = hf.to_flat().into_iter().collect();
            if flat != hf_before {
                let only_flat: Vec<_> = flat.difference(&hf_before).collect();
                let only_hf: Vec<_> = hf_before.difference(&flat).collect();
                if !only_flat.is_empty() || !only_hf.is_empty() {
                    panic!(
                        "{} TREE MISMATCH at gen {} (before step): flat={} hf={}\n\
                         flat_only({}): {:?}\n\
                         hf_only({}): {:?}\n\
                         depth={} center={:?}",
                        name, i, flat.len(), hf_before.len(),
                        only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                        only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                        hf.depth, hf.center,
                    );
                }
            }
            let flat_next = step_flat(&flat);
            hf.step();
            let hf_result: HashSet<u128> = hf.to_flat().into_iter().collect();
            if flat_next != hf_result {
                let only_flat: Vec<_> = flat_next.difference(&hf_result).collect();
                let only_hf: Vec<_> = hf_result.difference(&flat_next).collect();

                panic!(
                    "{} STEP MISMATCH gen {}->{}: flat={} hf={}\n\
                     depth={} center={:?} nodes={}\n\
                     flat_only({}): {:?}\n\
                     hf_only({}): {:?}",
                    name, i, i+1, flat_next.len(), hf_result.len(),
                    hf.depth, hf.center, hf.last_gc_live_len,
                    only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                    only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>()
                );
            }
            println!("{} gen {} OK ({} cells) depth={} nodes={}",
                name, i+1, flat_next.len(), hf.depth, hf.last_gc_live_len);
            flat = flat_next;
        }
    }

    #[test]
    fn test_blinker() {
        test_pattern("blinker", &[(100,100), (100,101), (100,102)], 10);
    }

    #[test]
    fn test_glider() {
        test_pattern("glider", &[(100,100), (101,101), (99,102), (100,102), (101,102)], 20);
    }

    #[test]
    fn test_pi_heptamino() {
        test_pattern("pi_heptamino", &[
            (100,100), (101,100), (102,100),
            (100,101),
            (100,102), (101,102), (102,102),
        ], 30);
    }

    /// Test advance_slow at specific levels by building a tree of known depth
    /// and comparing the advance result with flat stepping.
    #[test]
    fn test_advance_slow_level4() {
        // Create a 16x16 pattern (level 4) centered at (8,8)
        // Place cells that will produce interesting births/deaths
        let pattern_cells: Vec<(u32, u32)> = vec![
            // Pi heptamino in center of 16x16
            (6,6), (7,6), (8,6),
            (6,7),
            (6,8), (7,8), (8,8),
        ];

        // Build flat grid
        let mut flat: HashSet<u128> = HashSet::new();
        for &(x, y) in &pattern_cells {
            flat.insert(pack(x, y));
        }

        // Step flat
        let flat_next = step_flat(&flat);

        // Build HashLife tree at depth 4 (16x16)
        let mut cache = HashLifeCache::new();
        let mut grid_2d = vec![vec![0u8; 16]; 16];
        for &(x, y) in &pattern_cells {
            grid_2d[y as usize][x as usize] = 1;
        }
        let tree = build_quadtree(&mut cache, &grid_2d, 0, 0, 16, 16);

        // Advance with advance_slow at level 4
        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), tree, 4, 0, &mut 0, &mut 0);

        // Result should be a level-3 node (8x8), centered
        // The center 8x8 covers cells (4,4) to (11,11) in the 16x16 grid
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 3, 4, 4, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        // Compare: flat_next should match hf_set for the center 8x8 region
        let flat_center: HashSet<u128> = flat_next.iter()
            .filter(|&&c| {
                let (x, y) = unpack(c);
                x >= 4 && x < 12 && y >= 4 && y < 12
            })
            .copied()
            .collect();

        if flat_center != hf_set {
            let only_flat: Vec<_> = flat_center.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_center).collect();
            panic!(
                "advance_slow level 4 MISMATCH:\n\
                 flat_center({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only: {:?}\n\
                 hf_only: {:?}",
                flat_center.len(), flat_center.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
            );
        } else {
            println!("advance_slow level 4 OK: {} cells match", hf_set.len());
        }
    }

    /// Test advance_slow at level 5 (32x32)
    #[test]
    fn test_advance_slow_level5() {
        let pattern_cells: Vec<(u32, u32)> = vec![
            (14,14), (15,14), (16,14),
            (14,15),
            (14,16), (15,16), (16,16),
        ];

        let mut flat: HashSet<u128> = HashSet::new();
        for &(x, y) in &pattern_cells {
            flat.insert(pack(x, y));
        }
        let flat_next = step_flat(&flat);

        let mut cache = HashLifeCache::new();
        let mut grid_2d = vec![vec![0u8; 32]; 32];
        for &(x, y) in &pattern_cells {
            grid_2d[y as usize][x as usize] = 1;
        }
        let tree = build_quadtree(&mut cache, &grid_2d, 0, 0, 32, 32);

        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), tree, 5, 0, &mut 0, &mut 0);

        // Result is level-4 (16x16), centered at (8,8) of the 32x32
        // Covers cells (8,8) to (23,23)
        let mut hf_result = Vec::new();
        collect_alive_rel(&cache, result, 4, 8, 8, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.iter().map(|&(x,y)| pack(x, y)).collect();

        let flat_center: HashSet<u128> = flat_next.iter()
            .filter(|&&c| {
                let (x, y) = unpack(c);
                x >= 8 && x < 24 && y >= 8 && y < 24
            })
            .copied()
            .collect();

        if flat_center != hf_set {
            let only_flat: Vec<_> = flat_center.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_center).collect();
            panic!(
                "advance_slow level 5 MISMATCH:\n\
                 flat_center({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only: {:?}\n\
                 hf_only: {:?}",
                flat_center.len(), flat_center.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.iter().map(|&c| unpack(*c)).collect::<Vec<_>>(),
            );
        } else {
            println!("advance_slow level 5 OK: {} cells match", hf_set.len());
        }
    }

    /// Test advance_slow at level 7 (128x128) with pi heptamino stepped to gen 22
    #[test]
    fn test_advance_slow_level7_gen22() {
        // Step pi heptamino 22 times using flat
        let initial: HashSet<u128> = [
            pack(100,100), pack(101,100), pack(102,100),
            pack(100,101),
            pack(100,102), pack(101,102), pack(102,102),
        ].into_iter().collect();
        let mut flat = initial.clone();
        for _ in 0..22 {
            flat = step_flat(&flat);
        }
        // flat is now gen 22
        let flat_next = step_flat(&flat);

        // Build HashLife from gen 22 state
        let hf = HashLife::from_flat(&flat.iter().copied().collect::<Vec<_>>());
        
        // Expand to ensure tree is big enough
        let mut cache = hf.cache;
        let mut root = hf.root;
        let mut depth = hf.depth;
        while needs_expansion(&cache, root, depth) || depth < 3 {
            root = expand_node(&mut cache, root, depth);
            depth += 1;
        }

        // Advance with advance_slow
        let result = advance_node(&mut cache, &mut AHashMap::new(), &mut AHashMap::new(), root, depth, 0, &mut 0, &mut 0);

        // Result is at level depth-1
        let result_depth = depth - 1;
        let result_size = 1u32 << result_depth;
        
        // The result covers the center of the expanded tree
        let tree_size = 1u32 << depth;
        let offset = (tree_size - result_size) / 2;
        let origin_x = hf.center.0 - (tree_size as i64 / 2) + offset as i64;
        let origin_y = hf.center.1 - (tree_size as i64 / 2) + offset as i64;
        
        let mut hf_result = Vec::new();
        collect_alive(&cache, result, result_depth, origin_x as u64, origin_y as u64, &mut hf_result);
        let hf_set: HashSet<u128> = hf_result.into_iter().collect();

        if flat_next != hf_set {
            let only_flat: Vec<_> = flat_next.difference(&hf_set).collect();
            let only_hf: Vec<_> = hf_set.difference(&flat_next).collect();
            panic!(
                "advance_slow level {} MISMATCH at gen 22->23:\n\
                 flat({}): {:?}\n\
                 hf({}): {:?}\n\
                 flat_only({}): {:?}\n\
                 hf_only({}): {:?}\n\
                 result_depth={} origin=({},{})",
                depth,
                flat_next.len(), flat_next.iter().take(10).map(|&c| unpack(c)).collect::<Vec<_>>(),
                hf_set.len(), hf_set.iter().take(10).map(|&c| unpack(c)).collect::<Vec<_>>(),
                only_flat.len(), only_flat.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                only_hf.len(), only_hf.iter().take(5).map(|&c| unpack(*c)).collect::<Vec<_>>(),
                result_depth, origin_x, origin_y,
            );
        } else {
            println!("advance_slow level {} OK at gen 22->23: {} cells match", depth, hf_set.len());
        }
    }

    /// Find all distinct level-3 nodes in the tree, recording the absolute (top-left)
    /// coordinate of each occurrence.  Returns Vec<(node_idx, abs_x, abs_y)>.
    fn find_level3_nodes(
        cache: &HashLifeCache,
        node_idx: u32,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(u32, i64, i64)>,
    ) {
        if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
            return;
        }
        if depth == 3 {
            out.push((node_idx, abs_x, abs_y));
            return;
        }
        let node = cache.get_node(node_idx);
        let half = 1i64 << (depth - 1);
        find_level3_nodes(cache, node.north_west, depth - 1, abs_x, abs_y, out);
        find_level3_nodes(cache, node.north_east, depth - 1, abs_x + half, abs_y, out);
        find_level3_nodes(cache, node.south_west, depth - 1, abs_x, abs_y + half, out);
        find_level3_nodes(cache, node.south_east, depth - 1, abs_x + half, abs_y + half, out);
    }

    /// Same as find_level3_nodes but for level-4 nodes (16×16).
    fn find_level4_nodes(
        cache: &HashLifeCache,
        node_idx: u32,
        depth: u32,
        abs_x: i64,
        abs_y: i64,
        out: &mut Vec<(u32, i64, i64)>,
    ) {
        if node_idx == FALSE_NODE || node_idx == TRUE_NODE {
            return;
        }
        if depth == 4 {
            out.push((node_idx, abs_x, abs_y));
            return;
        }
        let node = cache.get_node(node_idx);
        let half = 1i64 << (depth - 1);
        find_level4_nodes(cache, node.north_west, depth - 1, abs_x, abs_y, out);
        find_level4_nodes(cache, node.north_east, depth - 1, abs_x + half, abs_y, out);
        find_level4_nodes(cache, node.south_west, depth - 1, abs_x, abs_y + half, out);
        find_level4_nodes(cache, node.south_east, depth - 1, abs_x + half, abs_y + half, out);
    }

    /// Convert a 4×4 cell block (top-left at tx,ty in the flat grid) into a 16-bit
    /// encoded value using the same bit layout as encode_level2:
    ///   bit_idx(r, c) = 15 - (r*4 + c),  where r=0 is the top row, c=0 is left.
    fn encode_4x4_from_grid(flat: &HashSet<u128>, tx: u32, ty: u32) -> u16 {
        let get = |x: u32, y: u32| -> u16 {
            if flat.contains(&pack(x, y)) { 1 } else { 0 }
        };
        let mut bits = 0u16;
        for r in 0..4u32 {
            for c in 0..4u32 {
                let bit_pos = 15 - (r * 4 + c);
                bits |= get(tx + c, ty + r) << bit_pos;
            }
        }
        bits
    }

    #[test]
    fn test_pi_heptamino_base_case() {
        // Pi heptamino initial pattern
        let pattern: &[(u32, u32)] = &[
            (100,100), (101,100), (102,100),
            (100,101),
            (100,102), (101,102), (102,102),
        ];
        let flat_initial: HashSet<u128> = pattern.iter().map(|&(x, y)| pack(x, y)).collect();

        // Step 22 times using flat stepping (the correct reference)
        let mut flat = flat_initial.clone();
        for _ in 0..22 {
            flat = step_flat(&flat);
        }
        println!("\n=== Pi heptamino at generation 22 (flat reference) ===");
        println!("Alive cells ({}):", flat.len());
        let mut cells: Vec<(u64, u64)> = flat.iter().map(|&c| unpack(c)).collect();
        cells.sort();
        for (x, y) in &cells {
            println!("  ({}, {})", x, y);
        }

        // Build a fresh HashLife from gen-22 flat state
        let flat_vec: Vec<u128> = flat.iter().copied().collect();
        let hf = HashLife::from_flat(&flat_vec);
        println!("\nHashLife: depth={} center={:?} size={}", hf.depth, hf.center, hf.size());

        // Find all level-3 nodes in the tree and their absolute positions
        let origin_x = hf.center.0 - (hf.size() as i64 / 2);
        let origin_y = hf.center.1 - (hf.size() as i64 / 2);
        let mut l3_nodes: Vec<(u32, i64, i64)> = Vec::new();
        find_level3_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l3_nodes);

        // Filter to level-3 nodes that contain at least one alive cell (non-empty)
        let nonempty: Vec<(u32, i64, i64)> = l3_nodes.iter().filter(|(idx, _, _)| {
            let node = hf.cache.get_node(*idx);
            !node.is_empty
        }).copied().collect();
        println!("\nFound {} level-3 nodes ({} non-empty)", l3_nodes.len(), nonempty.len());

        // For each non-empty level-3 node, encode its 4 quadrants (each 4×4) and
        // compare with what the flat grid says.
        //
        // A level-3 node covers an 8×8 region.  Its layout (top-left at ax, ay):
        //   NW quadrant: (ax+0..3, ay+0..3)  → bits in NW mask (0xCC00)
        //   NE quadrant: (ax+4..7, ay+0..3)  → bits in NE mask (0x3300)
        //   SW quadrant: (ax+0..3, ay+4..7)  → bits in SW mask (0x00CC)
        //   SE quadrant: (ax+4..7, ay+4..7)  → bits in SE mask (0x0033)
        //
        // encode_level3 calls encode_level2 on each child (level-2 node = 4×4).
        // The child's 4 leaf cells map to the 4 bits within that quadrant's mask.

        let mut found_mismatch = false;
        for &(idx, ax, ay) in nonempty.iter() {
            let q = encode_level3(&hf.cache, idx);

            // Expected 4×4 blocks from the flat grid
            let exp_nw = encode_4x4_from_grid(&flat, ax as u32, ay as u32);
            let exp_ne = encode_4x4_from_grid(&flat, (ax + 4) as u32, ay as u32);
            let exp_sw = encode_4x4_from_grid(&flat, ax as u32, (ay + 4) as u32);
            let exp_se = encode_4x4_from_grid(&flat, (ax + 4) as u32, (ay + 4) as u32);

            let nw_ok = q.nw == exp_nw;
            let ne_ok = q.ne == exp_ne;
            let sw_ok = q.sw == exp_sw;
            let se_ok = q.se == exp_se;

            if !nw_ok || !ne_ok || !sw_ok || !se_ok {
                found_mismatch = true;
                println!("\n--- ENCODING MISMATCH at level-3 node idx={} abs=({},{}) ---", idx, ax, ay);
                if !nw_ok {
                    println!("  NW: encoded={:#018b} expected={:#018b}", q.nw, exp_nw);
                }
                if !ne_ok {
                    println!("  NE: encoded={:#018b} expected={:#018b}", q.ne, exp_ne);
                }
                if !sw_ok {
                    println!("  SW: encoded={:#018b} expected={:#018b}", q.sw, exp_sw);
                }
                if !se_ok {
                    println!("  SE: encoded={:#018b} expected={:#018b}", q.se, exp_se);
                }
                // Print the 8×8 cell block from the flat grid for visual reference
                println!("  Flat grid 8×8 block at ({},{})", ax, ay);
                for r in 0..8u32 {
                    let mut row = String::new();
                    for c in 0..8u32 {
                        let x = ax as u32 + c;
                        let y = ay as u32 + r;
                        row.push(if flat.contains(&pack(x, y)) { '#' } else { '.' });
                    }
                    println!("    {}", row);
                }
            } else {
                // Encoding matches — now test the rule table lookup + assembly.
                // Reproduce advance_base_one_gen logic and check intermediate values.
                let table = rule_table();
                let r_nw = table[q.nw as usize];
                let r_n = table[window_n(q.nw, q.ne) as usize];
                let r_ne = table[q.ne as usize];
                let r_w = table[window_w(q.nw, q.sw) as usize];
                let r_c = table[window_center(q.nw, q.ne, q.sw, q.se) as usize];
                let r_e = table[window_e(q.ne, q.se) as usize];
                let r_sw = table[q.sw as usize];
                let r_s = table[window_s(q.sw, q.se) as usize];
                let r_se = table[q.se as usize];

                let result_bits = assemble_centered_6x6(r_nw, r_n, r_ne, r_w, r_c, r_e, r_sw, r_s, r_se);

                // Decode the result bits into a 4×4 grid (same layout as encode_level2).
                let result_4x4: [[bool; 4]; 4] = {
                    let mut g = [[false; 4]; 4];
                    for r in 0..4u32 {
                        for c in 0..4u32 {
                            let bit_pos = 15 - (r * 4 + c);
                            g[r as usize][c as usize] = ((result_bits >> bit_pos) & 1) != 0;
                        }
                    }
                    g
                };

                // advance_base_one_gen advances the 8×8 region one generation.
                // The result is a 4×4 node.  The question is: what absolute region
                // does this 4×4 result correspond to?
                //
                // advance_slow at level 3 calls advance_base_one_gen, which returns
                // a level-2 node.  This node represents the centered 4×4 of the next
                // generation.  In the GOLDE scheme, advancing an 8×8 by 1 gen produces
                // a 6×6 centered result, but only the inner 4×4 is stored.
                //
                // The 4×4 result maps to absolute coordinates (ax+2 .. ax+5, ay+2 .. ay+5).
                let flat_next = step_flat(&flat);

                // Build expected 4×4 from flat_next at (ax+2, ay+2)
                let mut expected_4x4 = [[false; 4]; 4];
                for r in 0..4usize {
                    for c in 0..4usize {
                        let x = ax as u32 + 2 + c as u32;
                        let y = ay as u32 + 2 + r as u32;
                        expected_4x4[r][c] = flat_next.contains(&pack(x, y));
                    }
                }

                let result_ok = result_4x4 == expected_4x4;
                let near_center = (ax >= 94 && ax <= 110 && ay >= 90 && ay <= 110);

                if !result_ok || near_center {
                    println!("\n  Level-3 node idx={} abs=({},{}) {}", idx, ax, ay,
                             if result_ok { "— result OK" } else { "— *** RESULT MISMATCH ***" });
                    println!("  Rule results: nw={:#06b} n={:#06b} ne={:#06b} w={:#06b} c={:#06b} e={:#06b} sw={:#06b} s={:#06b} se={:#06b}",
                             r_nw & 0x3F, r_n & 0x3F, r_ne & 0x3F, r_w & 0x3F, r_c & 0x3F, r_e & 0x3F, r_sw & 0x3F, r_s & 0x3F, r_se & 0x3F);
                    println!("  Assembled result_bits = {:#018b}", result_bits);

                    // Print result 4×4
                    print!("  Result  4×4 (ax+2..+5, ay+2..+5):  | ");
                    for r in 0..4usize {
                        if r > 0 { print!("\n                                      | "); }
                        for c in 0..4usize {
                            print!("{}", if result_4x4[r][c] { '#' } else { '.' });
                        }
                    }
                    println!();

                    // Print expected 4×4
                    print!("  Expected 4×4 from flat gen-23:      | ");
                    for r in 0..4usize {
                        if r > 0 { print!("\n                                      | "); }
                        for c in 0..4usize {
                            print!("{}", if expected_4x4[r][c] { '#' } else { '.' });
                        }
                    }
                    println!();

                    // Also show the full 8×8 at gen 22 and the inner 6×6 at gen 23 for context
                    println!("  Input 8×8 at gen 22 (abs {},{}):", ax, ay);
                    for r in 0..8u32 {
                        print!("    ");
                        for c in 0..8u32 {
                            let x = ax as u32 + c;
                            let y = ay as u32 + r;
                            print!("{}", if flat.contains(&pack(x, y)) { '#' } else { '.' });
                        }
                        println!();
                    }
                    println!("  Flat gen-23 inner 6×6 (abs {}+1..+6, {}+1..+6):", ax, ay);
                    for r in 0..6u32 {
                        print!("    ");
                        for c in 0..6u32 {
                            let x = ax as u32 + 1 + c;
                            let y = ay as u32 + 1 + r;
                            print!("{}", if flat_next.contains(&pack(x, y)) { '#' } else { '.' });
                        }
                        println!();
                    }

                    if !result_ok {
                        found_mismatch = true;
                    }
                }
            }
        }

        if found_mismatch {
            panic!("advance_base_one_gen result mismatch detected — see output above");
        } else {
            println!("\n=== All level-3 nodes: encoding OK, advance_base_one_gen result OK ===");
        }

        // Now test level-4 sub-nodes via advance_slow, since the base case is fine.
        // Find all level-4 nodes and verify advance_slow on them matches flat stepping.
        let mut l4_nodes: Vec<(u32, i64, i64)> = Vec::new();
        find_level4_nodes(&hf.cache, hf.root, hf.depth, origin_x, origin_y, &mut l4_nodes);
        let l4_nonempty: Vec<(u32, i64, i64)> = l4_nodes.iter().filter(|(idx, _, _)| {
            let node = hf.cache.get_node(*idx);
            !node.is_empty
        }).copied().collect();
        println!("\nFound {} level-4 nodes ({} non-empty)", l4_nodes.len(), l4_nonempty.len());

        let flat_next = step_flat(&flat);
        let mut l4_mismatch = false;

        // advance_slow needs &mut cache, so clone the cache by rebuilding.
        // We'll test each level-4 node by rebuilding it into a fresh cache.
        for &(idx, ax, ay) in l4_nonempty.iter() {
            // Build a fresh cache with just this sub-tree copied
            let mut test_cache = HashLifeCache::new();
            let copied = rebuild_tree(&mut test_cache, idx, &hf.cache);

            // advance_slow at level 4 recurses to level 3 base case.
            // The result is a level-3 node (8×8) representing (ax+4 .. ax+11, ay+4 .. ay+11).
            let result_idx = advance_node(&mut test_cache, &mut AHashMap::new(), &mut AHashMap::new(), copied, 4, 0, &mut 0, &mut 0);
            let result_cells = collect_alive_helper(&test_cache, result_idx, 3);

            // Expected: cells in the flat_next grid at (ax+4 .. ax+11, ay+4 .. ay+11)
            let mut expected: Vec<(u32, u32)> = Vec::new();
            for r in 0..8u32 {
                for c in 0..8u32 {
                    let x = ax as u32 + 4 + c;
                    let y = ay as u32 + 4 + r;
                    if flat_next.contains(&pack(x, y)) {
                        expected.push((c, r));
                    }
                }
            }
            expected.sort();

            let mut got: Vec<(u32, u32)> = result_cells.iter().copied().collect();
            got.sort();

            if got != expected {
                l4_mismatch = true;
                println!("\n--- LEVEL-4 advance_slow MISMATCH at node idx={} abs=({},{}) ---", idx, ax, ay);
                println!("  Got ({} cells): {:?}", got.len(), got);
                println!("  Expected ({} cells): {:?}", expected.len(), expected);

                // Show the input 16×16 block for context
                println!("  Input 16×16 at gen 22 (abs {},{}):", ax, ay);
                for r in 0..16u32 {
                    print!("    ");
                    for c in 0..16u32 {
                        let x = ax as u32 + c;
                        let y = ay as u32 + r;
                        print!("{}", if flat.contains(&pack(x, y)) { '#' } else { '.' });
                    }
                    println!();
                }
            }
        }
        if l4_mismatch {
            println!("\n=== Level-4 advance_slow mismatches found ===");
        } else {
            println!("\n=== All level-4 advance_slow results OK ===");
        }

        // Finally: step the fresh HashLife once and compare with flat_next
        let mut hf2 = HashLife::from_flat(&flat_vec);
        hf2.step();
        let hf2_result: HashSet<u128> = hf2.to_flat().into_iter().collect();
        if hf2_result != flat_next {
            let only_flat: Vec<_> = flat_next.difference(&hf2_result).collect();
            let only_hf: Vec<_> = hf2_result.difference(&flat_next).collect();
            println!("\n=== FRESH HASHLIFE STEP MISMATCH (gen 22->23) ===");
            println!("depth={} center={:?}", hf2.depth, hf2.center);
            println!("flat_only({}): {:?}", only_flat.len(), only_flat.iter().take(10).map(|&c| unpack(*c)).collect::<Vec<_>>());
            println!("hf_only({}): {:?}", only_hf.len(), only_hf.iter().take(10).map(|&c| unpack(*c)).collect::<Vec<_>>());
        } else {
            println!("\n=== Fresh HashLife step gen 22->23: {} cells match ===", hf2_result.len());
        }
    }

    #[test]
    fn test_populate_viewport() {
        // Build a simple pattern and verify populate_viewport produces non-zero bits
        let cells: Vec<u128> = vec![
            pack(100, 100), pack(101, 100), pack(102, 100),
            pack(100, 101),
            pack(100, 102), pack(101, 102), pack(102, 102),
        ];
        let hf = HashLife::from_flat(&cells);

        // Viewport covering the pattern
        let mut bits = vec![0u8; (20 * 20 + 7) / 8];
        hf.populate_viewport(95, 95, 20, 20, &mut bits);

        // Count set bits
        let mut set_bits = 0;
        for &b in &bits {
            set_bits += b.count_ones() as usize;
        }
        println!("Viewport bits: {} set out of {} total", set_bits, bits.len() * 8);

        // The pi heptamino has 7 cells, all within [95,95]-[115,115]
        assert!(set_bits >= 7, "Expected at least 7 set bits, got {}", set_bits);

        // Also test: after stepping, does the viewport still work?
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step();
        let mut bits2 = vec![0u8; (20 * 20 + 7) / 8];
        hf2.populate_viewport(95, 95, 20, 20, &mut bits2);
        let mut set_bits2 = 0;
        for &b in &bits2 {
            set_bits2 += b.count_ones() as usize;
        }
        println!("After step: {} set bits", set_bits2);
        assert!(set_bits2 >= 7, "Expected at least 7 set bits after step, got {}", set_bits2);
    }

    #[test]
    fn test_multi_gen_advance() {
        // Test that multi-gen advance matches single-gen steps
        let pattern = [
            (100, 100), (101, 100), (102, 100),
            (100, 101), (102, 101),
            (100, 102), (101, 102),
        ];
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance using single-gen steps
        let mut hf1 = HashLife::from_flat(&cells);
        for _ in 0..8 {
            hf1.step();
        }
        let cells1 = hf1.to_flat();

        // Advance using multi-gen (step_n with advance_depth=1)
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step_n(8);
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Multi-gen advance doesn't match single-gen steps");
    }

    #[test]
    fn test_multi_gen_heptamino() {
        // Test multi-gen advance on pi heptamino
        let pattern = [
            (100, 100), (101, 100),
            (100, 101), (101, 101), (102, 101),
            (100, 102), (101, 102),
        ];
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance 16 gens using single-gen steps
        let mut hf1 = HashLife::from_flat(&cells);
        for _ in 0..16 {
            hf1.step();
        }
        let cells1 = hf1.to_flat();

        // Advance 16 gens using multi-gen
        let mut hf2 = HashLife::from_flat(&cells);
        hf2.step_n(16);
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Multi-gen heptamino doesn't match single-gen steps");
    }

    #[test]
    fn test_freelist_gc() {
        // Test that freelist doesn't corrupt patterns across GC boundaries
        // Acorn pattern RLE: bo5b$3bo3b$2o2b3o!
        let pattern = [
            (101, 100), (103, 101),
            (100, 102), (101, 102), (104, 102), (105, 102), (106, 102),
        ];
        let cells: Vec<u128> = pattern.iter().map(|&(x,y)| coord_pack(x,y)).collect();

        // Advance 5000 gens
        let mut hf1 = HashLife::from_flat(&cells);
        hf1.step_n(5000);
        let cells1 = hf1.to_flat();

        // Advance 5000 gens using single steps
        let mut hf2 = HashLife::from_flat(&cells);
        for _ in 0..5000 {
            hf2.step();
        }
        let cells2 = hf2.to_flat();

        assert_eq!(cells1, cells2, "Freelist GC corruption");
    }

    #[test]
    fn test_block_population() {
        // An all-alive 2x2 ("block") compresses to a TRUE node at depth 1,
        // a full 8x8 root compresses to a TRUE node at depth 3. These
        // validate the 1 << (2*depth) formula (the old 1 << depth counted
        // the block as 2 and the 8x8 as 8).
        let pattern = [(100u64, 100u64), (101, 100), (100, 101), (101, 101)];
        let cells: Vec<u128> = pattern.iter().map(|&(x, y)| coord_pack(x, y)).collect();
        let mut hf = HashLife::from_flat(&cells);
        assert_eq!(hf.alive_count(), 4, "fresh block population");
        // The block is a still life: population must hold across gc
        for g in 1..=3u32 {
            hf.step();
            assert_eq!(hf.alive_count(), 4, "block population after gen {}", g);
        }
        // Full 8x8 (depth-3 TRUE root) = 64 cells
        let mut full: Vec<u128> = Vec::new();
        for y in 0..8u64 {
            for x in 0..8u64 {
                full.push(coord_pack(100 + x, 100 + y));
            }
        }
        let mut hf2 = HashLife::from_flat(&full);
        assert_eq!(hf2.alive_count(), 64, "full 8x8 population");
    }

    #[test]
    fn test_align_viewport_scale1() {
        let center = (100i64, 200i64);
        let (hvx_a, hvy_a, vw_a, vh_a) = align_viewport(center, 12345, 67890, 100, 100, 1);
        // Vertical seam byte-aligned: (center.0 - hvx_a) % 8 == 0
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(8), 0, "vertical seam not byte-aligned");
        // Row length multiple of 8
        assert_eq!(vw_a % 8, 0, "vw_a not multiple of 8");
        // Aligned viewport is a superset of the requested one
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvy_a <= 67890, "hvy_a should be <= hvy");
        assert!(hvx_a + vw_a as u64 >= 12345 + 100, "aligned viewport should cover requested x");
        assert!(hvy_a + vh_a as u64 >= 67890 + 100, "aligned viewport should cover requested y");
    }

    #[test]
    fn test_align_viewport_scale2() {
        let center = (100i64, 200i64);
        let (hvx_a, hvy_a, vw_a, vh_a) = align_viewport(center, 12345, 67890, 100, 100, 2);
        // Vertical seam: (center.0 - hvx_a) % (8*scale) == 0
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(16), 0, "vertical seam not aligned (scale=2)");
        // Horizontal seam: (center.1 - hvy_a) % scale == 0
        assert_eq!((center.1 - hvy_a as i64).rem_euclid(2), 0, "horizontal seam not aligned (scale=2)");
        // Agg dimensions multiple of 8
        assert_eq!(((vw_a as u64 + 1) / 2) % 8, 0, "agg_w not multiple of 8 (scale=2)");
        assert_eq!(((vh_a as u64 + 1) / 2) % 8, 0, "agg_h not multiple of 8 (scale=2)");
        // Aligned viewport is a superset
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvy_a <= 67890, "hvy_a should be <= hvy");
        assert!(hvx_a + vw_a as u64 >= 12345 + 100, "aligned viewport should cover requested x");
        assert!(hvy_a + vh_a as u64 >= 67890 + 100, "aligned viewport should cover requested y");
    }

    #[test]
    fn test_aligned_viewport_small() {
        let cells: Vec<u128> = vec![coord_pack(100, 100)];
        let hf = HashLife::from_flat(&cells);
        // 100x100 = 10000 cells = 1250 bytes < 64KB → no alignment
        let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(12345, 67890, 100, 100, 1);
        assert_eq!((hvx_a, hvy_a, vw_a, vh_a), (12345, 67890, 100, 100), "small viewport should be unchanged");
    }

    #[test]
    fn test_aligned_viewport_large() {
        // 16x16 area → depth 4, center (8, 8)
        let mut cells = Vec::new();
        for x in 0..16u64 {
            for y in 0..16u64 {
                cells.push(coord_pack(x, y));
            }
        }
        let hf = HashLife::from_flat(&cells);
        assert!(hf.depth >= 3, "tree should be depth >= 3, got {}", hf.depth);
        // 1000x1000 = 1M cells = 125000 bytes > 64KB → aligned
        let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(12345, 67890, 1000, 1000, 1);
        let center = hf.center;
        assert_eq!((center.0 - hvx_a as i64).rem_euclid(8), 0, "vertical seam not byte-aligned");
        assert_eq!(vw_a % 8, 0, "vw_a not multiple of 8");
        assert!(hvx_a <= 12345, "hvx_a should be <= hvx");
        assert!(hvx_a + vw_a as u64 >= 12345 + 1000, "aligned viewport should cover requested x");
    }

    // ---- Candidate 4: exact-bitmap tests for the TRUE_NODE fill path ----

    #[test]
    fn test_true_fill_exact_small_aligned() {
        // Full 32x32 block -> root (depth 5) is a TRUE node; 128-byte buffer
        // -> single path. The whole viewport must be all ones.
        let mut cells = Vec::new();
        for y in 0..32u64 { for x in 0..32u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 32, 32, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        assert_eq!(bits, vec![0xFFu8; bits.len()], "32x32 true block must fill viewport fully");
    }

    #[test]
    fn test_true_fill_exact_offset_head_tail() {
        // 32x32 block offset by (3,5) in a 30x30 viewport: exercises the
        // head/tail byte masks and scalar middle remainder.
        let mut cells = Vec::new();
        for y in 0..32u64 { for x in 0..32u64 { cells.push(coord_pack(x + 3, y + 5)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 30, 30, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        let mut exp = vec![0u8; bits.len()];
        // Block occupies tree coords [3,35)x[5,35); expected region is the
        // intersection with the viewport, relative to the returned origin.
        // Note: vw=30 here (no alignment under 64KB) so rows use a 30-bit
        // stride — the expected bit position is (y*vw+x) absolute, not
        // (x & 7) within its row byte.
        let x0 = 3u64.max(vx) - vx;
        let x1 = 35u64.min(vx + vw as u64) - vx;
        let y0 = 5u64.max(vy) - vy;
        let y1 = 35u64.min(vy + vh as u64) - vy;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = y * vw as u64 + x;
                exp[(p as usize) >> 3] |= 1 << (p & 7);
            }
        }
        assert_eq!(bits, exp, "offset true block: head/tail byte masks wrong");
    }

    #[test]
    fn test_true_fill_exact_single_path_sse2() {
        // 256x256 full block in a 256x256 viewport: 8KB buffer (single path),
        // 32 bytes per row -> middle run of 30 bytes >= 16 -> SSE2 executes.
        let mut cells = Vec::new();
        for y in 0..256u64 { for x in 0..256u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 256, 256, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        assert_eq!(bits, vec![0xFFu8; bits.len()], "256x256 true block (SSE2 middle) must be all 0xFF");
    }

    #[test]
    fn test_true_fill_exact_parallel_sse2() {
        // 256x256 full block in a 1024x1024 viewport: 128KB buffer ->
        // 4-thread parallel path; the block region must be all ones and the
        // rest of the viewport must stay zero.
        let mut cells = Vec::new();
        for y in 0..256u64 { for x in 0..256u64 { cells.push(coord_pack(x, y)); } }
        let hf = HashLife::from_flat(&cells);
        let (vx, vy, vw, vh) = hf.aligned_viewport(0, 0, 1024, 1024, 1);
        let mut bits = vec![0u8; (vw as usize) * (vh as usize) / 8];
        hf.populate_viewport(vx, vy, vw, vh, &mut bits);
        let mut exp = vec![0u8; bits.len()];
        let x0 = 0u64.max(vx) - vx;
        let x1 = 256u64.min(vx + vw as u64) - vx;
        let y0 = 0u64.max(vy) - vy;
        let y1 = 256u64.min(vy + vh as u64) - vy;
        for y in y0..y1 {
            for x in x0..x1 {
                exp[((y * vw as u64 + x) as usize) >> 3] |= 1 << (x as u32 & 7);
            }
        }
        assert_eq!(bits, exp, "parallel-path true fill: block must be 0xFF, rest zero");
    }

