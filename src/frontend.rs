// Auto-extracted from game_of_life.py
pub const FRONTEND: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Game of Life</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
html,body{background:#1a1a2e;width:100%;height:100%;font-family:monospace;color:#e94560}
#viewport{position:absolute;top:0;left:0;right:0;bottom:0}
canvas#main{position:absolute;top:48px;left:16px;right:12px;border:2px solid #e94560;image-rendering:pixelated;background:#1a1a2e;cursor:crosshair;margin-bottom:64px;transform:translate3d(0,0,0);will-change:transform}
.toolbar{position:absolute;bottom:12px;left:50%;transform:translateX(-50%);display:flex;gap:8px;align-items:center;justify-content:center;padding:6px 14px;background:rgba(22,33,62,.9);border-radius:6px;z-index:10;min-width:95vw}
button,label{background:#16213e;color:#e94560;border:1px solid #e94560;padding:5px 12px;cursor:pointer;font-family:monospace;font-size:13px;border-radius:3px}
button:hover{background:#e94560;color:#1a1a2e}
input[type=range]{width:100px;vertical-align:middle}
.info{font-size:12px;color:#888;margin-left:4px}
#topInfo{position:absolute;top:8px;left:50%;transform:translateX(-50%);font-family:monospace;font-size:12px;color:#888;z-index:10;display:flex;flex-direction:column;align-items:center;gap:2px}
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
<div class="toolbar">
  <button id="playBtn">&#9654; Play</button>
  <button id="stepBtn">Step</button>
  <label>Speed <input type="range" id="speedSlider" min="1" max="50" value="25"></label>
  <label>Step <input type="range" id="stepSlider" min="0" max="11" value="0"> <span id="stepVal">1</span></label>
  <button id="stepPlusBtn">Step+1</button>

  <button id="randBtn">Randomize</button>
  <button id="clearBtn">Clear</button>
  <button id="loadBtn">Load .lif</button>
  <input type="file" id="fileInput" accept=".lif,.txt,.rle,.mc" style="display:none"/>
  <button id="tracksBtn">Tracks</button>
  <button id="hashlifeBtn">HashLife</button>
  &nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;<button id="quitBtn">Quit</button>
</div>
<button id="timingBtn" style="position:absolute;right:24px;bottom:12px;background:#16213e;color:#e94560;border:1px solid #e94560;padding:5px 12px;cursor:pointer;font-family:monospace;font-size:13px;border-radius:3px;z-index:11">Timing</button>
<script>
const canvas = document.getElementById('main'), ctx = canvas.getContext('2d');

const step = [1,2,4,16,32,64,256,512,1024,4096,16384,65536];

var ZOOM_LEVELS = [15,14,13,12,11,10,9,8,7,6,5,4,3,2,1,0.5,0.25,0.125,0.0625,0.04167]; // 1/24
var zoomIdx = 12; // default to cellSize=3 (ZOOM_LEVELS[12] == 3)
var cellSize = ZOOM_LEVELS[zoomIdx];
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

    // Strip comments (lines starting with #) and whitespace
    text = text.replace(/#.*$/gm, '').replace(/\s/g, '');

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

    // Parse header — skip comment lines
    var lineIdx = 1;
    while (lineIdx < lines.length) {
        var line = lines[lineIdx].trim();
        if (line === '' || line.startsWith('#')) { lineIdx++; continue; }
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
    // Convention: SE child upper-left is (0, 1), so NW upper-left is (-rootSize, -rootSize + 1)
    var originX = -rootSize;
    var originY = -rootSize + 1;
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
var tracksEnabled = false; // disabled by default, hide active overlay when on
var hashlifeMode = false; // tracks hashlife mode toggle
var stepCountVal = 1; // current step count (slider value + manual adjustments)

function drawGrid(data) {
    var hdr  = new DataView(data, 0, 40);
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

    // Set globals for updateLabels()
    lblGen = gen; lblPop = pop; lblActive = active;
    lblBirths = births; lblDeaths = deaths; lblHeap = heap; lblTiles = tiles;

    var bitsLen = ((vw * vh + 7) >> 3);
    var bits = new Uint8Array(data, 40, bitsLen);
    var overlayOff = 40 + bitsLen;
    var overlay = ol_len > 0 ? new Uint8Array(data, overlayOff, ol_len) : new Uint8Array(0);

    // Server-side aggregation: when cellSize < 1, the server sends an already-aggregated bitmap.
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
        // When cellSize < 1, server sends aggregated bitmap — vw/vh are display dimensions (1:1)
        // When cellSize >= 1, vw/vh are raw cell counts — multiply by cellSize for display
        var cw = cellSize < 1 ? vw : vw * cellSize;
        var ch = cellSize < 1 ? vh : vh * cellSize;
        var ox = Math.floor((canvasW - cw) / 2);
        var oy = Math.floor((canvasH - ch) / 2);
        ctx.drawImage(offCanvas, ox, oy, cw, ch);
        if (overlay.length > 0) { var copyOv2 = new Uint8Array(overlay.length); copyOv2.set(overlay); prevOverlay = copyOv2; }
    } else {
        // Delta: draw ONLY changed cells directly on main canvas
        // (avoids putImageData of megabytes per frame at 1px zoom)
        var total = vw * vh;
        var numBytes = (total + 7) >> 3;

        // At sub-pixel zoom, aggregated bitmap — each pixel is 1px on screen
        var isAggregated = cellSize < 1;
        var dSize = isAggregated ? 1 : cellSize;
        var cw2 = isAggregated ? vw : vw * cellSize;
        var ch2 = isAggregated ? vh : vh * cellSize;
        var ox2 = Math.floor((canvasW - cw2) / 2);
        var oy2 = Math.floor((canvasH - ch2) / 2);

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

        // Batch draw: dead cells first — show tracks or restore bg
        for (var i = 0; i < deadCells.length; i += 2) {
             var di = deadCells[i+1] * vw + deadCells[i];
            if (tracksEnabled && camX === prevCamX && camY === prevCamY) ctx.fillStyle = '#3a3a5a'; // track color only on static frames, not pan
            else ctx.fillStyle = (overlay.length > 0 && ((overlay[di >>> 3] >> (di & 7)) & 1)) ? '#22223a' : '#1a1a2e';
            ctx.fillRect(ox2 + deadCells[i]*dSize, oy2 + deadCells[i+1]*dSize, dSize, dSize);
        }

        ctx.fillStyle = '#e94560';
        for (var i = 0; i < aliveCells.length; i += 2)
            ctx.fillRect(ox2 + aliveCells[i] * dSize, oy2 + aliveCells[i+1] * dSize, dSize, dSize);

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
            ctx.fillStyle = '#22223a';
            for (var i = 0; i < bgLight.length; i += 2)
                ctx.fillRect(ox2 + bgLight[i]*dSize, oy2 + bgLight[i+1]*dSize, dSize, dSize);
            ctx.fillStyle = '#1a1a2e';
            for (var i = 0; i < bgDark.length; i += 2)
                ctx.fillRect(ox2 + bgDark[i]*dSize, oy2 + bgDark[i+1]*dSize, dSize, dSize);
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
        var zoomTxt = 'Zoom: ' + cellSize + 'px';
        if (zoomPending > 0) zoomTxt += ' (' + zoomPending + '/' + zoomThreshold() + ')';
        zoomTxt += ' | ' + vw + '\u00d7' + vh;
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
        var hdr = new DataView(data, 0, 40);
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
async function zoomRefresh() {
    ensureCanvasSize();
    var vp = calcCells(cellSize);
    var scale = cellSize < 1 ? Math.round(1 / cellSize) : 1;
    var url = '/state?vx=' + camX + '&vy=' + camY + '&vw=' + vp.vw + '&vh=' + vp.vh + '&scale=' + scale;
    var r = await fetch(url);
    var data = await r.arrayBuffer();
    drawGrid(data);
}

async function doQuit() { stopAnim(); await fetch('/action', {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify({action:'quit'})}); }

// ---- Pan: Shift+drag or right-click drag. Right-click: recenter on mouseup ----
var panning = false, pStartX, pStartY, camStartX, camStartY;
var rcPending = null; // {cellX, cellY} — right-click held, awaiting click vs drag
var wasPlayingBeforePan = false; // remember playback state during pan

// Global so play/stop functions can be called from anywhere
var running = false;
var lastGenCount = 0, lastTime = 0, lastGenNum = 0; // for G/s calculation

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
function clickToGrid(e) {
    var rect = canvas.getBoundingClientRect();
    var scaleX = canvas.width / (rect.right - rect.left);
    var scaleY = canvas.height / (rect.bottom - rect.top);
    var mx = ((e.clientX - rect.left)) * scaleX;
    var my = ((e.clientY - rect.top))  * scaleY;

    var vp = calcCells(cellSize);
    var cw = vp.vw * cellSize;
    var ch = vp.vh * cellSize;
    var ox = Math.floor((canvas.width  - cw) / 2);
    var oy = Math.floor((canvas.height - ch) / 2);
    mx -= ox; my -= oy;

    if (mx < 0 || my < 0 || mx >= cw || my >= ch) return null;

    return { cellX: Math.floor(mx / cellSize) + camX, cellY: Math.floor(my / cellSize) + camY };
}

canvas.addEventListener('mousedown', function(e) {
    var target = clickToGrid(e);
    if (!target) return;

    // Right-click: record for recenter vs pan decision on mouseup
    if (e.button === 2) {
        e.preventDefault();
        wasPlayingBeforePan = running;
        if (running) stopAnim();
        rcPending   = target;
        pStartX     = e.clientX; pStartY = e.clientY;
        camStartX   = camX;      camStartY = camY;
        canvas.style.cursor = 'grabbing';
        return;
    }

    // Shift+drag for pan (left-click only)
    if (e.shiftKey) {
        panning   = true;
        wasPlayingBeforePan = running;
        if (running) stopAnim();
        pStartX   = e.clientX; pStartY = e.clientY;
        camStartX = camX;      camStartY = camY;
        canvas.style.cursor = 'grabbing';
        return;
    }

    // Left-click: toggle cell
    if (e.button === 0) {
      toggleCell(target.cellX, target.cellY).then(zoomRefresh);
    }
});

window.addEventListener('mouseup', function(e) {
    panning = false; canvas.style.cursor = 'crosshair';

    // Right-click released: recenter if we didn't end up panning
    if (e.button === 2 && rcPending && !panning) {
        camX = rcPending.cellX - Math.floor(calcCells(cellSize).vw / 2);
       camY = rcPending.cellY - Math.floor(calcCells(cellSize).vh / 2);
        rcPending = null;
        zoomRefresh();
    }

    // Restart playback if it was running before pan started
    if (wasPlayingBeforePan) {
        wasPlayingBeforePan = false;
        startAnim();
    }
});

window.addEventListener('mousemove', function(e) {
    if (panning) {
        var dx = Math.round((e.clientX - pStartX) / cellSize);
        var dy = Math.round((e.clientY - pStartY) / cellSize);
        camX = camStartX - dx;
        camY = camStartY - dy;
   zoomRefresh(); // fire-and-forget, gen-wait loop handles stale data
    }
    // Right-click: check if we should switch from recenter to pan (moved > 10px)
    if (rcPending && !panning) {
        var moved = Math.sqrt((e.clientX - pStartX)**2 + (e.clientY - pStartY)**2);
        if (moved > 10) {
            panning = true;
            camStartX = camX; camStartY = camY;
            rcPending = null; // was a drag, cancel recenter
        }
    }
});

// ---- Zoom: mouse wheel & keyboard ----
// Deep zoom levels need multiple clicks to reach (avoid Brave renderD128 errors)
var zoomPending = 0; // accumulated ticks for deep zoom

function zoomThreshold() {
    // How many clicks needed to zoom deeper from current position
    if (zoomIdx < 17) return 1;  // normal: 1 click
    if (zoomIdx === 17) return 2; // 0.125 -> 0.0625: 2 clicks
    if (zoomIdx === 18) return 3; // 0.0625 -> 0.04167: 3 clicks
    return 1; // already at max depth
}

function tryZoomDeeper() {
    if (zoomIdx >= ZOOM_LEVELS.length - 1) return false; // max depth
    var threshold = zoomThreshold();
    zoomPending++;
    if (zoomPending >= threshold) {
        zoomPending = 0;
        zoomIdx++;
        return true;
    }
    return false;
}

function doZoom(oldCs) {
    // Keep center of viewport anchored in grid coords
    var vpOld = calcCells(oldCs);
    var centerX = camX + Math.floor(vpOld.vw / 2);
    var centerY = camY + Math.floor(vpOld.vh / 2);

    var vpNew = calcCells(cellSize);
    camX = centerX - Math.floor(vpNew.vw / 2);
    camY = centerY - Math.floor(vpNew.vh / 2);
    if (!running) zoomRefresh();
}

canvas.addEventListener('wheel', function(e) {
    e.preventDefault();
    var oldCs = cellSize;
    if (e.deltaY < 0) {
        // Zoom in (deeper) -- may need multiple clicks
        tryZoomDeeper();
    } else {
        // Zoom out -- always single click, clear pending
        zoomPending = 0;
        zoomIdx = Math.max(0, zoomIdx - 1);
    }
    cellSize = ZOOM_LEVELS[zoomIdx];
    if (cellSize !== oldCs) doZoom(oldCs);

}, {passive: false});

document.addEventListener('keydown', function(e) {
    if (e.target.tagName === 'INPUT') return;
    var oldCs = cellSize;
    if (e.key === '=' || e.key === '+') {
        tryZoomDeeper();
    } else if (e.key === '-') {
        zoomPending = 0;
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
        zoomRefresh();
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
        this.textContent = hashlifeMode ? 'Conventional' : 'HashLife';
    });
    document.getElementById('timingBtn').addEventListener('click', async function() {
        await fetch('/toggle-timing', {method:'POST', headers:{'Content-Type':'application/json'}});
        this.textContent = this.textContent === 'NoTiming' ? 'Timing' : 'NoTiming';
    });
    document.getElementById('clearBtn').addEventListener('click', async function() {
       stopAnim(); tracksEnabled = false; document.getElementById('tracksBtn').textContent = 'Tracks';
       await call({action:'clear'}); imgData = null; prevBits = null; prevOverlay = null; zoomRefresh();
    });

    // Load .lif or .mc file: parse and load cells centered on viewport
    document.getElementById('loadBtn').addEventListener('click', function() {
        document.getElementById('fileInput').click();
    });
    document.getElementById('fileInput').addEventListener('change', async function(e) {
        var file = e.target.files[0];
        if (!file) return;
        stopAnim();
        await call({action:'clear'});  // clear board before loading
        var text = await file.text();
        var isMc = file.name.endsWith('.mc');
        var cells;
        if (isMc) {
            cells = parseMacrocell(text);
            // .mc cells have absolute coords (can be negative). Add center of coord space
            // so server (cx + anchor) as u32 doesn't wrap negative values to huge numbers.
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
        // For .mc: cells already shifted to ~2B range; compute offset to center on viewport
        // For .lif: cells are relative (0,0 top-left); anchor = centerX centers them
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
        zoomRefresh();
        e.target.value = ''; // allow re-selecting same file
    });

 // Update step value display when slider changes
    document.getElementById('stepSlider').addEventListener('input', function() {
        stepCountVal = step[+this.value];
        document.getElementById('stepVal').textContent = stepCountVal;
        stepPlusBtn.textContent = 'Step+1';
    });

    // Init
    camX = 2000000000; camY = 2000000000;
    var cx = Math.floor(400 / 2) - 50, cy = Math.floor(300 / 2) - 50;
    cx += 2000000000; cy += 2000000000;
    await call({action:'randomize', cx: cx, cy: cy});
    zoomRefresh();
    setInterval(function() { if (!running) refresh(); }, 2000);
});
</script></body></html>"#;
