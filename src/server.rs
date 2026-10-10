use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tiny_http::{Server, Response, StatusCode, Header};
use serde_json::Value;
use crate::grid::{Engine, Grid};

fn lock_grid(grid: &Arc<Mutex<Grid>>) -> MutexGuard<'_, Grid> {
    // Even if a previous request panicked while holding the lock, keep
    // serving instead of unwrapping a poisoned mutex forever.
    grid.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn run(grid: Arc<Mutex<Grid>>) {
    let frames: StateFramesHandle = Arc::new(Mutex::new(HashMap::new()));
    // GOL_PORT: experiment harness only (lets a second instance run beside the
    // production one). Uncommitted.
    let port: u16 = std::env::var("GOL_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(7654);
    let server = Server::http(format!("0.0.0.0:{port}")).expect("Failed to start server");

    println!("Game of Life running at http://localhost:{port}");

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
                ("GET", "/state") => serve_state(&grid, &frames, &params),
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
                // MED #3: the panic may have interrupted a mutation
                // mid-way (e.g. after the active_tiles swap in
                // apply_rules), leaving alive/alive_index half-updated.
                // Mark Classic state suspect; the next step() re-derives
                // every derived structure from `alive`.
                if let Engine::Classic(c) = &mut lock_grid(&grid).engine {
                    c.mark_dirty();
                }
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
    use crate::frontend::{FRONTEND_CSS, FRONTEND_HTML, FRONTEND_JS};
    let js = FRONTEND_JS.replace("__MAX_BITMAP_CELLS__", &MAX_BITMAP_CELLS.to_string());
    let html = FRONTEND_HTML.replace("__JS__", &js).replace("__CSS__", FRONTEND_CSS);
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

/// 400 + `{"ok":false,"error":"..."}` — same `ok` key as success responses,
/// plus an `error` string. Used when a request is malformed or names an
/// unknown action, instead of silently replying `{"ok":true}` (MED #2).
fn serve_bad_request(msg: &str) -> Response<Cursor<Vec<u8>>> {
    let body = serde_json::json!({"ok": false, "error": msg}).to_string();
    let len = body.len();
    Response::new(
        StatusCode(400),
        vec![Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap()],
        Cursor::new(body.into_bytes()),
        Some(len),
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
/// proto_version 3 (HashLife only): the frame the last `/state` response handed
/// THIS client, so the next response can carry only the bytes that changed.
///
/// Keyed by client id (`&cid=`, one per page load), NOT one global slot: a
/// single shared slot let any second poller (a benchmark run, another tab)
/// evict the browser's base frame, which made every browser response a full
/// frame. One entry per client, LRU-capped.
///
/// The entry is exactly what we last SENT to that cid, so a diff against it is
/// exact no matter what the sim did in between — mutations never need to
/// invalidate it. It is usable only when the viewport key matches AND the
/// client confirms via `&base=<gen>` that it still holds that frame. Anything
/// else (pan, zoom, resize, mode switch, server restart, a response the client
/// discarded) costs one full frame and can never corrupt the picture.
struct ClientFrame {
    key: (u64, u64, u32, u32, u32, bool), // vx, vy, final_vw, final_vh, scale, mode
    base_gen: u32,
    bits: Vec<u8>,
    last_use: u64,
}
type StateFramesHandle = Arc<Mutex<HashMap<String, ClientFrame>>>;

/// Tabs/reloads get a slot each; older ones age out instead of the map growing
/// without bound. Each entry costs one viewport bitmap (120 KB at 1200x800).
const MAX_CLIENT_FRAMES: usize = 4;

/// Below this the delta machinery is not worth it, and it also rules out the
/// degenerate sizes where a v3 total length could collide with a raw frame's.
const DELTA_MIN_BYTES: usize = 4096;

static FRAME_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn lock_frames(h: &StateFramesHandle) -> MutexGuard<'_, HashMap<String, ClientFrame>> {
    h.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Unsigned LEB128. Index deltas are almost always 1-2 bytes.
fn put_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 { out.push(b); break; }
        out.push(b | 0x80);
    }
}

/// proto_version 3 payload: the bytes of `cur` that differ from `prev`, as
/// `varint(count)` then `count` entries of `varint(index - previous_index)` +
/// the new byte. Indices ascend, so each delta is >= 1.
///
/// The scan is word-wise: eight bytes compared per iteration, and a word that
/// matches (the overwhelmingly common case — a 1200x800 viewport with ~5000
/// live cells has ~15,000 words and almost all of them repeat) is skipped
/// whole. It runs outside the grid lock, on a buffer the fill just wrote.
// Encodes `cur` as a byte-delta over `prev` into `out`, returning the number of
// changed bytes. `limit` is the point past which the delta can no longer win the
// caller's half-bitmap test: once the entry stream is that long the rest of the
// scan is pure waste, so bail early and report usize::MAX. `out` is left
// untouched in that case — the caller must not send it.
fn delta_encode(prev: &[u8], cur: &[u8], out: &mut Vec<u8>, limit: usize) -> usize {
    let n = prev.len().min(cur.len());
    let mut entries: Vec<u8> = Vec::new();
    let mut last = 0usize;
    let mut changed = 0usize;
    let mut i = 0usize;
    while i + 8 <= n {
        let p = u64::from_le_bytes(prev[i..i + 8].try_into().unwrap());
        let c = u64::from_le_bytes(cur[i..i + 8].try_into().unwrap());
        if p != c {
            for k in 0..8 {
                if prev[i + k] != cur[i + k] {
                    put_varint((i + k - last) as u64, &mut entries);
                    entries.push(cur[i + k]);
                    last = i + k;
                    changed += 1;
                }
            }
        }
        i += 8;
        if entries.len() >= limit {
            return usize::MAX;
        }
    }
    while i < n {
        if prev[i] != cur[i] {
            put_varint((i - last) as u64, &mut entries);
            entries.push(cur[i]);
            last = i;
            changed += 1;
            if entries.len() >= limit {
                return usize::MAX;
            }
        }
        i += 1;
    }
    put_varint(changed as u64, out);
    out.append(&mut entries);
    changed
}

fn serve_state(
    grid: &Arc<Mutex<Grid>>,
    frames: &StateFramesHandle,
    params: &HashMap<String, String>,
) -> Response<Cursor<Vec<u8>>> {
   let vx: u64 = params.get("vx").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vy: u64 = params.get("vy").and_then(|s| s.parse().ok()).unwrap_or(0);
    let vw: u32 = params.get("vw").and_then(|s| s.parse().ok()).unwrap_or(530);
    let vh: u32 = params.get("vh").and_then(|s| s.parse().ok()).unwrap_or(300);
    let scale: u32 = params.get("scale").and_then(|s| s.parse().ok()).unwrap_or(1);

    // Cap the response bitmap (post-aggregation cell count) at the effective
    // cap — min(MAX_BITMAP_CELLS, the client's per-request maxcells param) —
    // preserving aspect. The client declares its window pixel budget so the
    // cap tracks resizes / dual screens; absent/malformed values fall back to
    // the hard ceiling (previous behavior). Legitimate requests never hit the
    // cap (largest is the window's pixel count at 1px); it only triggers on
    // malformed requests.
    let scale = scale.max(1);
    let cap: u64 = params.get("maxcells")
        .and_then(|s| s.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(MAX_BITMAP_CELLS)
        .min(MAX_BITMAP_CELLS);
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
        let prod = aw.saturating_mul(ah);
        if prod <= cap { (vw, vh) }
        else if aw >= ah { ((vw as u64 * cap / prod) as u32, vh) }
        else { (vw, (vh as u64 * cap / prod) as u32) }
    };

    // Lock grid for snapshot
    let mut g = lock_grid(grid);

    let alive_count = g.engine.alive_count();

    // Fill the viewport bitmap via the live engine (each engine interprets the
    // request in its own client space); the clamp above keeps it bounded.
    let (final_bits, final_overlay, final_vw, final_vh) = g.engine.snapshot(vx, vy, vw, vh, scale);

    let base_gen = g.generation;
    let hashlife = g.is_hashlife();

    // Header: 13 big-endian u32 fields — see docs/protocol.md
    // Reserved up front: growing this by doubling costs ~20 realloc-and-copy
    // rounds for a 460 KB response, and it happens while the lock is held.
    let mut data = Vec::with_capacity(52 + final_bits.len() + final_overlay.len());
    data.extend(base_gen.to_be_bytes());
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
    data.extend((hashlife as u32).to_be_bytes());

    // The grid lock ends here. Everything below — the diff against this client's
    // previous frame and the payload copy — used to run inside it, and `/state`
    // and the `/action` step take the SAME mutex, so time spent here came
    // straight out of sim throughput (measured the hard way with the RLE codec;
    // docs/protocol.md).
    drop(g);

    let key = (vx, vy, final_vw, final_vh, scale, hashlife);
    let cid = params.get("cid").cloned().unwrap_or_default();
    let base: Option<u32> = params.get("base").and_then(|s| s.parse().ok());

    let mut table = lock_frames(frames);
    // Classic keeps its existing process: full raw frames, no diff.
    let prev = if hashlife {
        table.remove(&cid).filter(|f| {
            f.key == key && Some(f.base_gen) == base && f.bits.len() == final_bits.len()
                && final_bits.len() >= DELTA_MIN_BYTES
        })
    } else {
        table.remove(&cid);
        None
    };

    let mut delta: Vec<u8> = Vec::new();
    let mut encoded = false;
    if let Some(f) = &prev {
        // Half the bitmap is the break-even point (see use_delta below); past it
        // there is nothing to learn from finishing the scan.
        encoded = delta_encode(&f.bits, &final_bits, &mut delta, final_bits.len() / 2) != usize::MAX;
    }
    // A delta has to beat half the bitmap to be sent. That bound also keeps a v3
    // total strictly shorter than the raw total, so the client's exact-length
    // check can never mistake a delta frame for a raw one.
    let use_delta = encoded && delta.len() * 2 < final_bits.len();

    let tick = FRAME_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let frame = table.entry(cid).or_insert(ClientFrame {
        key, base_gen: 0, bits: Vec::new(), last_use: 0,
    });
    frame.key = key;
    frame.base_gen = base_gen;
    frame.bits = final_bits;
    frame.last_use = tick;

    // proto_version is the 13th header field, so it lands at offset 48 either way
    data.extend(if use_delta { 3u32 } else { 1u32 }.to_be_bytes());
    if use_delta {
        data.extend(delta);
    } else {
        data.extend(&frame.bits);
        data.extend(&final_overlay);
    }

    if table.len() > MAX_CLIENT_FRAMES {
        let mut victim: Option<(String, u64)> = None;
        for (k, f) in table.iter() {
            if victim.as_ref().map_or(true, |(_, t)| f.last_use < *t) {
                victim = Some((k.clone(), f.last_use));
            }
        }
        if let Some((k, _)) = victim { table.remove(&k); }
    }

    drop(table);
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

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return serve_bad_request("malformed JSON"),
    };
    let action = match json.get("action").and_then(|v| v.as_str()) {
        Some(a) => a,
        None => return serve_bad_request("missing action"),
    };

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
        other => {
            // Log to stderr so a broken client is visible even without the network tab.
            eprintln!("unknown action: {other}");
            return serve_bad_request(&format!("unknown action: {other}"));
        }
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

    let (x, y) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        _ => return serve_bad_request("missing x/y"),
    };
    let mut g = lock_grid(grid);
    g.engine.toggle_cell(x, y);

    serve_json(r#"{"ok":true}"#)
}

fn handle_load_pattern(
    grid: &Arc<Mutex<Grid>>,
    request: &mut tiny_http::Request,
) -> Response<Cursor<Vec<u8>>> {
    let mut body = String::new();
    let mut reader = request.as_reader();
    std::io::Read::read_to_string(&mut reader, &mut body).ok();

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return serve_bad_request("malformed JSON"),
    };
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
    } else {
        return serve_bad_request("missing cells");
    }

    serve_json(r#"{"ok":true}"#)
}

#[cfg(test)]
mod delta_tests {
    use super::*;

    /// Decoder mirroring the app.js / probe-script decoder byte-for-byte, so a
    /// passing test means the browser will reproduce the frame too.
    fn read_varint(buf: &[u8], mut off: usize) -> (u64, usize) {
        let mut v = 0u64;
        let mut shift = 0u32;
        loop {
            let b = buf[off];
            off += 1;
            v += ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 { return (v, off); }
            shift += 7;
        }
    }

    fn delta_apply(prev: &[u8], src: &[u8]) -> (Vec<u8>, usize) {
        let mut out = prev.to_vec();
        let (count, mut off) = read_varint(src, 0);
        let mut idx = 0u64;
        for _ in 0..count {
            let (d, n) = read_varint(src, off);
            off = n;
            idx += d;
            if (idx as usize) < out.len() { out[idx as usize] = src[off]; }
            off += 1;
        }
        (out, off)
    }

    fn roundtrip(prev: &[u8], cur: &[u8]) {
        let mut delta = Vec::new();
        let changed = delta_encode(prev, cur, &mut delta, usize::MAX);
        let (got, next) = delta_apply(prev, &delta);
        assert_eq!(got, cur, "reconstruction differs");
        assert_eq!(next, delta.len(), "decoder consumed {} of {} bytes", next, delta.len());
        let mut n = 0;
        let (count, _) = read_varint(&delta, 0);
        assert_eq!(count as usize, changed, "count field vs return value");
        for i in 0..prev.len() { if prev[i] != cur[i] { n += 1; } }
        assert_eq!(changed, n, "changed-byte count");
    }

    #[test]
    fn no_change_is_one_byte() {
        let a = vec![0u8; 4096];
        let mut delta = Vec::new();
        assert_eq!(delta_encode(&a, &a, &mut delta, usize::MAX), 0);
        assert_eq!(delta, vec![0u8]);
        roundtrip(&a, &a);
    }

    #[test]
    fn word_boundaries_and_edges() {
        // Changes at index 0, 7, 8, 15, 16 and the last byte: the word-wise scan
        // must not miss a byte at a u64 boundary or in the <8-byte tail.
        let prev = vec![0u8; 130];
        let mut cur = prev.clone();
        for i in [0usize, 7, 8, 15, 16, 129] { cur[i] = 0b1000_0001; }
        roundtrip(&prev, &cur);
    }

    #[test]
    fn multi_byte_index_delta() {
        // Gap of 5000 bytes between changes -> index delta needs 2 varint bytes.
        let prev = vec![0u8; 20000];
        let mut cur = prev.clone();
        cur[1] = 0xff;
        cur[5001] = 0x01;
        cur[19999] = 0x80;
        roundtrip(&prev, &cur);
        let mut delta = Vec::new();
        delta_encode(&prev, &cur, &mut delta, usize::MAX);
        assert!(delta.len() < 20, "sparse delta should stay tiny, got {}", delta.len());
    }

    #[test]
    fn dense_bitmap_encodes_every_byte() {
        // Worst case: every byte differs (0xAA -> 0x55). Delta must still decode
        // back exactly; the caller's half-size rule decides not to send it.
        let prev = vec![0xAAu8; 8192];
        let cur = vec![0x55u8; 8192];
        let mut delta = Vec::new();
        assert_eq!(delta_encode(&prev, &cur, &mut delta, usize::MAX), 8192);
        roundtrip(&prev, &cur);
        assert!(delta.len() * 2 >= cur.len(), "dense delta must be rejected by the size rule");
    }

    #[test]
    fn pseudo_random_frame_pair() {
        // Two "frames" from a cheap LCG: ~2% of bytes differ, like a live viewport.
        let n = 120_000;
        let mut s = 0x5DEECE66Du64;
        let mut prev = vec![0u8; n];
        let mut cur = vec![0u8; n];
        for i in 0..n {
            s = (s * 6364136223846793005 + 1442695040888963407) & 0xffff_ffff_ffff_ffff;
            prev[i] = (s >> 33) as u8;
            s = (s * 6364136223846793005 + 1442695040888963407) & 0xffff_ffff_ffff_ffff;
            let b = (s >> 33) as u8;
            cur[i] = if i % 50 == 0 { b } else { prev[i] };
        }
        roundtrip(&prev, &cur);
        let mut delta = Vec::new();
        delta_encode(&prev, &cur, &mut delta, usize::MAX);
        assert!(delta.len() * 2 < cur.len(), "sparse delta should win the size rule");
    }

    #[test]
    fn bails_once_the_delta_cannot_win() {
        // Every byte differs, and the caller's limit is half the bitmap: the scan
        // must stop early, report usize::MAX, and leave `out` untouched so the
        // caller cannot mistake a truncated stream for a valid delta.
        let prev = vec![0xAAu8; 8192];
        let cur = vec![0x55u8; 8192];
        let mut delta = Vec::new();
        assert_eq!(delta_encode(&prev, &cur, &mut delta, cur.len() / 2), usize::MAX);
        assert!(delta.is_empty(), "aborted encode must leave out empty, got {} bytes", delta.len());
    }
}
