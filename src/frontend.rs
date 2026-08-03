// Auto-extracted from game_of_life.py
pub const FRONTEND: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Game of Life</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
html,body{background:#1a1a2e;width:100%;height:100%;font-family:monospace;color:#e94560}
#viewport{position:absolute;top:0;left:0;right:0;bottom:0}
canvas#main{position:absolute;top:48px;left:16px;right:12px;border:2px solid #e94560;image-rendering:pixelated;background:#1a1a2e;cursor:crosshair;margin-bottom:64px;transform:translate3d(0,0,0);will-change:transform}
.toolbar{position:absolute;bottom:12px;left:16px;right:12px;display:flex;gap:8px;align-items:center;padding:6px 14px;background:rgba(22,33,62,.9);border-radius:6px;z-index:10}
.toolbar-content{flex:1;display:flex;gap:8px;align-items:center;justify-content:center;flex-wrap:wrap}
@media(max-width:2000px){.toolbar-content{justify-content:flex-start}}
button,label{background:#16213e;color:#e94560;border:1px solid #e94560;padding:5px 12px;cursor:pointer;font-family:monospace;font-size:13px;border-radius:3px}
button:hover{background:#e94560;color:#1a1a2e}
input[type=range]{width:100px;vertical-align:middle}
.info{font-size:12px;color:#888;margin-left:4px}
#topInfo{position:absolute;top:8px;left:16px;right:12px;font-family:monospace;font-size:12px;color:#888;z-index:10;display:flex;flex-direction:column;align-items:center;gap:2px}
#infoLine1,#infoLine2{white-space:nowrap;text-align:center}
#pasteModal{display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,.7);z-index:100;justify-content:center;align-items:center}
#pasteModal.active{display:flex}
#pasteModal .modal-content{background:#16213e;border:2px solid #e94560;border-radius:6px;padding:16px;width:80%;max-width:700px;height:70vh;display:flex;flex-direction:column;gap:8px}
#pasteModal textarea{flex:1;background:#1a1a2e;color:#e94560;border:1px solid #e94560;border-radius:3px;padding:8px;font-family:monospace;font-size:12px;resize:none}
#pasteModal .modal-buttons{display:flex;gap:8px;justify-content:flex-end}
#bookmarkMenu{display:none;position:fixed;z-index:200;background:#16213e;border:2px solid #e94560;border-radius:6px;padding:8px;min-width:140px}
#bookmarkMenu.active{display:block}
#bookmarkMenu button{display:block;width:100%;margin:2px 0;padding:4px 8px;font-size:12px}
#helpModal{display:none;position:fixed;top:0;left:0;right:0;bottom:0;background:rgba(0,0,0,.7);z-index:100;justify-content:center;align-items:center}
#helpModal.active{display:flex}
#helpModal .modal-content{background:#16213e;border:2px solid #e94560;border-radius:6px;padding:16px;width:80%;max-width:700px;height:70vh;display:flex;flex-direction:column;gap:8px}
#helpModal .help-text{flex:1;background:#1a1a2e;color:#ccc;border:1px solid #e94560;border-radius:3px;padding:8px;font-family:monospace;font-size:12px;overflow-y:auto;white-space:pre-wrap}
#helpModal .modal-buttons{display:flex;gap:8px;justify-content:flex-end}
</style></head><body>
<div id="topInfo">
  <div id="infoLine1">
    <span id="genLabel"></span>&nbsp;&nbsp;
    <span id="fpsLabel"></span>&nbsp;&nbsp;
    <span id="birthsLabel"></span>&nbsp;&nbsp;
    <span id="deathsLabel"></span>&nbsp;&nbsp;
    <span id="popLabel"></span>&nbsp;&nbsp;
    <span id="heapLabel"></span>
  </div>
  <div id="infoLine2">
    <span id="zoomLabel"></span>&nbsp;&nbsp;
    <span id="camLabel"></span>
  </div>
</div>
<div id="viewport"><canvas id="main"></canvas></div>
<div id="pasteModal">
  <div class="modal-content">
    <textarea id="pasteArea" placeholder="Paste .lif, .rle, or .mc pattern here..."></textarea>
    <input type="file" id="readFileInput" accept=".lif,.txt,.rle,.mc" style="display:none"/>
    <div class="modal-buttons">
      <button id="pasteSaveBtn">Save</button>
      <button id="pasteReadBtn">Read</button>
      <button id="pasteCopyBtn">Copy</button>
      <button id="pasteOkBtn">Paste</button>
      <button id="pasteClearBtn">Clear</button>
      <button id="pasteCancelBtn">Cancel</button>
    </div>
  </div>
</div>
<div id="helpModal">
  <div class="modal-content">
    <div class="help-text" id="helpText"></div>
    <div class="modal-buttons">
      <button id="helpOkBtn">Close</button>
    </div>
  </div>
</div>
<div id="bookmarkMenu">
  <button id="bmSaveA">Save A</button>
  <button id="bmSaveB">Save B</button>
  <button id="bmSaveC">Save C</button>
  <button id="bmGotoA">Goto A</button>
  <button id="bmGotoB">Goto B</button>
  <button id="bmGotoC">Goto C</button>
</div>
<div class="toolbar">
  <button id="helpBtn">Help</button>
  <div class="toolbar-content">
    <button id="playBtn">&#9654; Play</button>
    <button id="stepBtn">Step</button>
    <label>Speed <input type="range" id="speedSlider" min="1" max="50" value="25"></label>
    <label>Step <input type="range" id="stepSlider" min="0" max="11" value="0"> <span id="stepVal">1</span></label>
    <button id="stepPlusBtn">Step+1</button>

    <button id="randBtn">Randomize</button>
    <button id="clearBtn">Clear</button>
    <button id="loadBtn">Load</button>
    <button id="pasteBtn">Paste</button>
    <input type="file" id="fileInput" accept=".lif,.txt,.rle,.mc" style="display:none"/>
    <button id="tracksBtn">Tracks</button>
    <button id="hashlifeBtn">Classic</button>
  </div>
  <button id="quitBtn">Quit</button>
</div>
<script>
const canvas = document.getElementById('main'), ctx = canvas.getContext('2d');

const step = [1,4,16,32,64,256,512,1024,4096,16384,65536,131072];

var ZOOM_LEVELS = [15,14,13,12,11,10,9,8,7,6,5,4,3,2,1,0.5,0.25,0.125,0.0625,0.03125,0.015625,0.0078125]; // 1/64
var zoomIdx = 12; // default to cellSize=3 (ZOOM_LEVELS[12] == 3)
var cellSize = ZOOM_LEVELS[zoomIdx];
var HELP_TEXT = '# Game of Life Simulator\n\nA Conway\'s Game of Life implementation with HashLife optimization and a canvas-based frontend.\n\n## How to Run\n\nStart the server: `cargo run --release`\n\nOpen a browser at `http://localhost:7654`\n\n## Controls\n\n### Playback\n- **Play** - Start/stop animation\n- **Step** - Advance one generation (or multiple based on Step slider)\n- **Speed slider** - Control animation speed (1-50)\n- **Step slider** - Set generations per step (1 to 131072)\n- **Step+1** - Increase step count level by one\n\n### Patterns\n- **Randomize** - Fill viewport with random cells\n- **Clear** - Remove all cells\n- **Load** - Load .lif/.rle/.txt/.mc files\n- **Paste** - Paste RLE text from clipboard\n\n### Display\n- **Tracks** - Toggle birth/death overlay colors\n- **HashLife/Classic** - Toggle between HashLife and classic algorithms\n- **Quit** - Shut down the server\n\n## Mouse Commands\n\n- **Left click** - Toggle a cell (live/dead)\n- **Left drag** - Draw multiple cells\n- **Right click** - Release to recenter viewport on clicked cell\n- **Right drag** - Pan the viewport\n- **Right double-click** - Position bookmarks menu (Save/Goto A, B, C)\n- **Both buttons** - Hold both and drag up/down to zoom\n- **Mouse wheel** - Zoom in/out\n\n## Keyboard\n\n- **+ / =** - Zoom in\n- **-** - Zoom out\n- **Escape** - Close popup menus\n\n## Position Bookmarks\n\nRight-double-click to open the position bookmarks menu. Save A, B, or C to store the current viewport center. Goto A, B, or C to jump to a saved position. Bookmarks reset when you load a pattern, paste, or randomize.\n\n## Acknowledgments\n\nThe HashLife implementation is based on GOLDE (Game Of Life Development Environment) by RyanJK5.\nGOLDE: https://github.com/RyanJK5/GOLDE';
var camX = 2000000000, camY = 2000000000;         // top-left of viewport in grid coords

// Fixed frame: fill ~90% of window, aspect ratio constrained to 16:9
function calcFrameSize() {
    var ww = window.innerWidth - 32; // left 16 + right 12 + 4px border
    var wh = window.innerHeight - 112; // top 48 + bottom 64
    return {fw: ww, fh: wh};
}

// How many cells fit in the frame — fill available space without aspect ratio constraint
function calcCells(cs) {
    var fs = calcFrameSize();
    var vw = Math.max(2, Math.floor(fs.fw / cs));
    var vh = Math.max(2, Math.floor(fs.fh / cs));
    return {vw: vw, vh: vh};
}

const offCanvas = document.createElement('canvas');
const offCtx    = offCanvas.getContext('2d');

function call(a) {
    return fetch('/action', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(a)});
}
function toggleCell(gx, gy) {
    return fetch('/toggle', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({x:gx, y:gy})});
}

// RLE parser — .lif / .rle format, based on Golly specification
function parseRLE(text) {
    var cells = [];
    var x = 0, y = 0;
    var count = '';

    // Strip comments (lines starting with #)
    text = text.replace(/#.*$/gm, '');
    // Strip RLE header lines (x = ..., y = ..., rule = ...) before removing whitespace
    text = text.replace(/^[xy]\s*=\s*.*/gm, '');
    // Strip the rule= part if it appears on the same line as x/y
    text = text.replace(/,\s*rule\s*=\s*[^,]*/gi, '');
    // Remove all whitespace
    text = text.replace(/\s/g, '');

    for (var i = 0; i < text.length; i++) {
        var c = text[i];
        if (c >= '0' && c <= '9') { count += c; continue; }

        var n = count ? parseInt(count) : 1;
        count = '';

        if (c === 'o') {
            for (var j = 0; j < n; j++) { cells.push([x, y]); x++; }
        } else if (c === 'b') {
            x += n;
        } else if (c === '$') {
            x = 0; y += n;
        } else if (c === '!') {
            break;
        }
    }
    return cells;
}

// Macrocell (.mc) parser — Golly HashLife format
// Child-first quadtree: leaf nodes (8x8 grids) and non-leaf nodes (level nw ne sw se)
function parseMacrocell(text) {
    var lines = text.split('\n');
    if (!lines[0] || !lines[0].startsWith('[M2]')) {
        return null; // not a macrocell file
    }

    // Parse header — skip comment lines, extract origin if present
    var lineIdx = 1;
    var originX = null, originY = null;
    while (lineIdx < lines.length) {
        var line = lines[lineIdx].trim();
        if (line === '') { lineIdx++; continue; }
        if (line.startsWith('#')) {
            // Check for origin offset comment
            var m = line.match(/origin\s*=\s*(-?\d+)\s+(-?\d+)/);
            if (m) { originX = parseInt(m[1]); originY = parseInt(m[2]); }
            lineIdx++; continue;
        }
        break;
    }

    // Parse tree — child-first ordering, nodes numbered from 1
    var nodes = [null]; // 1-based: nodes[1] = first node (null at 0 for empty node)
    var cells = [];

    while (lineIdx < lines.length) {
        var line = lines[lineIdx];
        // Strip trailing \r
        if (line.length > 0 && line[line.length - 1] === '\r') line = line.slice(0, -1);
        line = line.trim();
        if (line === '') { lineIdx++; continue; }
        lineIdx++;

        var first = line[0];

        if (first === '.' || first === '*' || first === '$') {
            // Leaf node — parse 8x8 grid
            var grid = [];
            var row = 0, col = 0;
            for (var r = 0; r < 8; r++) grid[r] = [false, false, false, false, false, false, false, false];

            for (var ci = 0; ci < line.length; ci++) {
                var c = line[ci];
                if (c === '.') {
                    if (col < 8) grid[row][col] = false;
                    col++;
                } else if (c === '*') {
                    if (col < 8) grid[row][col] = true;
                    col++;
                } else if (c === '$') {
                    row++;
                    col = 0;
                }
            }
            nodes.push({type: 'leaf', grid: grid});
        } else if (first >= '0' && first <= '9') {
            // Non-leaf node — "level nw ne sw se"
            var parts = line.split(/\s+/);
            var level = parseInt(parts[0]);
            var nw = parseInt(parts[1]);
            var ne = parseInt(parts[2]);
            var sw = parseInt(parts[3]);
            var se = parseInt(parts[4]);
            nodes.push({type: 'node', level: level, nw: nw, ne: ne, sw: sw, se: se});
        }
    }

    // Expand root (last node) to extract live cells
    // Convention: upper-left of SE child of root is at (0, 1)
    // We extract all live cells with absolute coordinates
    function expandNode(nodeNum, originX, originY, size) {
        if (nodeNum === 0) return; // empty node
        var node = nodes[nodeNum];
        if (!node) return;

        if (node.type === 'leaf') {
            // Leaf is level 3 = 8x8
            var grid = node.grid;
            for (var r = 0; r < 8; r++) {
                for (var c = 0; c < 8; c++) {
                    if (grid[r][c]) {
                        cells.push([originX + c, originY + r]);
                    }
                }
            }
        } else {
            var halfSize = size / 2;
            expandNode(node.nw, originX, originY, halfSize);
            expandNode(node.ne, originX + halfSize, originY, halfSize);
            expandNode(node.sw, originX, originY + halfSize, halfSize);
            expandNode(node.se, originX + halfSize, originY + halfSize, halfSize);
        }
    }

    var rootNum = nodes.length - 1; // 1-based, null at index 0
    var rootNode = nodes[rootNum];
    var rootLevel = rootNode.type === 'leaf' ? 3 : rootNode.level;
    var rootSize = Math.pow(2, rootLevel);
    // Use explicit origin if present, otherwise fall back to Golly convention
    if (originX === null) {
        originX = -rootSize;
        originY = -rootSize + 1;
    }
    expandNode(rootNum, originX, originY, rootSize);

    return cells;
}

// Fixed frame size tracking — only resize when dimensions actually change (setting .width/.height clears canvas!)
var canvasW = 0, canvasH = 0;

function ensureCanvasSize() {
    var fs = calcFrameSize();
    if (fs.fw !== canvasW || fs.fh !== canvasH) {
        canvasW = fs.fw; canvasH = fs.fh;
        var dpr = window.devicePixelRatio || 1;
        canvas.width  = canvasW * dpr;
        canvas.height = canvasH * dpr;
        canvas.style.width  = canvasW + 'px';
        canvas.style.height = canvasH + 'px';
        ctx.setTransform(1, 0, 0, 1, 0, 0); // reset any previous scale
        ctx.scale(dpr, dpr);
    }
}

// Persistent buffers — reused across frames to avoid reallocation
var prevBits = null, imgData = null, prevOverlay = null;
var prevVw = 0, prevVh = 0;
var prevCamX = -1, prevCamY = -1;  // track pan to detect viewport shift
// Global label data accessible from updateLabels
var lblGen=0, lblPop=0, lblActive=0, lblBirths=0, lblDeaths=0, lblHeap=0, lblTiles=0;
var refreshSeq = 0; // sequence guard: discard stale async responses
var zoomRefreshBusy = false; // serial: only one zoomRefresh at a time
var tracksEnabled = false; // disabled by default, hide active overlay when on
var hashlifeMode = true; // tracks hashlife mode toggle
var stepCountVal = 1; // current step count (slider value + manual adjustments)
var currentScale = 1; // server-side aggregation scale from last drawGrid
var currentVw = 0, currentVh = 0; // viewport dimensions from server (aggregated when scale>1)

// Bookmarks: save/restore viewport centers
var bookmarks = {A: null, B: null, C: null}; // {x, y} grid center coords
var rightTimer = null; // 250ms timer
var rightDownTarget = null; // {cellX, cellY} for pending recenter
var pendingRecenter = false; // set on first right up, acted on when timer fires
var lastRightMoveX = 0, lastRightMoveY = 0; // track mouse position during right-click
var bookmarkMenuOpen = false;

function getViewportCenter() {
    var vp = calcCells(cellSize);
    return {x: camX + Math.floor(vp.vw / 2), y: camY + Math.floor(vp.vh / 2)};
}
function closeBookmarkMenu() {
    bookmarkMenuOpen = false;
    document.getElementById('bookmarkMenu').classList.remove('active');
}
function saveBookmark(label) {
    var center = getViewportCenter();
    bookmarks[label] = {x: center.x, y: center.y};
    closeBookmarkMenu();
}
function gotoBookmark(label) {
    var bm = bookmarks[label];
    if (!bm) { alert('Bookmark ' + label + ' not set. Right-double-click and Save ' + label + ' first.'); return; }
    var vp = calcCells(cellSize);
    camX = bm.x - Math.floor(vp.vw / 2);
    camY = bm.y - Math.floor(vp.vh / 2);
    closeBookmarkMenu();
    zoomRefresh();
}
function doRightTimer() {
    if (pendingRecenter && !panning) {
        // Timer expired after first mouseup: recenter
        camX = rightDownTarget.cellX - Math.floor(calcCells(cellSize).vw / 2);
        camY = rightDownTarget.cellY - Math.floor(calcCells(cellSize).vh / 2);
        zoomRefresh();
    } else if (!panning) {
        // Timer expired with no mouseup: start panning
        panning = true;
        camStartX = camX; camStartY = camY;
    }
    pendingRecenter = false;
    rightDownTarget = null;
    rightTimer = null;
}
function cancelRightTimer() {
    clearTimeout(rightTimer);
    rightTimer = null;
    pendingRecenter = false;
    rightDownTarget = null;
}

function drawGrid(data) {
    var hdr  = new DataView(data, 0, 44);
    var gen     = hdr.getUint32(0, false);
    var vw      = hdr.getUint32(4, false);
    var vh      = hdr.getUint32(8, false);
    var pop     = hdr.getUint32(12, false);
    var active  = hdr.getUint32(16, false);
    var ol_len  = hdr.getUint32(20, false);
    var births  = hdr.getUint32(24, false);
    var deaths  = hdr.getUint32(28, false);
    var heap    = hdr.getUint32(32, false);
    var tiles   = hdr.getUint32(36, false);
    var serverScale = hdr.getUint32(40, false);
    currentScale = serverScale;
    currentVw = vw; currentVh = vh;

    // Discard stale response if server scale doesn't match expected
    var expectedScale = cellSize < 1 ? Math.round(1 / cellSize) : 1;
    if (serverScale !== expectedScale) return;

    // Set globals for updateLabels()
    lblGen = gen; lblPop = pop; lblActive = active;
    lblBirths = births; lblDeaths = deaths; lblHeap = heap; lblTiles = tiles;

    var bitsLen = ((vw * vh + 7) >> 3);
    var bits = new Uint8Array(data, 44, bitsLen);
    var overlayOff = 44 + bitsLen;
    var overlay = ol_len > 0 ? new Uint8Array(data, overlayOff, ol_len) : new Uint8Array(0);

    // Server-side aggregation: when serverScale > 1, the server sends an already-aggregated bitmap.
    // vw/vh in the header are the aggregated dimensions. Just render normally.
    // Decide: full redraw (viewport/zoom changed or first call) vs delta update
    // At sub-pixel zoom, server sends aggregated bitmap — delta works fine (1px fillRect)
    var needsFullRedraw = (vw !== prevVw || vh !== prevVh || camX !== prevCamX || camY !== prevCamY || !imgData);

    // Only resize offCanvas when needed — setting .width/.height clears it!
    if (offCanvas.width !== vw || offCanvas.height !== vh) {
        offCanvas.width  = vw;
        offCanvas.height = vh;
    }

    if (needsFullRedraw) {
        // Full redraw: iterate per-byte (8x fewer loop iterations)
        imgData = offCtx.createImageData(vw, vh);
        var px = imgData.data;
        var total = vw * vh;
        var bitsLen = (total + 7) >> 3;
        var hasOverlay = overlay.length > 0 && !tracksEnabled;

        for (var b = 0; b < bitsLen; b++) {
            var bitMask = bits[b];
            var overlayMask = hasOverlay ? overlay[b] : 0;
            var startIdx = b << 3;
            for (var bit = 0; bit < 8; bit++) {
                var idx = startIdx + bit;
                if (idx >= total) break;
                var p = idx * 4;
                if (bitMask & (1 << bit)) {
                    px[p]=233; px[p+1]=69; px[p+2]=96;
                } else if (hasOverlay && (overlayMask & (1 << bit))) {
                    px[p]=34; px[p+1]=34; px[p+2]=58;
                } else {
                    px[p]=26; px[p+1]=26; px[p+2]=46;
                }
                px[p+3] = 255;
            }
        }
        ctx.fillStyle = '#1a1a2e';
        ctx.fillRect(0, 0, canvasW, canvasH);

        offCtx.putImageData(imgData, 0, 0);
        ctx.imageSmoothingEnabled = false;
        // Internal pixel coords — use cellSize * scale for aggregated mode
        var dSize = cellSize * serverScale; // CSS px per aggregated pixel
        var dpr = window.devicePixelRatio || 1;
        var dSizei = Math.ceil(dSize * dpr);
        var cwActual = vw * dSizei;
        var chActual = vh * dSizei;
        var oxInternal = Math.floor((canvasW * dpr - cwActual) / 2);
        var oyInternal = Math.floor((canvasH * dpr - chActual) / 2);
        ctx.save();
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.drawImage(offCanvas, oxInternal, oyInternal, cwActual, chActual);
        ctx.restore();
        if (overlay.length > 0) { var copyOv2 = new Uint8Array(overlay.length); copyOv2.set(overlay); prevOverlay = copyOv2; }
    } else {
        // Delta: draw ONLY changed cells directly on main canvas
        // (avoids putImageData of megabytes per frame at 1px zoom)
        var total = vw * vh;
        var numBytes = (total + 7) >> 3;

        // Aggregated bitmap — each pixel is cellSize*scale CSS pixels
        var dSize = cellSize * serverScale;
        var dpr = window.devicePixelRatio || 1;
        var dSizei = Math.ceil(dSize * dpr);
        var cwActual = vw * dSizei;
        var chActual = vh * dSizei;
        var ox2i = Math.floor((canvasW * dpr - cwActual) / 2);
        var oy2i = Math.floor((canvasH * dpr - chActual) / 2);

        // Collect changes by color to minimize fillStyle switches
        var aliveCells = [];
        var deadCells = [];

        for (var b = 0; b < numBytes; b++) {
            var diff = bits[b] ^ prevBits[b];
            if (diff === 0) continue;

            var startPixel = b << 3;
            for (var bit = 0; bit < 8; bit++) {
                var idx = startPixel + bit;
                if (idx >= total) break;
                if (!(diff & (1 << bit))) continue;

                var row = Math.floor(idx / vw);
                var col = idx - row * vw;
                if ((bits[b] >> bit) & 1) aliveCells.push(col, row);
                else deadCells.push(col, row);
            }
        }

        if (aliveCells.length === 0 && deadCells.length === 0) {
            prevBits = bits;
            updateLabels(vw, vh);
            return;
        }

        // Draw in internal pixel coords to avoid sub-pixel anti-aliasing from DPR scale
        ctx.save();
        ctx.setTransform(1, 0, 0, 1, 0, 0);

        // Batch draw: dead cells first — show tracks or restore bg
        for (var i = 0; i < deadCells.length; i += 2) {
             var di = deadCells[i+1] * vw + deadCells[i];
            if (tracksEnabled && camX === prevCamX && camY === prevCamY) ctx.fillStyle = '#3a3a5a'; // track color only on static frames, not pan
            else ctx.fillStyle = (overlay.length > 0 && ((overlay[di >>> 3] >> (di & 7)) & 1)) ? '#22223a' : '#1a1a2e';
            ctx.fillRect(ox2i + deadCells[i]*dSizei, oy2i + deadCells[i+1]*dSizei, dSizei, dSizei);
        }

        ctx.fillStyle = '#e94560';
        for (var i = 0; i < aliveCells.length; i += 2)
            ctx.fillRect(ox2i + aliveCells[i] * dSizei, oy2i + aliveCells[i+1] * dSizei, dSizei, dSizei);
        ctx.restore();

        // Redraw background cells whose tile status changed (overlay update) — skip when tracks on
        if (!tracksEnabled && prevOverlay !== null && overlay.length > 0) {
            var bgLight = [], bgDark = [];
            for (var b2 = 0; b2 < numBytes; b2++) {
                var diff2 = overlay[b2] ^ prevOverlay[b2];
                if (diff2 === 0) continue;
                var startPx2 = b2 << 3;
                for (var bit2 = 0; bit2 < 8; bit2++) {
                    var idx2 = startPx2 + bit2;
                    if (idx2 >= total) break;
                    if (!(diff2 & (1 << bit2))) continue;
                    // Only redraw dead cells (alive cells keep pink color)
                    if (!((bits[b2] >> bit2) & 1)) {
                        var row2 = Math.floor(idx2 / vw);
                        var col2 = idx2 - row2 * vw;
                        if ((overlay[b2] >> bit2) & 1) bgLight.push(col2, row2);
                        else bgDark.push(col2, row2);
                    }
                }
            }
            ctx.save();
            ctx.setTransform(1, 0, 0, 1, 0, 0);
            ctx.fillStyle = '#22223a';
            for (var i = 0; i < bgLight.length; i += 2)
                ctx.fillRect(ox2i + bgLight[i]*dSizei, oy2i + bgLight[i+1]*dSizei, dSizei, dSizei);
            ctx.fillStyle = '#1a1a2e';
            for (var i = 0; i < bgDark.length; i += 2)
                ctx.fillRect(ox2i + bgDark[i]*dSizei, oy2i + bgDark[i+1]*dSizei, dSizei, dSizei);
            ctx.restore();
        }
    }

    prevBits = bits;

    prevVw = vw; prevVh = vh; prevCamX = camX; prevCamY = camY;
    if (overlay.length > 0) { var copyOv = new Uint8Array(overlay.length); copyOv.set(overlay); prevOverlay = copyOv; } else { prevOverlay = null; }
    updateLabels(vw, vh);
}

function updateLabels(vw, vh) {
    var l1 = document.getElementById('infoLine1');
    if (l1) {
        l1.children[0].textContent = 'Gen: ' + lblGen;
        // fpsLabel updated in animLoop
        l1.children[2].textContent = 'Births: ' + lblBirths;
        l1.children[3].textContent = 'Deaths: ' + lblDeaths;
        l1.children[4].textContent = 'Pop: ' + lblPop + ' (' + lblActive + ')';
        if (hashlifeMode) {
            var rate = (lblTiles / 10.0).toFixed(1) + '%';
            l1.children[5].textContent = 'Cache: ' + lblHeap + ' (' + rate + ')';
        } else {
            l1.children[5].textContent = 'Heap: ' + lblHeap + ' (' + lblTiles + ')';
        }
    }
    var l2 = document.getElementById('infoLine2');
    if (l2) {
        var zoomTxt = 'Zoom: ' + cellSize + 'px | ' + vw + '\u00d7' + vh;
        l2.children[0].textContent = zoomTxt;
        l2.children[1].textContent = 'Cam: ' + camX + ', ' + camY;
    }
}

async function refresh() {
    ensureCanvasSize();  // only resizes when dimensions actually changed
    var vp = calcCells(cellSize);
    var scale = cellSize < 1 ? Math.round(1 / cellSize) : 1;
    var url = '/state?vx=' + camX + '&vy=' + camY + '&vw=' + vp.vw + '&vh=' + vp.vh + '&scale=' + scale;
    var seq = ++refreshSeq;

    // Wait until gen has advanced (avoid getting stale cached state)
    var initialGen = lblGen;
    while (true) {
        var r = await fetch(url);
        var data = await r.arrayBuffer();  // read fully before seq check — prevents stale overwrite
        if (seq !== refreshSeq) return;   // stale response, discard
        var hdr = new DataView(data, 0, 44);
        var gen = hdr.getUint32(0, false);
        if (gen !== initialGen) {
            drawGrid(data);
            return;
        }
        // Gen hasn't advanced — Python still stepping, wait and retry
        await new Promise(function(resolve) { setTimeout(resolve, 5); });
    }
}

// Zoom refresh: doesn't touch refreshSeq so it won't cancel pending animLoop requests
// Serial: only one active at a time. During pan, loops until panning stops.
async function zoomRefresh() {
    if (zoomRefreshBusy) return;
    zoomRefreshBusy = true;
    try {
        while (true) {
            ensureCanvasSize();
            var vp = calcCells(cellSize);
            var scale = cellSize < 1 ? Math.round(1 / cellSize) : 1;
            var url = '/state?vx=' + camX + '&vy=' + camY + '&vw=' + vp.vw + '&vh=' + vp.vh + '&scale=' + scale;
            var r = await fetch(url);
            var data = await r.arrayBuffer();
            drawGrid(data);
            // If still panning, keep refreshing with latest cam position
            if (!panning) break;
        }
    } finally {
        zoomRefreshBusy = false;
    }
}

async function doQuit() { stopAnim(); await fetch('/action', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({action:'quit'})}); }

// ---- Pan: Shift+drag or right-click drag. Right-click: recenter on mouseup ----
var panning = false, pStartX, pStartY, camStartX, camStartY;
var wasPlayingBeforePan = false; // remember playback state during pan
var leftHeld = false, rightHeld = false; // track which buttons are held
var zoomDragStartY, zoomDragDelta = 0; // zoom drag state (both buttons held)
var rcPending = null; // {cellX, cellY} — right-click held alone, awaiting recenter vs pan
var leftClickPending = null; // {cellX, cellY} — deferred toggle, fired on mouseup if no drag
var leftDragActive = false; // track if left button dragged
var zoomDragThreshold = 20; // px before zoom activates
var zoomDragArmed = false; // threshold crossed, zooming active

// Global so play/stop functions can be called from anywhere
var running = false;
var lastGenCount = 0, lastTime = 0, lastGenNum = 0; // for G/s calculation
var tracksBeforePan = false; // restore tracks after pan

function stopAnim() { running = false; document.getElementById('playBtn').innerHTML = '&#9654; Play'; fetch('/action', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({action:'stop'})}); }

function startAnim() { running = true; document.getElementById('playBtn').innerHTML = '&#9638; Stop'; lastGenNum = lblGen; lastGenCount = 0; lastTime = performance.now(); animLoop(); }

var animLoopPending = false; // prevent multiple simultaneous dispatches
async function animLoop() {
    var speedEl = document.getElementById('speedSlider');
    var stepEl = document.getElementById('stepSlider');
    if (!running || animLoopPending) return; // block while still processing previous step

    var stepCount = stepCountVal;
    var startTime = performance.now();

    animLoopPending = true; // block new dispatches until this iteration completes
    try {
        if (stepCount > 1) {
            // Batch-step: step on server, then fetch snapshot
            await call({action:'batch-step', count: stepCount});
            await refresh();
     } else {
            // Use zoomRefresh (no gen-wait loop) to avoid stale data issues with keep-alive
            await call({action:'step'});
            zoomRefresh();
        }

        // If Stop was pressed while awaiting, exit loop
        if (!running) return;
    } finally {
        animLoopPending = false; // allow next iteration to dispatch
    }

    var elapsed = performance.now() - startTime;

    // G/s: measure actual generation progress from server-reported gen numbers
    var now = performance.now();
    if (now - lastTime >= 1000) {
        var gs = Math.round((lblGen - lastGenNum) / ((now - lastTime) / 1000));
        document.getElementById('fpsLabel').textContent = 'Gen/s: ' + gs;
        lastGenNum = lblGen;
        lastGenCount = 0; lastTime = now;
    }

    // max of 500 f/s with 2ms frame times, provided we can run that fast
    var targetInterval = Math.max(2, 510 - 20 * (+speedEl.value));
    var remaining = targetInterval - elapsed;
    setTimeout(animLoop, remaining > 0 ? remaining : 0);
}

canvas.addEventListener('contextmenu', function(e) { e.preventDefault(); });

// Helper: map click coords to grid cell
// If clamp=true, clicks outside viewport map to nearest edge cell (for right-click center)
function clickToGrid(e, clamp) {
    var rect = canvas.getBoundingClientRect();
    // Work in CSS pixels — cellSize and frame size are all CSS pixels
    var mx = e.clientX - rect.left;
    var my = e.clientY - rect.top;

    var dpr = window.devicePixelRatio || 1;
    var dSizei = Math.ceil(cellSize * currentScale * dpr);
    var cellCSS = dSizei / dpr; // actual rendered cell size in CSS pixels (includes aggregation scale)
    var cwActual = currentVw * dSizei;
    var chActual = currentVh * dSizei;
    var ox = Math.floor((canvasW * dpr - cwActual) / 2) / dpr;
    var oy = Math.floor((canvasH * dpr - chActual) / 2) / dpr;
    mx -= ox; my -= oy;

    if (mx < 0 || my < 0 || mx >= currentVw * cellCSS || my >= currentVh * cellCSS) {
        if (clamp) {
            mx = Math.max(0, Math.min(currentVw * cellCSS - cellCSS, mx));
            my = Math.max(0, Math.min(currentVh * cellCSS - cellCSS, my));
        } else {
            return null;
        }
    }

    return { cellX: Math.floor(mx / cellCSS) * currentScale + camX, cellY: Math.floor(my / cellCSS) * currentScale + camY };
}

canvas.addEventListener('mousedown', function(e) {
    var target = clickToGrid(e);
    if (!target) return;

    if (e.button === 0) {
        leftHeld = true;
        // Both buttons held: do nothing
        if (rightHeld) {
            leftClickPending = null;
            rcPending = null; // cancel recenter
            return;
        }
        // Shift+left: pan
        if (e.shiftKey) {
            panning   = true;
            wasPlayingBeforePan = running;
            if (running) stopAnim();
            tracksBeforePan = tracksEnabled; tracksEnabled = false;
            pStartX   = e.clientX; pStartY = e.clientY;
            camStartX = camX;      camStartY = camY;
            canvas.style.cursor = 'grabbing';
            return;
        }
        // Left alone: defer toggle to mouseup (in case right button follows)
        leftClickPending = target;
        leftDragActive = false;
        pStartX = e.clientX; pStartY = e.clientY;
    }

    if (e.button === 2) {
        e.preventDefault();
        // Detect right double-click: if timer from previous click is still active
        if (rightTimer !== null) {
            // Right double-click — show bookmark menu, cancel pending recenter
            cancelRightTimer();
            rightHeld = false;
            bookmarkMenuOpen = true;
            var menu = document.getElementById('bookmarkMenu');
            menu.classList.add('active');
            menu.style.left = e.clientX + 'px';
            menu.style.top = e.clientY + 'px';
            return;
        }

        rightHeld = true;
        // Both buttons held: do nothing
        if (leftHeld) {
            leftClickPending = null; // cancel deferred toggle
            return;
        }
        // Right alone: start pending recenter/pan
        wasPlayingBeforePan = running;
        if (running) stopAnim();
        tracksBeforePan = tracksEnabled; tracksEnabled = false;
        rightDownTarget = target;
        pStartX     = e.clientX; pStartY = e.clientY;
        camStartX   = camX;      camStartY = camY;
        canvas.style.cursor = 'grabbing';
        // Start 250ms timer — fires to recenter if pendingRecenter is set
        rightTimer = setTimeout(doRightTimer, 250);
    }
});

window.addEventListener('mouseup', function(e) {
    var wasPanning = panning;

    if (e.button === 0) {
        // Left released: if we didn't drag AND right not held, toggle the pending cell
        if (leftClickPending && !leftDragActive && !rightHeld) {
            toggleCell(leftClickPending.cellX, leftClickPending.cellY).then(zoomRefresh);
        }
        leftClickPending = null;
        leftDragActive = false;
        leftHeld = false;
        zoomDragStartY = null;
        zoomDragDelta = 0;
        zoomDragArmed = false;
        // Shift+left pan: release left stops panning
        if (wasPanning) {
            panning = false;
            canvas.style.cursor = 'crosshair';
            zoomRefresh();
            tracksEnabled = tracksBeforePan;
            if (tracksEnabled) document.getElementById('tracksBtn').textContent = 'Active';
            else document.getElementById('tracksBtn').textContent = 'Tracks';
            if (wasPlayingBeforePan) {
                wasPlayingBeforePan = false;
                startAnim();
            }
        }
    }

    if (e.button === 2) {
        rightHeld = false;
        // Right released: if timer is active, cancel it, set pendingRecenter, start new timer
        if (rightTimer !== null && rightDownTarget && !panning) {
            clearTimeout(rightTimer);
            pendingRecenter = true;
            rightTimer = setTimeout(doRightTimer, 250);
        } else {
            // Timer already expired or no target: clean up
            clearTimeout(rightTimer);
            rightTimer = null;
            rightDownTarget = null;
            pendingRecenter = false;
        }
        panning = false;
        canvas.style.cursor = 'crosshair';
        zoomDragStartY = null;
        zoomDragDelta = 0;
        zoomDragArmed = false;
        // Restore tracks state
        tracksEnabled = tracksBeforePan;
        if (tracksEnabled) document.getElementById('tracksBtn').textContent = 'Active';
        else document.getElementById('tracksBtn').textContent = 'Tracks';
        // Restart playback if it was running
        if (wasPlayingBeforePan) {
            wasPlayingBeforePan = false;
            startAnim();
        }
    }
});

window.addEventListener('mousemove', function(e) {
    // Both buttons held: zoom by vertical drag (with debounce threshold)
    if (leftHeld && rightHeld) {
        if (!zoomDragStartY) {
            zoomDragStartY = e.clientY;
            zoomDragDelta = 0;
            zoomDragArmed = false;
        }
        if (!zoomDragArmed) {
            var dy = Math.abs(e.clientY - zoomDragStartY);
            if (dy >= zoomDragThreshold) {
                zoomDragArmed = true;
                zoomDragStartY = e.clientY;
                zoomDragDelta = 0;
                leftClickPending = null; // cancel deferred toggle, we're zooming
            }
            return;
        }
        // Armed — accumulate delta for zoom steps
        zoomDragDelta += e.clientY - zoomDragStartY;
        zoomDragStartY = e.clientY;
        var steps = Math.floor(Math.abs(zoomDragDelta) / 60);
        if (steps >= 1) {
            var oldCs = cellSize;
            var dir = zoomDragDelta > 0 ? 1 : -1; // down = zoom in, up = zoom out
            for (var i = 0; i < steps; i++) {
                if (dir > 0 && zoomIdx < ZOOM_LEVELS.length - 1) zoomIdx++;
                else if (dir < 0) zoomIdx = Math.max(0, zoomIdx - 1);
            }
            cellSize = ZOOM_LEVELS[zoomIdx];
            zoomDragDelta %= 60;
            if (cellSize !== oldCs) doZoom(oldCs);
        }
        return;
    }

    // Left held alone: draw cells (only after moving from click point)
    if (leftHeld && !rightHeld && !panning && leftClickPending) {
        var dpr = window.devicePixelRatio || 1;
        var dSizei = Math.ceil(cellSize * currentScale * dpr);
        var cellCSS = dSizei / dpr;
        var moved = Math.sqrt((e.clientX - pStartX)**2 + (e.clientY - pStartY)**2);
        if (moved > cellCSS) {
            leftDragActive = true;
            var target = clickToGrid(e);
            if (target) {
                toggleCell(target.cellX, target.cellY);
            }
            zoomRefresh(); // show cells immediately
        }
    }

    // Right held alone: check for pan
    if (rightHeld && !leftHeld && rightDownTarget && !panning) {
        lastRightMoveX = e.clientX; lastRightMoveY = e.clientY;
        var moved = Math.sqrt((e.clientX - pStartX)**2 + (e.clientY - pStartY)**2);
        if (moved > 10) {
            // Mouse moved >10px: start panning, cancel recenter timer
            panning = true;
            camStartX = camX; camStartY = camY;
            cancelRightTimer();
            closeBookmarkMenu(); // close menu when panning starts
        }
    }

    if (panning) {
        var dpr = window.devicePixelRatio || 1;
        var dSizei = Math.ceil(cellSize * currentScale * dpr);
        var cellCSS = dSizei / dpr; // actual rendered cell size in CSS pixels (includes aggregation scale)
        var dx = Math.round((e.clientX - pStartX) / cellCSS) * currentScale;
        var dy = Math.round((e.clientY - pStartY) / cellCSS) * currentScale;
        camX = camStartX - dx;
        camY = camStartY - dy;
        zoomRefresh();
    }
});

// Prevent browser context menu on canvas
canvas.addEventListener('contextmenu', function(e) { e.preventDefault(); });

// ---- Zoom: mouse wheel & keyboard ----
// Deep zoom levels need multiple clicks to reach (avoid Brave renderD128 errors)
function doZoom(oldCs) {
    // Keep center of viewport anchored in grid coords
    var vpOld = calcCells(oldCs);
    var centerX = camX + Math.floor(vpOld.vw / 2);
    var centerY = camY + Math.floor(vpOld.vh / 2);

    var vpNew = calcCells(cellSize);
    camX = centerX - Math.floor(vpNew.vw / 2);
    camY = centerY - Math.floor(vpNew.vh / 2);
    // Clear prevBits so old tracks from previous zoom level don't persist
    prevBits = null;
    // Clear canvas immediately so old tracks don't flash
    ctx.fillStyle = '#1a1a2e';
    ctx.fillRect(0, 0, canvasW, canvasH);
    if (!running) zoomRefresh();
}

var lastWheelZoomTime = 0; // throttle: one zoom step per 100ms
document.addEventListener('wheel', function(e) {
    // Don't zoom when scrolling in inputs/textarea/modal
    if (e.target.tagName === 'TEXTAREA' || e.target.tagName === 'INPUT' || e.target.closest('#pasteModal')) return;
    e.preventDefault();
    // Throttle: only accept one zoom step per 100ms
    var now = performance.now();
    if (now - lastWheelZoomTime < 100) return;
    var oldCs = cellSize;
    if (e.deltaY < 0) {
        if (zoomIdx < ZOOM_LEVELS.length - 1) zoomIdx++;
    } else {
        zoomIdx = Math.max(0, zoomIdx - 1);
    }
    cellSize = ZOOM_LEVELS[zoomIdx];
    if (cellSize !== oldCs) { doZoom(oldCs); lastWheelZoomTime = now; }
}, {passive: false});

document.addEventListener('keydown', function(e) {
    if (e.target.tagName === 'INPUT') return;
    // ESC closes popups
    if (e.key === 'Escape') {
        if (bookmarkMenuOpen) { closeBookmarkMenu(); return; }
        if (document.getElementById('helpModal').classList.contains('active')) {
            document.getElementById('helpModal').classList.remove('active'); return;
        }
        if (document.getElementById('pasteModal').classList.contains('active')) {
            document.getElementById('pasteModal').classList.remove('active'); return;
        }
    }
    var oldCs = cellSize;
    if (e.key === '=' || e.key === '+') {
        if (zoomIdx < ZOOM_LEVELS.length - 1) zoomIdx++;
    } else if (e.key === '-') {
        zoomIdx = Math.max(0, zoomIdx - 1);
    } else return;
    cellSize = ZOOM_LEVELS[zoomIdx];
    if (cellSize !== oldCs) doZoom(oldCs);

});

// ---- Resize handler ----
window.addEventListener('resize', function() { refresh(); });

// ---- Controls ----
document.addEventListener('DOMContentLoaded', async function() {
    stepCountVal = step[0]; // init to first step size
    var playBtn = document.getElementById('playBtn');

    playBtn.addEventListener('click', function() {
        if (running) stopAnim();
        else startAnim();
    });

    document.getElementById('stepBtn').addEventListener('click', async function() {
        stopAnim();
        await call({action:'batch-step', count: stepCountVal});
        zoomRefresh();
    });

    stepPlusBtn = document.getElementById('stepPlusBtn');
    stepPlusBtn.addEventListener('click', function() {
        if (stepPlusBtn.textContent === 'Step+1') {
            stepCountVal += 1;
            stepPlusBtn.textContent = 'Step-1';
        } else {
            stepCountVal = Math.max(1, stepCountVal - 1);
            stepPlusBtn.textContent = 'Step+1';
        }
        document.getElementById('stepVal').textContent = stepCountVal;
    });

    document.getElementById('randBtn').addEventListener('click', async function() {
        stopAnim();
        var vp = calcCells(cellSize);
        var centerX = camX + Math.floor(vp.vw / 2);
        var centerY = camY + Math.floor(vp.vh / 2);
        await call({action:'randomize', cx: centerX, cy: centerY, size:100});
        // Set all bookmarks to current center
        var center = getViewportCenter();
        bookmarks.A = {x: center.x, y: center.y};
        bookmarks.B = {x: center.x, y: center.y};
        bookmarks.C = {x: center.x, y: center.y};
        zoomRefresh();
    });

    document.getElementById('helpBtn').addEventListener('click', function() {
        closeBookmarkMenu();
        document.getElementById('helpText').textContent = HELP_TEXT;
        document.getElementById('helpModal').classList.add('active');
    });
    document.getElementById('helpOkBtn').addEventListener('click', function() {
        document.getElementById('helpModal').classList.remove('active');
    });
    document.getElementById('quitBtn').addEventListener('click', doQuit);
    document.getElementById('tracksBtn').addEventListener('click', function() {
        tracksEnabled = !tracksEnabled;
        this.textContent = tracksEnabled ? 'Active' : 'Tracks';
        imgData = null; // force full redraw
        zoomRefresh();
    });
    document.getElementById('hashlifeBtn').addEventListener('click', async function() {
        hashlifeMode = !hashlifeMode;
        await call({action:'toggle-hashlife'});
        this.textContent = hashlifeMode ? 'Classic' : 'HashLife';
    });
    document.getElementById('clearBtn').addEventListener('click', async function() {
       stopAnim(); tracksEnabled = false; document.getElementById('tracksBtn').textContent = 'Tracks';
       await call({action:'clear'}); imgData = null; prevBits = null; prevOverlay = null; zoomRefresh();
    });

    // Load .lif or .mc file: parse and load cells centered on viewport
    document.getElementById('loadBtn').addEventListener('click', function() {
        document.getElementById('fileInput').click();
    });

    // Extract rule from RLE header. Returns null if no rule specified (OK — default B3/S23).
    // Returns the rule string if present.
    function extractRule(text) {
        var lines = text.split('\n');
        for (var i = 0; i < lines.length; i++) {
            var line = lines[i].trim();
            // RLE rule can be: "rule = B3/S23" or in a comment line
            var m = line.match(/rule\s*=\s*(\S+)/i);
            if (m) return m[1];
        }
        return null;
    }

    // Shared pattern loader — used by both file load and paste
    async function loadPattern(text, isMc) {
        // Check rule for RLE patterns (skip for .mc — no rule concept)
        if (!isMc) {
            var rule = extractRule(text);
            if (rule && rule.toUpperCase() !== 'B3/S23') {
                alert('Pattern uses rule ' + rule + '. This simulator only supports B3/S23.');
                return;
            }
        }
        stopAnim();
        await call({action:'clear'});
        var cells;
        if (isMc) {
            cells = parseMacrocell(text);
            var coordCenter = 2000000000;
            for (var i = 0; i < cells.length; i++) {
                cells[i][0] += coordCenter;
                cells[i][1] += coordCenter;
            }
        } else {
            cells = parseRLE(text);
        }
        var centerX = camX + Math.floor(calcCells(cellSize).vw / 2);
        var centerY = camY + Math.floor(calcCells(cellSize).vh / 2);
        var anchorX, anchorY;
        if (isMc) {
            var minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
            for (var i = 0; i < cells.length; i++) {
                if (cells[i][0] < minX) minX = cells[i][0];
                if (cells[i][0] > maxX) maxX = cells[i][0];
                if (cells[i][1] < minY) minY = cells[i][1];
                if (cells[i][1] > maxY) maxY = cells[i][1];
            }
            var patCX = Math.floor((minX + maxX) / 2);
            var patCY = Math.floor((minY + maxY) / 2);
            anchorX = centerX - patCX;
            anchorY = centerY - patCY;
        } else {
            anchorX = centerX;
            anchorY = centerY;
        }
        await fetch('/load-pattern', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({cells: cells, anchor_x: anchorX, anchor_y: anchorY})});
        // Set all bookmarks to current center
        var center = getViewportCenter();
        bookmarks.A = {x: center.x, y: center.y};
        bookmarks.B = {x: center.x, y: center.y};
        bookmarks.C = {x: center.x, y: center.y};
        zoomRefresh();
    }

    document.getElementById('fileInput').addEventListener('change', async function(e) {
        var file = e.target.files[0];
        if (!file) return;
        var text = await file.text();
        var isMc = file.name.endsWith('.mc');
        await loadPattern(text, isMc);
        e.target.value = ''; // allow re-selecting same file
    });

    // Paste modal
    document.getElementById('pasteBtn').addEventListener('click', function() {
        closeBookmarkMenu();
        document.getElementById('pasteModal').classList.add('active');
        document.getElementById('pasteArea').focus();
    });
    document.getElementById('pasteClearBtn').addEventListener('click', function() {
        document.getElementById('pasteArea').value = '';
        document.getElementById('pasteArea').focus();
    });
    document.getElementById('pasteSaveBtn').addEventListener('click', async function() {
        var text = document.getElementById('pasteArea').value.trim();
        if (!text) return;
        // Detect format for filename extension
        var ext = 'rle';
        if (text.includes('[M2]')) ext = 'mc';
        else if (text.includes('@')) ext = 'lif';
        var filename = 'pattern.' + ext;
        var blob = new Blob([text], {type: 'text/plain'});
        // Try File System Access API (Chromium browsers) — shows native save dialog
        if (window.showSaveFilePicker) {
            try {
                var handle = await window.showSaveFilePicker({
                    suggestedName: filename,
                    types: [{description: 'Pattern file', accept: {'text/plain': ['.rle', '.lif', '.mc']}}]
                });
                var writable = await handle.createWritable();
                await writable.write(blob);
                await writable.close();
                return;
            } catch (e) {
                if (e.name !== 'AbortError') console.warn('Save failed:', e);
            }
        }
        // Fallback: Blob download (Firefox, Safari)
        var a = document.createElement('a');
        a.href = URL.createObjectURL(blob);
        a.download = filename;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(a.href);
    });
    document.getElementById('pasteReadBtn').addEventListener('click', function() {
        document.getElementById('readFileInput').click();
    });
    document.getElementById('readFileInput').addEventListener('change', async function(e) {
        var file = e.target.files[0];
        if (!file) return;
        document.getElementById('pasteArea').value = await file.text();
        e.target.value = '';
        document.getElementById('pasteArea').focus();
    });
    document.getElementById('pasteCopyBtn').addEventListener('click', async function() {
        try {
            var resp = await fetch('/export-pattern');
            var text = await resp.text();
            document.getElementById('pasteArea').value = text;
            document.getElementById('pasteArea').focus();
        } catch (e) {
            alert('Export failed: ' + e.message);
        }
    });
    document.getElementById('pasteCancelBtn').addEventListener('click', function() {
        document.getElementById('pasteModal').classList.remove('active');
    });
    document.getElementById('pasteOkBtn').addEventListener('click', async function() {
        var text = document.getElementById('pasteArea').value.trim();
        if (!text) { document.getElementById('pasteModal').classList.remove('active'); return; }
        document.getElementById('pasteModal').classList.remove('active');
        var isMc = text.includes('cells=') || text.includes('Cells=');
        await loadPattern(text, isMc);
    });

 // Update step value display when slider changes
    document.getElementById('stepSlider').addEventListener('input', function() {
        stepCountVal = step[+this.value];
        document.getElementById('stepVal').textContent = stepCountVal;
        stepPlusBtn.textContent = 'Step+1';
    });

    // Bookmark menu button handlers
    document.getElementById('bmSaveA').addEventListener('click', function() { saveBookmark('A'); });
    document.getElementById('bmSaveB').addEventListener('click', function() { saveBookmark('B'); });
    document.getElementById('bmSaveC').addEventListener('click', function() { saveBookmark('C'); });
    document.getElementById('bmGotoA').addEventListener('click', function() { gotoBookmark('A'); });
    document.getElementById('bmGotoB').addEventListener('click', function() { gotoBookmark('B'); });
    document.getElementById('bmGotoC').addEventListener('click', function() { gotoBookmark('C'); });

    // Click outside bookmark menu to dismiss (use click instead of mousedown to avoid interfering with double-click)
    document.addEventListener('click', function(e) {
        if (bookmarkMenuOpen && !document.getElementById('bookmarkMenu').contains(e.target)) {
            closeBookmarkMenu();
        }
    });

    // Init
    camX = 2000000000; camY = 2000000000;
    var cx = Math.floor(400 / 2) - 50, cy = Math.floor(300 / 2) - 50;
    cx += 2000000000; cy += 2000000000;
    await call({action:'randomize', cx: cx, cy: cy});
    // Set all bookmarks to initial center
    var center = getViewportCenter();
    bookmarks.A = {x: center.x, y: center.y};
    bookmarks.B = {x: center.x, y: center.y};
    bookmarks.C = {x: center.x, y: center.y};
    zoomRefresh();
    setInterval(function() { if (!running) refresh(); }, 2000);
});
</script></body></html>"#;
