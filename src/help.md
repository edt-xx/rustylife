# Game of Life Simulator

A Conway's Game of Life implementation with HashLife optimization and a canvas-based frontend.
Currently just B3/S23.

## How to Run

Start the server: `cargo run --release`

Open a browser at `http://localhost:7654`

## Controls

### Playback
- **Play** - Start/stop animation
- **Step** - Advance one Step
- **Speed slider** - Control animation speed (1-50)
- **Step slider** - Set generations per step (1 to 131072)
- **Step+1** - Increase step count level by one

### Patterns
- **Clear** - Remove all cells
- **Load** - Load .lif/.rle/.txt/.mc files
- **Edit** - Paste RLE text from clipboard
    -- **Save - Save the pattern in the buffer to disk (may require you to allow in browser settings)
                    The Brave browser blocks the File System write access API on HTTP. 
                    Add exception via: Settings > Privacy and security > Site and Shields Settings >
                    Additional content settings > Insecure content > add http://localhost:7654
                    This menu path is browser specific
    -- **Read - Read a pattern file into the buffer
    -- **Copy - Copy the current pattern to the buffer as .rle or .mc depending on size.
    -- **Random - Create a Random pattern, copy to the buffer and paste.
    -- **Paste - Paste the current buffer to the game
    -- **Clear - Clear the current buffer
    -- **Cancel - exit without changing the pattern

### Display
- **Tracks** - Toggle birth/death overlay colors.  Reset when you recenter, pan or zoom.
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

Right-double-click to open the position bookmarks menu. 
Save A, B, or C to store the current viewport center. 
Goto A, B, or C to jump to a saved position. 
Bookmarks reset when you load a pattern, paste, or randomize.

## Acknowledgments

The HashLife implementation is based on GOLDE (Game Of Life Development Environment).
GOLDE: https://github.com/RyanJK5/GOLDE
