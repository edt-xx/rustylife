# Game of Life Simulator

A Conway's Game of Life implementation with HashLife optimization and a canvas-based frontend.

## How to Run

Start the server: `cargo run --release`

Open a browser at `http://localhost:7654`

## Controls

### Playback
- **Play** - Start/stop animation
- **Step** - Advance one generation (or multiple based on Step slider)
- **Speed slider** - Control animation speed (1-50)
- **Step slider** - Set generations per step (1 to 131072)
- **Step+1** - Increase step count level by one

### Patterns
- **Randomize** - Fill viewport with random cells
- **Clear** - Remove all cells
- **Load** - Load .lif/.rle/.txt/.mc files
- **Paste** - Paste RLE text from clipboard

### Display
- **Tracks** - Toggle birth/death overlay colors
- **HashLife/Classic** - Toggle between HashLife and classic algorithms
- **Quit** - Shut down the server

## Mouse Commands

- **Left click** - Toggle a cell (live/dead)
- **Left drag** - Draw multiple cells
- **Right click** - Release to recenter viewport on clicked cell
- **Right drag** - Pan the viewport
- **Right double-click** - Position bookmarks menu (Save/Goto A, B, C)
- **Both buttons** - Hold both and drag up/down to zoom
- **Mouse wheel** - Zoom in/out

## Keyboard

- **+ / =** - Zoom in
- **-** - Zoom out
- **Escape** - Close popup menus

## Position Bookmarks

Right-double-click to open the position bookmarks menu. Save A, B, or C to store the current viewport center. Goto A, B, or C to jump to a saved position. Bookmarks reset when you load a pattern, paste, or randomize.

## Acknowledgments

The HashLife implementation is based on GOLDE (Game Of Life Development Environment) by RyanJK5.
GOLDE: https://github.com/RyanJK5/GOLDE
