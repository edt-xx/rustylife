# HashLife Fix Plan — GOLDE Comparison

## Root Cause

Our `step()` calls `advance_slow()` directly without expanding the tree first.
GOLDE's `DoOneJump` expands until `NeedsExpansion(root, depth) || depth - 2 < m_StepAdvanceDepth`.

For a blinker (3 cells, depth=2): `advance_base_one_gen` calls `encode_level3` →
`encode_level2` on level-1 children → `encode_quadrant_*` on level-0 grandchildren
that don't exist. Reads FALSE_NODE (index 0) for everything → returns all-zero pattern.

For a glider (5 cells, depth=2): same issue, but some grandchildren happen to be
TRUE_NODE from the pattern, producing wrong but non-zero output.

## Comparison Summary

| Component | GOLDE | Ours | Status |
|-----------|-------|------|--------|
| TRUE_NODE | StaticTrueNode{nullptr×4} | Index 1, children all 0 | ✅ Equivalent |
| FALSE_NODE | nullptr | Index 0 | ✅ Equivalent |
| advance_4x4 bit layout | bitPosition(col,row) = (3-row)*4+(3-col) | 15-(r*4+c) | ✅ Equivalent |
| encode_quadrant_* | Check `== TrueNode` | Check `is_alive()` | ✅ Equivalent |
| window functions | WindowN/W/E/S/Center | window_n/w/e/s/center | ✅ Equivalent |
| assemble_quadrants | (rNW<<10)&MaskNW \| (rNE<<8)&MaskNE \| (rSW<<2)&MaskSW \| rSE&MaskSE | Same | ✅ Equivalent |
| assemble_centered_6x6 | (nw<<15)\|(n<<13)\|((ne<<11)&0x1000)\|... | Same | ✅ Equivalent |
| fetch_segments | bit=2,1,0 (3 levels) | (0..3).rev() = 2,1,0 | ✅ Equivalent |
| window positions | buildWindow(1,1),(3,1),(1,3),(3,3) | c_11,c_13,c_31,c_33 etc | ✅ Equivalent |
| **expansion before step** | **Yes — while loop in DoOneJump** | **No** | ❌ BUG |
| **from_flat min depth** | **ExpandUniverse(4)** | **No expansion** | ❌ BUG |
| advance_slow recursive | AdvanceNode (routes fast/slow) | advance_fast only | ⚠️ Minor |
| needs_expansion | Direct child checks | Generic check_rim | ⚠️ Minor |

## Fixes (in order)

### Fix 1: Expand before step()
Add expansion loop to `step()` matching GOLDE's DoOneJump pattern:
```rust
pub fn step(&mut self) {
    if self.is_empty() { return; }
    // Expand until tree is large enough for advance_slow base case
    while needs_expansion(&self.cache, self.root, self.depth) || self.depth < 3 {
        self.root = expand_node(&mut self.cache, self.root, self.depth);
        self.depth += 1;
    }
    // advance_slow advances exactly 1 generation
    self.root = advance_slow(&mut self.cache, self.root, self.depth);
    self.depth -= 1; // advance_slow returns level-2 result from level-3 base case
}
```

Wait — advance_slow doesn't reduce depth. GOLDE does `OverwriteData(advanced.Node, depth - 1)`.
advance_slow at level L returns a node at level L-1. advance_fast at level L returns level L-2.

For single-gen step: advance_slow(level) → result at level-1.
We need: expand to level >= 3, call advance_slow, result is level-1.

Actually looking more carefully at GOLDE:
- AdvanceSlow at level 3 → AdvanceBaseOneGen → returns level-2 node (DecodeLevel2)
- AdvanceSlow at level > 3 → recursive → returns level-1 node (FindOrCreate of 4 advanced windows)

So advance_slow always reduces level by 1. Our step() needs:
```rust
pub fn step(&mut self) {
    if self.is_empty() { return; }
    while needs_expansion(&self.cache, self.root, self.depth) || self.depth < 3 {
        self.root = expand_node(&mut self.cache, self.root, self.depth);
        self.depth += 1;
    }
    self.root = advance_slow(&mut self.cache, self.root, self.depth);
    self.depth -= 1;
}
```

### Fix 2: Remove level==0 special case in step()
GOLDE handles level < 3 by expanding, not special-casing. Remove the `if level == 0` branch.

### Fix 3: Expand in from_flat
After building the quadtree, expand to at least depth 3 so step() works immediately:
```rust
// After building tree:
while depth < 3 {
    tree = expand_node(&mut cache, tree, depth);
    depth += 1;
}
```

### Fix 4: advance_slow recursive call
Change `advance_fast(cache, window, level-1)` to `advance_slow(cache, window, level-1)`
for consistency with GOLDE's AdvanceNode routing for single-gen stepping.

Actually, GOLDE's AdvanceNode routes to AdvanceSlow when `level - 2 > m_StepAdvanceDepth`.
For single-gen stepping (m_StepAdvanceDepth=0), this means AdvanceSlow when level > 2.
AdvanceFast when level <= 2 (which returns node unchanged for level < 3).

For our single-gen step, advance_slow is always correct. advance_fast at level<3 returns
the node unchanged, which is wrong for the base case. So fix: use advance_slow recursively.

## Expected Results

After fixes:
- Blinker (3 cells): oscillates correctly between horizontal and vertical
- Glider (5 cells): moves diagonally, maintains 5 cells
- No crashes (index out of bounds)
