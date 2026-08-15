use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tiny_http::{Server, Response, StatusCode, Header};
use serde_json::Value;
use crate::grid::{Grid, Coord};

pub fn run(grid: Arc<Mutex<Grid>>) {
    let server = Server::http("0.0.0.0:7654").expect("Failed to start server");

    println!("Game of Life running at http://localhost:7654");

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().clone();

        // Parse query parameters
        let mut params: HashMap<String, String> = HashMap::new();
        if let Some(query) = url.split('?').nth(1) {
            for pair in query.split('&') {
                let mut parts = pair.splitn(2, '=');
                if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
                    params.insert(key.to_string(), value.to_string());
                }
            }
        }

        let path = url.split('?').next().unwrap_or("");

        let response = match (method.as_str(), path) {
            ("GET", "/") => serve_index(),
            ("GET", "/state") => serve_state(&grid, &params),
            ("POST", "/action") => handle_action(&grid, &mut request),
            ("POST", "/toggle") => handle_toggle(&grid, &mut request),
            ("POST", "/load-pattern") => handle_load_pattern(&grid, &mut request),
            ("GET", "/export-pattern") => handle_export_pattern(&grid),
            _ => serve_404(),
        };

        let _ = request.respond(response);
    }
}

fn serve_index() -> Response<Cursor<Vec<u8>>> {
    use crate::frontend::FRONTEND;
    serve_html(FRONTEND)
}

fn serve_404() -> Response<Cursor<Vec<u8>>> {
    let body = b"Not found".to_vec();
    Response::new(
        StatusCode(404),
        vec![],
        Cursor::new(body),
        Some(9),
        None,
    )
}

fn serve_html(body: &str) -> Response<Cursor<Vec<u8>>> {
    let bytes = body.as_bytes().to_vec();
    Response::new(
        StatusCode(200),
        vec![Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap()],
        Cursor::new(bytes),
        Some(body.len()),
        None,
    )
}

fn serve_json(body: &str) -> Response<Cursor<Vec<u8>>> {
    let bytes = body.as_bytes().to_vec();
    Response::new(
        StatusCode(200),
        vec![Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()],
        Cursor::new(bytes),
        Some(body.len()),
        None,
    )
}

fn serve_octet_stream(data: Vec<u8>) -> Response<Cursor<Vec<u8>>> {
    let len = data.len();
    Response::new(
        StatusCode(200),
        vec![Header::from_bytes(&b"Content-Type"[..], &b"application/octet-stream"[..]).unwrap()],
        Cursor::new(data),
        Some(len),
        None,
    )
}

fn serve_state(
    grid: &Arc<Mutex<Grid>>,
    params: &HashMap<String, String>,
) -> Response<Cursor<Vec<u8>>> {
   let vx: u64 = params.get("vx").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vy: u64 = params.get("vy").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vw: u32 = params.get("vw").and_then(|s| s.parse().ok()).unwrap_or(530);
    let vh: u32 = params.get("vh").and_then(|s| s.parse().ok()).unwrap_or(300);
    let scale: u32 = params.get("scale").and_then(|s| s.parse().ok()).unwrap_or(1);

    // Lock grid for snapshot
    let mut g = grid.lock().unwrap();

    let alive_count = if g.hashlife_mode {
        if let Some(ref mut hf) = g.hashlife { hf.alive_count() as u32 } else { 0 }
    } else {
        g.alive.len() as u32
    };

    // When scale > 1, populate aggregated bitmap directly (avoids allocating huge raw bitmap)
    let (final_bits, final_overlay, final_vw, final_vh) = if scale > 1 {
        let scale_usize = scale as usize;
        let agg_w = (vw as usize + scale_usize - 1) / scale_usize;
        let agg_h = (vh as usize + scale_usize - 1) / scale_usize;
        let agg_len = (agg_w * agg_h + 7) / 8;
        let mut agg_bits = vec![0u8; agg_len];
        let mut agg_overlay = vec![0u8; agg_len];

        if g.hashlife_mode {
            // HashLife: populate aggregated bitmap directly from quadtree (no raw allocation)
            // Frontend sends frontend-space coords; convert to hashlife-space
            if let Some(ref hf) = g.hashlife {
                let (hvx, hvy) = game_of_life::hashlife::frontend_to_hashlife(vx, vy);
                hf.populate_aggregated_viewport(hvx, hvy, vw, vh, scale, &mut agg_bits);
            }
        } else {
            // Conventional: aggregate directly from alive set (no raw bitmap, O(alive-in-viewport))
            for &k in &g.alive {
                let (ax, ay) = Coord::unpack(k);
                let ax = ax as u64;
                let ay = ay as u64;
                if ax >= vx && ay >= vy {
                    let rx = ax - vx;
                    let ry = ay - vy;
                    if rx < vw as u64 && ry < vh as u64 {
                        let aggx = (rx / scale as u64) as usize;
                        let aggy = (ry / scale as u64) as usize;
                        let aidx = aggy * agg_w + aggx;
                        agg_bits[aidx >> 3] |= 1 << (aidx & 7);
                    }
                }
            }
        }

        // Overlay: only meaningful in conventional mode
        // Aggregate directly: each active tile (ss×ss cells) maps to a small agg-pixel box
        let ss = crate::grid::STATIC_SIZE;
        if !g.hashlife_mode {
            for &tkey in &g.active_tiles {
                let (tx, ty) = Coord::unpack(tkey);
                // Tile cell range in absolute coords, clamped to viewport
                let cx0 = (tx as u64).max(vx);
                let cx1 = (tx as u64 + ss as u64).min(vx + vw as u64);
                let cy0 = (ty as u64).max(vy);
                let cy1 = (ty as u64 + ss as u64).min(vy + vh as u64);
                if cx0 >= cx1 || cy0 >= cy1 { continue; }
                // Agg-pixel box covered by the tile (viewport-relative; ceil on the far edge)
                let ax0 = ((cx0 - vx) / scale as u64) as usize;
                let ax1 = (((cx1 - vx) + scale as u64 - 1) / scale as u64) as usize;
                let ay0 = ((cy0 - vy) / scale as u64) as usize;
                let ay1 = (((cy1 - vy) + scale as u64 - 1) / scale as u64) as usize;
                for aggy in ay0..ay1 {
                    for aggx in ax0..ax1 {
                        let aidx = aggy * agg_w + aggx;
                        agg_overlay[aidx >> 3] |= 1 << (aidx & 7);
                    }
                }
            }
        }

        (agg_bits, agg_overlay, agg_w as u32, agg_h as u32)
    } else {
        // scale == 1: original path
        let bits_len = (vw as usize * vh as usize + 7) / 8;
        let mut bits = vec![0u8; bits_len];

        if g.hashlife_mode {
            // Frontend sends frontend-space coords; convert to hashlife-space
            if let Some(ref hf) = g.hashlife {
                let (hvx, hvy) = game_of_life::hashlife::frontend_to_hashlife(vx, vy);
                hf.populate_viewport(hvx, hvy, vw, vh, &mut bits);
            }
        } else {
            for &k in &g.alive {
                let (ax, ay) = Coord::unpack(k);
                let ax = ax as u64;
                let ay = ay as u64;
                if ax >= vx && ay >= vy {
                    let rx = ax - vx;
                    let ry = ay - vy;
                    if rx < vw as u64 && ry < vh as u64 {
                        let idx = ry as usize * vw as usize + rx as usize;
                        bits[idx >> 3] |= 1 << (idx & 7);
                    }
                }
            }
        }

        // Overlay: only meaningful in conventional mode
        let ss = crate::grid::STATIC_SIZE;
        let mut overlay = vec![0u8; bits_len];
        if !g.hashlife_mode {
            for &tkey in &g.active_tiles {
                let (tx, ty) = Coord::unpack(tkey);
                for row in ty..(ty + ss) {
                    let row = row as u64;
                    if row < vy || row >= vy + vh as u64 { continue; }
                    let ry = row - vy;
                    for col in tx..(tx + ss) {
                        let col = col as u64;
                        if col >= vx && col < vx + vw as u64 {
                            let rx = col - vx;
                            let idx = ry as usize * vw as usize + rx as usize;
                            overlay[idx >> 3] |= 1 << (idx & 7);
                        }
                    }
                }
            }
        }

        (bits, overlay, vw, vh)
    };

    // Header: gen(u32), vw(u32), vh(u32), pop(u32), active(u32), ol_len(u32), births(u32), deaths(u32), heap(u32), active_tiles(u32)
    // Big-endian
    let mut data = Vec::new();
    data.extend(g.generation.to_be_bytes());
    data.extend((final_vw as u32).to_be_bytes());
    data.extend((final_vh as u32).to_be_bytes());
    data.extend(alive_count.to_be_bytes());
    if g.hashlife_mode {
        if let Some(ref hf) = g.hashlife {
            data.extend((hf.cache.nodes.len() as u32).to_be_bytes());
        } else {
            data.extend(0u32.to_be_bytes());
        }
    } else {
        data.extend(g.active_count.to_be_bytes());
    }
    data.extend((final_overlay.len() as u32).to_be_bytes());
    data.extend(g.births.to_be_bytes());
    data.extend(g.deaths.to_be_bytes());
    if g.hashlife_mode {
        if let Some(ref hf) = g.hashlife {
            data.extend(hf.last_cache_size.to_be_bytes());
            data.extend(hf.last_cache_hit_rate.to_be_bytes());
        } else {
            data.extend(0u32.to_be_bytes());
            data.extend(0u32.to_be_bytes());
        }
    } else {
        data.extend(g.heap.to_be_bytes());
        data.extend((g.active_tiles.len() as u32).to_be_bytes());
    }
    data.extend(scale.to_be_bytes());
    // HashLife mode flag (0 or 1) for frontend sync
    data.extend((if g.hashlife_mode { 1u32 } else { 0u32 }).to_be_bytes());

    data.extend(final_bits);
    data.extend(final_overlay);

    serve_octet_stream(data)
}

fn handle_action(
    grid: &Arc<Mutex<Grid>>,
    request: &mut tiny_http::Request,
) -> Response<Cursor<Vec<u8>>> {
    // Read body
    let mut body = String::new();
    let mut reader = request.as_reader();
    std::io::Read::read_to_string(&mut reader, &mut body).ok();

    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Object(serde_json::Map::new()));
    let action = json.get("action").and_then(|v| v.as_str()).unwrap_or("");

    match action {
        "step" => {
            let mut g = grid.lock().unwrap();
            // eprintln!("SERVER step: hashlife_mode={}", g.hashlife_mode);
            if g.hashlife_mode {
                g.step_hashlife();
            } else {
                g.step();
            }
        }
        "batch-step" => {
            let count = json.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
            let mut g = grid.lock().unwrap();
            // eprintln!("SERVER batch-step: count={}, hashlife_mode={}", count, g.hashlife_mode);
            if g.hashlife_mode {
                g.step_hashlife_n(count as u32);
            } else {
                for _ in 0..count {
                    g.step();
                }
            }
        }
        "toggle-hashlife" => {
            let mut g = grid.lock().unwrap();
            g.hashlife_mode = !g.hashlife_mode;
            if g.hashlife_mode {
                // Switching to hashlife: init from alive set only if quadtree doesn't exist or is empty
                if g.hashlife.is_none() || g.hashlife.as_ref().map_or(true, |hf| hf.is_empty()) {
                    g.init_hashlife();
                }
            } else {
                // Switching to classic: rebuild alive from quadtree and drop it
                g.sync_alive_from_hashlife();
            }
            // eprintln!("SERVER toggle-hashlife: hashlife_mode={}, alive={}, hashlife={:?}",
            //     g.hashlife_mode, g.alive.len(), g.hashlife.is_some());
        }
        "randomize" => {
            let cx = json.get("cx").and_then(|v| v.as_i64()).unwrap_or(200);
            let cy = json.get("cy").and_then(|v| v.as_i64()).unwrap_or(150);
            let size = json.get("size").and_then(|v| v.as_i64()).unwrap_or(100);
            let density = json.get("density").and_then(|v| v.as_f64()).unwrap_or(0.3);
            let mut g = grid.lock().unwrap();
            if g.hashlife_mode {
                // Frontend sends frontend-space coords; convert to hashlife-space
                let (hc, hy) = game_of_life::hashlife::frontend_to_hashlife(cx as u64, cy as u64);
                // Clear existing state before randomizing
                g.hashlife = Some(game_of_life::hashlife::HashLife::new());
                if let Some(ref mut hf) = g.hashlife {
                    let half = size / 2;
                    for dx in -half..=half {
                        for dy in -half..=half {
                            if fastrand::f64() < density {
                                let x = hc.wrapping_add(dx as i64 as u64);
                                let y = hy.wrapping_add(dy as i64 as u64);
                                hf.set_cell(x, y, true);
                            }
                        }
                    }
                }
                g.alive.clear();
                g.alive_index.clear();
                g.active_tiles.clear();
            } else {
                g.randomize(cx, cy, size, density);
                g.invalidate_hashlife();
            }
        }
     "clear" => {
            let mut g = grid.lock().unwrap();
            g.clear();
            if g.hashlife_mode {
                // Clear hashlife quadtree too
                if let Some(ref mut hf) = g.hashlife {
                    *hf = game_of_life::hashlife::HashLife::new();
                }
            } else {
                g.invalidate_hashlife();
            }
        }
        "quit" => {
            println!("Quit requested, shutting down...");
            std::process::exit(0);
        }
        _ => {}
    }

    serve_json(r#"{"ok":true}"#)
}

fn handle_export_pattern(
    grid: &Arc<Mutex<Grid>>,
) -> Response<Cursor<Vec<u8>>> {
    let g = grid.lock().unwrap();

    // Collect alive cells
    let cells: Vec<(u64, u64)> = if g.hashlife_mode {
        if let Some(ref hf) = g.hashlife {
            hf.to_flat().iter().map(|&p| {
                let (hx, hy) = game_of_life::hashlife::coord_unpack(p);
                game_of_life::hashlife::hashlife_to_frontend(hx, hy)
            }).collect()
        } else {
            Vec::new()
        }
    } else {
        g.alive.iter().map(|&k| {
            let (x, y) = Coord::unpack(k);
            (x as u64, y as u64)
        }).collect()
    };

    // Use MC format for large patterns, RLE for small ones
    let text = if g.hashlife_mode && cells.len() > 5000 {
        if let Some(ref hf) = g.hashlife {
            hf.export_mc()
        } else {
            export_rle(&cells)
        }
    } else {
        export_rle(&cells)
    };

    serve_text(&text)
}

/// Export cells to RLE format
fn export_rle(cells: &[(u64, u64)]) -> String {
    if cells.is_empty() {
        return "b!".to_string();
    }

    // Find bounds
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (u64::MAX, 0, u64::MAX, 0);
    for &(x, y) in cells {
        if x < min_x { min_x = x; }
        if x > max_x { max_x = x; }
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    // Build a set for O(1) lookup
    let alive: std::collections::HashSet<(u64, u64)> = cells.iter().copied().collect();

    // Encode header
    let mut out = String::new();
    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    out.push_str(&format!("x = {}, y = {}, rule = B3/S23\n", width, height));

    // Encode row by row
    let mut x = min_x;
    let mut y = min_y;
    let mut run_len = 0;
    let mut run_alive = alive.contains(&(min_x, min_y));

    while y <= max_y {
        while x <= max_x {
            let is_alive = alive.contains(&(x, y));
            if is_alive == run_alive {
                run_len += 1;
            } else {
                // Flush current run
                if run_len > 0 {
                    if run_len > 1 { out.push_str(&run_len.to_string()); }
                    out.push(if run_alive { 'o' } else { 'b' });
                }
                run_len = 1;
                run_alive = is_alive;
            }
            x += 1;
        }
        // Flush remaining run for this row
        if run_len > 0 {
            if run_len > 1 { out.push_str(&run_len.to_string()); }
            out.push(if run_alive { 'o' } else { 'b' });
            run_len = 0;
        }
        out.push('$');
        y += 1;
        x = min_x;
    }

    out.push('!');
    out
}

fn serve_text(body: &str) -> Response<Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap();
    Response::new(
        StatusCode(200),
        vec![header],
        Cursor::new(body.as_bytes().to_vec()),
        Some(body.len()),
        None,
    )
}

fn handle_toggle(
    grid: &Arc<Mutex<Grid>>,
    request: &mut tiny_http::Request,
) -> Response<Cursor<Vec<u8>>> {
    let mut body = String::new();
    let mut reader = request.as_reader();
    std::io::Read::read_to_string(&mut reader, &mut body).ok();

    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Object(serde_json::Map::new()));
  let x = json.get("x").and_then(|v| v.as_f64()).map(|f| f as i64);
    let y = json.get("y").and_then(|v| v.as_f64()).map(|f| f as i64);

    if let (Some(x), Some(y)) = (x, y) {
        let mut g = grid.lock().unwrap();
        if g.hashlife_mode {
            if let Some(ref mut hf) = g.hashlife {
                let (hx, hy) = game_of_life::hashlife::frontend_to_hashlife(x as u64, y as u64);
                let was_alive = hf.get_cell(hx, hy);
                hf.set_cell(hx, hy, !was_alive);
            }
        } else {
            g.toggle(x, y);
        }
    }

    serve_json(r#"{"ok":true}"#)
}

fn handle_load_pattern(
    grid: &Arc<Mutex<Grid>>,
    request: &mut tiny_http::Request,
) -> Response<Cursor<Vec<u8>>> {
    let mut body = String::new();
    let mut reader = request.as_reader();
    std::io::Read::read_to_string(&mut reader, &mut body).ok();

    let json: Value = serde_json::from_str(&body).unwrap_or(Value::Object(serde_json::Map::new()));
    let anchor_x = json.get("anchor_x").and_then(|v| v.as_i64()).unwrap_or(0);
    let anchor_y = json.get("anchor_y").and_then(|v| v.as_i64()).unwrap_or(0);

    if let Some(cells) = json.get("cells").and_then(|v| v.as_array()) {
        let cell_list: Vec<(i64, i64)> = cells.iter()
            .filter_map(|c| {
                if let [Some(x), Some(y)] = [c.get(0).and_then(|v| v.as_i64()), c.get(1).and_then(|v| v.as_i64())] {
                    Some((x, y))
                } else {
                    None
                }
            })
            .collect();
        let mut g = grid.lock().unwrap();
        if g.hashlife_mode {
            if let Some(ref mut hf) = g.hashlife {
                // Frontend sends frontend-space coords; convert to hashlife-space
                let (ha_x, ha_y) = game_of_life::hashlife::frontend_to_hashlife(anchor_x as u64, anchor_y as u64);
                if hf.is_empty() {
                    let flat: Vec<u128> = cell_list.iter().map(|&(ccx, ccy)| {
                        let gx = ha_x.wrapping_add(ccx as i64 as u64);
                        let gy = ha_y.wrapping_add(ccy as i64 as u64);
                        game_of_life::hashlife::coord_pack(gx, gy)
                    }).collect();
                    *hf = game_of_life::hashlife::HashLife::from_flat(&flat);
                } else {
                    for &(ccx, ccy) in &cell_list {
                        let x = ha_x.wrapping_add(ccx as i64 as u64);
                        let y = ha_y.wrapping_add(ccy as i64 as u64);
                        hf.set_cell(x, y, true);
                    }
                }
            }
        } else {
            g.load_pattern(&cell_list, anchor_x, anchor_y);
        }
    }

    serve_json(r#"{"ok":true}"#)
}
