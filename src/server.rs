use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tiny_http::{Server, Response, StatusCode, Header};
use serde_json::Value;
use crate::grid::Grid;

fn lock_grid(grid: &Arc<Mutex<Grid>>) -> MutexGuard<'_, Grid> {
    // Even if a previous request panicked while holding the lock, keep
    // serving instead of unwrapping a poisoned mutex forever.
    grid.lock().unwrap_or_else(PoisonError::into_inner)
}

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

        let response = match catch_unwind(AssertUnwindSafe(|| {
            match (method.as_str(), path) {
                ("GET", "/") => serve_index(),
                ("GET", "/state") => serve_state(&grid, &params),
                ("POST", "/action") => handle_action(&grid, &mut request),
                ("POST", "/toggle") => handle_toggle(&grid, &mut request),
                ("POST", "/load-pattern") => handle_load_pattern(&grid, &mut request),
                ("GET", "/export-pattern") => handle_export_pattern(&grid),
                _ => serve_404(),
            }
        })) {
            Ok(resp) => resp,
            Err(_) => {
                eprintln!("request to {path} panicked; serving 500");
                serve_error()
            }
        };

        let _ = request.respond(response);
    }
}

/// Cap on the response bitmap of `/state`, in cells after aggregation
/// (`ceil(vw/scale) x ceil(vh/scale)`). Covers an 8K display at 1px zoom
/// (33M cells); injected into the frontend as `MAX_BITMAP_CELLS`. A
/// malformed request must not force a huge allocation — `vec!` aborts on
/// OOM, which `catch_unwind` cannot catch.
const MAX_BITMAP_CELLS: u64 = 1 << 26; // 64M cells = 8 MiB bitmap

fn serve_index() -> Response<Cursor<Vec<u8>>> {
    use crate::frontend::FRONTEND;
    let html = FRONTEND.replace("__MAX_BITMAP_CELLS__", &MAX_BITMAP_CELLS.to_string());
    serve_html(&html)
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

fn serve_error() -> Response<Cursor<Vec<u8>>> {
    let body = b"internal error".to_vec();
    Response::new(
        StatusCode(500),
        vec![Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap()],
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

/// GET /state handler. Wire format: big-endian u32 header (13 fields,
/// proto_version = 1 at offset 48) followed by the viewport bitmap and
/// the overlay. Canonical spec: `docs/protocol.md`.
fn serve_state(
    grid: &Arc<Mutex<Grid>>,
    params: &HashMap<String, String>,
) -> Response<Cursor<Vec<u8>>> {
   let vx: u64 = params.get("vx").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vy: u64 = params.get("vy").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vw: u32 = params.get("vw").and_then(|s| s.parse().ok()).unwrap_or(530);
    let vh: u32 = params.get("vh").and_then(|s| s.parse().ok()).unwrap_or(300);
    let scale: u32 = params.get("scale").and_then(|s| s.parse().ok()).unwrap_or(1);

    // Cap the response bitmap (post-aggregation cell count) at MAX_BITMAP_CELLS,
    // preserving aspect. Legitimate requests never hit the cap (largest is the
    // window's pixel count at 1px); this only triggers on malformed requests.
    let scale = scale.max(1);
    let (vw, vh) = {
        let aw = if scale > 1 { (vw as u64 + scale as u64 - 1) / scale as u64 } else { vw as u64 };
        let ah = if scale > 1 { (vh as u64 + scale as u64 - 1) / scale as u64 } else { vh as u64 };
        // Include the alignment edge buffer from align_viewport, so the
        // actually-allocated bitmap stays within the budget. Both dims are
        // padded to a multiple of 8 (adds up to 7), plus the seam shift:
        // x shifts by up to 8 aggregated columns (byte alignment,
        // unit_x = 8*scale raw cells) -> x gets +15; y shifts by <1
        // aggregated row (row alignment, unit_y = scale) -> y gets +8.
        let (aw, ah) = (aw + 15, ah + 8);
        let prod = aw * ah;
        if prod <= MAX_BITMAP_CELLS { (vw, vh) }
        else if aw >= ah { ((vw as u64 * MAX_BITMAP_CELLS / prod) as u32, vh) }
        else { (vw, (vh as u64 * MAX_BITMAP_CELLS / prod) as u32) }
    };

    // Lock grid for snapshot
    let mut g = lock_grid(grid);

    let alive_count = g.engine.alive_count();

    // Fill the viewport bitmap via the live engine (each engine interprets the
    // request in its own client space); the clamp above keeps it bounded.
    let (final_bits, final_overlay, final_vw, final_vh) = g.engine.snapshot(vx, vy, vw, vh, scale);

    // Header: 13 big-endian u32 fields — see docs/protocol.md
    let mut data = Vec::new();
    data.extend(g.generation.to_be_bytes());
    data.extend((final_vw as u32).to_be_bytes());
    data.extend((final_vh as u32).to_be_bytes());
    data.extend(alive_count.to_be_bytes());
    let (active, heap, tiles) = g.engine.header_stats();
    data.extend(active.to_be_bytes());
    data.extend((final_overlay.len() as u32).to_be_bytes());
    data.extend(g.births.to_be_bytes());
    data.extend(g.deaths.to_be_bytes());
    data.extend(heap.to_be_bytes());
    data.extend(tiles.to_be_bytes());
    data.extend(scale.to_be_bytes());
    // Engine mode flag (0 = classic, 1 = hashlife) for frontend sync
    data.extend((g.is_hashlife() as u32).to_be_bytes());
    // Protocol version (1 = 13-field header); appended so old clients still parse
    data.extend(1u32.to_be_bytes());

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
            let mut g = lock_grid(grid);
            g.step();
        }
        "batch-step" => {
            let count = json.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
            let mut g = lock_grid(grid);
            g.step_n(count as u32);
        }
        "toggle-hashlife" => {
            let mut g = lock_grid(grid);
            g.toggle_engine();
        }
        "randomize" => {
            let cx = json.get("cx").and_then(|v| v.as_i64()).unwrap_or(200);
            let cy = json.get("cy").and_then(|v| v.as_i64()).unwrap_or(150);
            let size = json.get("size").and_then(|v| v.as_i64()).unwrap_or(100);
            let density = json.get("density").and_then(|v| v.as_f64()).unwrap_or(0.3);
            let mut g = lock_grid(grid);
            g.randomize(cx, cy, size, density);
        }
     "clear" => {
            let mut g = lock_grid(grid);
            g.clear();
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
    let mut g = lock_grid(grid);

    // Engine exports in its own client space (hl: macrocells for large
    // patterns, RLE otherwise; classic: RLE)
    let text = g.engine.export_text();

    serve_text(&text)
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
        let mut g = lock_grid(grid);
        g.engine.toggle_cell(x, y);
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
        let mut g = lock_grid(grid);
        g.engine.load_pattern(&cell_list, anchor_x, anchor_y);
    }

    serve_json(r#"{"ok":true}"#)
}
