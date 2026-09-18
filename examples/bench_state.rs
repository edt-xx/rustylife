// Headless /state fill benchmark: load a pattern (.mc macrocell or .rle),
// advance the HashLife tree, then time `populate_viewport` exactly the way
// server.rs serve_state does — aligned viewport, fresh zeroed buffer per
// call. Reports ms/call plus the TRUE_NODE fill byte attribution from
// hashlife::take_true_fill_bytes (the candidate-4 instrumentation).
//
// Usage: bench_state [path] [max-gens] [vw] [vh] [step]
// Defaults: /home/ed/builds/a_lif/Spiral growth.mc, 196608, 1200, 800, 16384
// (12 full 16384-steps ~= 200K generations — "about 200K without problems")

use game_of_life::hashlife::{coord_pack, frontend_to_hashlife, take_true_fill_bytes, HashLife};
use std::time::Instant;

// hashlife-mode frontend coordOffset (frontend.rs: coordOffset = 2e15)
const COORD_OFFSET: i64 = 2_000_000_000_000_000;

// ---------------------------------------------------------------- .mc parse

#[derive(Clone, Copy)]
enum McNode {
    Empty,
    Leaf(u64), // bit (row*8+col) set = alive
    Internal { level: u32, nw: usize, ne: usize, sw: usize, se: usize },
}

// Rust port of frontend.rs::parseMacrocell (Golly [M2] child-first quadtree).
fn parse_macrocell(text: &str) -> Option<Vec<(i64, i64)>> {
    let lines: Vec<&str> = text.lines().collect();
    if !lines.first().map(|l| l.trim()).unwrap_or("").starts_with("[M2]") {
        return None;
    }

    // Header: skip blank/# lines, capture "# origin = X Y".
    let mut origin: Option<(i64, i64)> = None;
    let mut idx = 1;
    while idx < lines.len() {
        let line = lines[idx].trim();
        if line.is_empty() { idx += 1; continue; }
        if line.starts_with('#') {
            let parts: Vec<&str> = line[1..].split_whitespace().collect();
            if parts.first() == Some(&"origin") {
                if let (Some(x), Some(y)) = (
                    parts.get(1).and_then(|s| s.parse().ok()),
                    parts.get(2).and_then(|s| s.parse().ok()),
                ) {
                    origin = Some((x, y));
                }
            }
            idx += 1;
            continue;
        }
        break;
    }

    // Tree: child-first, nodes numbered from 1 (0 = empty).
    let mut nodes: Vec<McNode> = vec![McNode::Empty];
    while idx < lines.len() {
        let line = lines[idx].trim();
        idx += 1;
        if line.is_empty() { continue; }
        let first = line.as_bytes()[0];
        if first == b'.' || first == b'*' || first == b'$' {
            // Leaf — 8x8 grid: '.' dead, '*' alive, '$' row break.
            let mut bits = 0u64;
            let mut row = 0u32;
            let mut col = 0u32;
            for c in line.as_bytes() {
                match c {
                    b'.' => { if col < 8 { col += 1; } }
                    b'*' => { if row < 8 && col < 8 { bits |= 1u64 << (row * 8 + col); col += 1; } }
                    b'$' => { if row < 7 { row += 1; } col = 0; }
                    _ => {}
                }
            }
            nodes.push(McNode::Leaf(bits));
        } else if first.is_ascii_digit() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                if let (Ok(level), Ok(nw), Ok(ne), Ok(sw), Ok(se)) = (
                    parts[0].parse(), parts[1].parse(), parts[2].parse(), parts[3].parse(), parts[4].parse(),
                ) {
                    nodes.push(McNode::Internal { level, nw, ne, sw, se });
                }
            }
        }
        // Any other header-ish line (e.g. "Cells=...") is skipped, as in the JS parser.
    }
    if nodes.len() < 2 { return None; }
    let root = nodes.len() - 1;
    let root_size: u64 = match nodes[root] {
        McNode::Leaf(_) => 8,
        McNode::Internal { level, .. } => 1u64 << level.min(60),
        McNode::Empty => return None,
    };
    // Golly convention: upper-left of the root's SE child at (0,1).
    let rs = root_size as i64;
    let (ox0, oy0) = origin.unwrap_or((-rs / 2, 1 - rs / 2));

    fn expand(nodes: &[McNode], num: usize, ox: i64, oy: i64, size: i64, cells: &mut Vec<(i64, i64)>) {
        if num == 0 { return; }
        let node = nodes.get(num).cloned().unwrap_or(McNode::Empty);
        match node {
            McNode::Leaf(bits) => {
                for r in 0..8u32 {
                    for c in 0..8u32 {
                        if (bits >> (r * 8 + c)) & 1 == 1 {
                            cells.push((ox + c as i64, oy + r as i64));
                        }
                    }
                }
            }
            McNode::Internal { nw, ne, sw, se, .. } => {
                let h = size / 2;
                expand(nodes, nw, ox, oy, h, cells);
                expand(nodes, ne, ox + h, oy, h, cells);
                expand(nodes, sw, ox, oy + h, h, cells);
                expand(nodes, se, ox + h, oy + h, h, cells);
            }
            McNode::Empty => {}
        }
    }
    let mut cells: Vec<(i64, i64)> = Vec::new();
    expand(&nodes, root, ox0, oy0, root_size as i64, &mut cells);
    Some(cells)
}

// Replicates frontend.rs::parseRLE (relative coords, x=col, y=row) — for .lif/.rle inputs.
fn parse_rle(text: &str) -> Vec<(u64, u64)> {
    let mut cells: Vec<(u64, u64)> = Vec::new();
    let mut x: u64 = 0;
    let mut y: u64 = 0;
    let mut count: String = String::new();
    let cleaned: String = text
        .lines()
        .filter(|l| !l.starts_with('#')
            && !l.trim_start().to_ascii_lowercase().starts_with('x')
            && !l.trim_start().to_ascii_lowercase().starts_with('y'))
        .collect::<Vec<_>>()
        .concat()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for c in cleaned.chars() {
        if c.is_ascii_digit() { count.push(c); continue; }
        let n: u64 = if count.is_empty() { 1 } else { count.parse().unwrap_or(1) };
        count.clear();
        match c {
            'o' => { for _ in 0..n { cells.push((x, y)); x += 1; } }
            'b' => x += n,
            '$' => { x = 0; y += n; }
            '!' => break,
            _ => {}
        }
    }
    cells
}

// -------------------------------------------------------------------- main

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_else(|| "/home/ed/builds/a_lif/Spiral growth.mc".into());
    let max_gens: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(196608);
    let vw: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1200);
    let vh: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(800);
    let step: u32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(16384);

    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let is_mc = text.trim_start().starts_with("[M2]");
    let cells: Vec<(i64, i64)> = if is_mc {
        parse_macrocell(&text).unwrap_or_else(|| panic!("parse macrocell {}", path))
    } else {
        parse_rle(&text).into_iter().map(|(x, y)| (x as i64, y as i64)).collect()
    };
    eprintln!("Loaded {} cells from {} ({})", cells.len(), path, if is_mc { "mc" } else { "rle" });

    // Replicate the frontend load: .mc cells += 2e15 (hashlife-mode coordOffset).
    // from_flat centers on the pattern bbox, so the absolute space is
    // anchor-invariant — packing through frontend_to_hashlife(0,0) is representative.
    let (ha_x, ha_y) = frontend_to_hashlife(0, 0);
    let off = if is_mc { COORD_OFFSET } else { 0 };
    let flat: Vec<u128> = cells.iter().map(|&(cx, cy)| {
        let fx = off.wrapping_add(cx) as u64;
        let fy = off.wrapping_add(cy) as u64;
        coord_pack(ha_x.wrapping_add(fx), ha_y.wrapping_add(fy))
    }).collect();
    let mut hf = HashLife::from_flat(&flat);
    eprintln!("from_flat: nodes={}, alive={}", hf.cache.nodes.len(), hf.alive_count());

    // Advance.
    let mut gen_done: u32 = 0;
    let ts = Instant::now();
    while gen_done < max_gens {
        hf.step_n(step);
        gen_done = gen_done.saturating_add(step);
    }
    eprintln!("stepped to gen {} in {:.2}s ({} nodes, {} alive)",
        gen_done, ts.elapsed().as_secs_f64(), hf.cache.nodes.len(), hf.alive_count());

    // Center the viewport on the live-content bbox (same coord space as tree).
    let flat2 = hf.to_flat();
    if flat2.is_empty() { eprintln!("no live cells — nothing to measure"); return; }
    // Hashlife coords live in 64-bit wrapping space centered near 2^63 — a
    // wide pattern straddles the i64 sign bit, so use SIGNED i64 throughout:
    // signed comparison is the correct order, and min + (max-min)/2 cannot
    // overflow (band width << 2^63). Plain (min+max)/2 overflows i64.
    let f0 = flat2[0];
    let mut minx = (f0 & 0xFFFF_FFFF_FFFF_FFFF) as u64 as i64;
    let mut maxx = minx;
    let mut miny = (f0 >> 64) as u64 as i64;
    let mut maxy = miny;
    for f in flat2.iter().skip(1) {
        let x = (f & 0xFFFF_FFFF_FFFF_FFFF) as u64 as i64;
        let y = (f >> 64) as u64 as i64;
        if x < minx { minx = x; }
        if x > maxx { maxx = x; }
        if y < miny { miny = y; }
        if y > maxy { maxy = y; }
    }
    let cx = minx + (maxx - minx) / 2;
    let cy = miny + (maxy - miny) / 2;
    eprintln!("content bbox: {}x{} at ({},{}), center ({},{})",
        maxx - minx + 1, maxy - miny + 1, minx, miny, cx, cy);

    let hvx = cx.wrapping_sub(vw as i64 / 2) as u64;
    let hvy = cy.wrapping_sub(vh as i64 / 2) as u64;
    let (hvx_a, hvy_a, vw_a, vh_a) = hf.aligned_viewport(hvx, hvy, vw, vh, 1);
    let bits_len = (vw_a as usize * vh_a as usize + 7) / 8;
    let depth = (hf.size() as f64).log2().round() as u32;
    let parallel = bits_len > 64 * 1024 && depth >= 3;
    eprintln!("viewport aligned: ({},{}) {}x{}, buffer {} bytes, path={}",
        hvx_a, hvy_a, vw_a, vh_a, bits_len, if parallel { "parallel-4q" } else { "single" });

    // Warmup (primes the tree for the measurement window).
    for _ in 0..3 {
        let mut bits = vec![0u8; bits_len];
        hf.populate_viewport(hvx_a, hvy_a, vw_a, vh_a, &mut bits);
    }
    take_true_fill_bytes(); // discard warmup attribution

    // Measure ~3s of server-style /state fills: fresh zeroed buffer per call.
    let t1 = Instant::now();
    let mut n: u64 = 0;
    let mut live_bits: u64 = 0;
    let mut tf_bytes: u64 = 0;
    while t1.elapsed().as_secs_f64() < 3.0 {
        let mut bits = vec![0u8; bits_len];
        hf.populate_viewport(hvx_a, hvy_a, vw_a, vh_a, &mut bits);
        live_bits += bits.iter().map(|b| b.count_ones() as u64).sum::<u64>();
        tf_bytes += take_true_fill_bytes();
        n += 1;
    }
    let wall = t1.elapsed();
    let ms = wall.as_secs_f64() * 1000.0 / n as f64;
    eprintln!("[bench_state] gen={} nodes={} alive={} path={} viewport={}x{}",
        gen_done, hf.cache.nodes.len(), hf.alive_count(),
        if parallel { "parallel-4q" } else { "single" }, vw_a, vh_a);
    eprintln!("[bench_state] populate_viewport: {:.3} ms/call over {} calls ({:.2}s total)",
        ms, n, wall.as_secs_f64());
    eprintln!("[bench_state] TRUE_NODE fill: {:.0} bytes/call of {}-byte buffer; {:.0} live bits/call",
        tf_bytes as f64 / n as f64, bits_len, live_bits as f64 / n as f64);
}
