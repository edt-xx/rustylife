# /state wire protocol

Endpoint: `GET /state?vx=<u64>&vy=<u64>&vw=<u32>&vh=<u32>&scale=<u32>` (+ optional `&maxcells=<u64>`)
Response: `application/octet-stream` — a big-endian u32 header followed by two packed bitmaps.

Defaults: vx=0, vy=0, vw=530, vh=300, scale=1. Coordinates are frontend-space; the server converts them internally in HashLife mode. maxcells omitted/malformed → the hard ceiling (64M) applies.

## Header — 13 fields × 4 bytes (52 bytes), big-endian u32

| Off | Field | Classic engine | HashLife engine |
|-----|-------|----------------|-----------------|
| 0   | gen | current generation | current generation |
| 4   | vw | bitmap width: requested vw, or ceil(vw/scale) when scale > 1 | bitmap width: aligned superset, ceil-divided by scale |
| 8   | vh | bitmap height (same rule) | bitmap height (same rule) |
| 12  | pop | `alive.len()` | `alive_count()` |
| 16  | active | `active_count` — cells processed in the last step | `last_gc_live_len` |
| 20  | ol_len | overlay length in bytes | overlay length in bytes (overlay is all zeros) |
| 24  | births | births in the last step | **stale** — written only by the classic engine; frontend hides it |
| 28  | deaths | deaths in the last step | **stale** — same as births |
| 32  | heap | node heap size | `last_cache_size` (slow cache) |
| 36  | tiles | `active_tiles.len()` | `last_cache_hit_rate` |
| 40  | scale | aggregation scale used (1 = none) | same |
| 44  | hashlife_mode | 1 = HashLife, 0 = classic | |
| 48  | proto_version | header format revision — currently **1** | |

## Mode-dependent fields

The same offset carries a different meaning in each mode. Read `hashlife_mode` (offset 44) before interpreting:

- `vw`/`vh` (4/8) — both report the actual bitmap dimensions, but HashLife mode aligns the viewport to byte-aligned seams (a superset) before optional aggregation, so reported dimensions can exceed the request.
- `active` (16) — classic: cells processed in the last step; HashLife: last GC live length.
- `births`/`deaths` (24/28) — written only by the classic engine; stale/zero in HashLife mode (frontend hides them).
- `heap` (32) — classic: node heap size; HashLife: slow cache size.
- `tiles` (36) — classic: number of active tiles; HashLife: slow cache hit rate.

All other fields (`gen`, `pop`, `ol_len`, `scale`, `hashlife_mode`, `proto_version`) have the same meaning in both modes.

## Payload

1. **bits** — `(vw*vh+7)/8` bytes. Bit index `idx = y*vw + x`; bit `(idx & 7)` of byte `idx >> 3` (bit 0 of the first byte = leftmost cell of the first row). 1 = cell (vx+x, vy+y) alive.
2. **overlay** — `ol_len` bytes, same bit layout. Classic: cells inside active 4×4 tiles (`STATIC_SIZE` = 4). HashLife: present but all zeros.

## Response bitmap cap

The server caps the response bitmap — the requested post-aggregation cell count `ceil(vw/scale) x ceil(vh/scale)`, plus the alignment edge buffer (`align_viewport` adds up to 15 aggregated cells on x — byte-alignment seam shift — and 8 on y — row-alignment seam shift — before padding both to multiples of 8) — at the effective cap `min(MAX_BITMAP_CELLS, maxcells)` (the optional per-request `maxcells` param is the client's declared window pixel budget; `MAX_BITMAP_CELLS` = 64M cells = 8 MiB is the hard ceiling), preserving aspect (the larger dimension is scaled down). This stops a malformed request from forcing a huge allocation: `vec!` aborts on OOM, which the request `catch_unwind` cannot catch.

Legitimate requests never hit the cap: the largest response bitmap is the window's pixel count at 1px zoom, and every sub-pixel zoom level (1/2 ... 1/1024) sends that same window-pixel-sized bitmap (the raw request dims grow, but the server allocates only the aggregated bitmap).

The cap value is sent to the frontend at serve time: the server injects it into the page as the JS constant `MAX_BITMAP_CELLS` (the served HTML contains the resolved number).

## Version detection (client)

- Old format (no proto_version): 48-byte header.
- Exact total length disambiguates: `byteLength == 52 + bitsLen + ol_len` → v1; otherwise old 48-byte header.
- If proto_version is present and ≠ 1, the client should surface a warning rather than render (the current frontend appends `⚠ proto v<N` to the status line).

## Versioning policy

- Future layout changes bump `proto_version`. New fields should be appended at the end so old clients keep parsing the fields they know by position.
- The exact-length detection works for v0 → v1. From v2 on, clients should read proto_version at offset 48 whenever the total length matches no known revision, and warn instead of guessing.
