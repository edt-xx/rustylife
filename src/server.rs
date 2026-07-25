use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tiny_http::{Server, Response, StatusCode, Header};
use serde_json::Value;
use crate::grid::{Grid, Coord};

pub fn run(grid: Arc<Mutex<Grid>>) {
    let server = Server::http("0.0.0.0:7654").expect("Failed to start server");

    println!("Game of Life running at http://grover:7654");

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
            ("POST", "/toggle-timing") => handle_toggle_timing(),
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
   let vx: u32 = params.get("vx").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vy: u32 = params.get("vy").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vw: u32 = params.get("vw").and_then(|s| s.parse().ok()).unwrap_or(530);
    let vh: u32 = params.get("vh").and_then(|s| s.parse().ok()).unwrap_or(300);

    // Lock grid for snapshot
    let mut g = grid.lock().unwrap();
    let bits_len = (vw as usize * vh as usize + 7) / 8;

    // Bits: bit-packed alive cells in viewport
    let mut bits = vec![0u8; bits_len];

    let alive_count = if g.hashlife_mode {
        // HashLife mode: populate viewport from quadtree
        if let Some(ref hf) = g.hashlife {
            // let (cx, cy) = hf.center();
            // eprintln!("STATE hashlife: vx={} vy={} vw={} vh={} center=({},{})",
            //     vx, vy, vw, vh, cx, cy);
            hf.populate_viewport(vx, vy, vw, vh, &mut bits);
            // // Count set bits for debugging
            // let set_bits: usize = bits.iter().map(|b| b.count_ones() as usize).sum();
            // eprintln!("STATE hashlife: set_bits={}", set_bits);
        }
        if let Some(ref mut hf) = g.hashlife { hf.alive_count() as u32 } else { 0 }
    } else {
        // Conventional mode: scan flat alive array
        let ac = g.alive.len() as u32;
        for &k in &g.alive {
            let (ax, ay) = Coord::unpack(k);
            if ax >= vx && ay >= vy {
                let rx = ax - vx;
                let ry = ay - vy;
                if rx < vw && ry < vh {
                    let idx = ry as usize * vw as usize + rx as usize;
                    bits[idx >> 3] |= 1 << (idx & 7);
                }
            }
        }
        ac
    };

    // Overlay: only meaningful in conventional mode (active tiles)
    let ss = crate::grid::STATIC_SIZE;
    let mut overlay = vec![0u8; bits_len];
    if !g.hashlife_mode {
        for &tkey in &g.active_tiles {
            let (tx, ty) = Coord::unpack(tkey);
            for row in ty..(ty + ss) {
                if row < vy || row >= vy + vh { continue; }
                let ry = row - vy;
                for col in tx..(tx + ss) {
                    if col >= vx && col < vx + vw {
                        let rx = col - vx;
                        let idx = ry as usize * vw as usize + rx as usize;
                        overlay[idx >> 3] |= 1 << (idx & 7);
                    }
                }
            }
        }
    }

    // Header: gen(u32), vw(u16), vh(u16), pop(u32), active(u32), ol_len(u32), births(u32), deaths(u32), heap(u32), active_tiles(u32)
    // In hashlife mode: active=n1_cache_size, heap=total_cache_size, active_tiles=cache_hit_rate*10
    // Big-endian from Python's struct.pack(">IHHIIIIIII", ...)
    let mut data = Vec::new();
    data.extend(g.generation.to_be_bytes());
    data.extend((vw as u16).to_be_bytes());
    data.extend((vh as u16).to_be_bytes());
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
    data.extend((overlay.len() as u32).to_be_bytes());
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

    data.extend(bits);
    data.extend(overlay);

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
                g.init_hashlife();
            } else {
                g.invalidate_hashlife();
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
            g.randomize(cx, cy, size, density);
            if g.hashlife_mode { g.rebuild_hashlife(); } else { g.invalidate_hashlife(); }
        }
     "clear" => {
            let mut g = grid.lock().unwrap();
            g.clear();
            if g.hashlife_mode { g.rebuild_hashlife(); } else { g.invalidate_hashlife(); }
        }
        "quit" => {
            println!("Quit requested, shutting down...");
            std::process::exit(0);
        }
        _ => {}
    }

    serve_json(r#"{"ok":true}"#)
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
                let was_alive = hf.get_cell(x as u32, y as u32);
                hf.set_cell(x as u32, y as u32, !was_alive);
            }
        } else {
            g.toggle(x, y);
        }
    }

    serve_json(r#"{"ok":true}"#)
}

fn handle_toggle_timing() -> Response<Cursor<Vec<u8>>> {
    crate::step::toggle_timing();
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
                for &(cx, cy) in &cell_list {
                    let x = (cx + anchor_x) as u32;
                    let y = (cy + anchor_y) as u32;
                    hf.set_cell(x, y, true);
                }
            }
        } else {
            g.load_pattern(&cell_list, anchor_x, anchor_y);
        }
    }

    serve_json(r#"{"ok":true}"#)
}
