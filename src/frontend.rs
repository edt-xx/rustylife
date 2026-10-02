//! Frontend assets, auto-extracted from game_of_life.py, split into separate
//! files 2026-10-02. The served page is a single inline HTML document:
//! `serve_index` (server.rs) fills the `__CSS__` / `__JS__` placeholders in
//! `index.html` from these files, injecting `__MAX_BITMAP_CELLS__` into
//! `app.js` (the `/state` protocol cap — see docs/protocol.md).
pub const FRONTEND_HTML: &str = include_str!("../frontend/index.html");
pub const FRONTEND_CSS: &str = include_str!("../frontend/style.css");
pub const FRONTEND_JS: &str = include_str!("../frontend/app.js");
