//! Pane tree for splits (SPEC SPLIT-001..015, spec §9/§10): a tab is a
//! binary split tree whose leaves are [`Pane`]s. A split node carries a
//! direction and one ratio: the fraction of the tab's width (vertical
//! split) or height (horizontal split) given to its first child.
//!
//! Leaves are `Rc<RefCell<Pane>>` so the tree can hand a leaf out to the
//! renderer, the input router, and the shell drain without index-path
//! acrobatics. The tree is touched from the window thread only.
//!
//! Layout order is the flatten order: left-to-right for vertical splits,
//! top-to-bottom for horizontal splits, computed depth-first. Tab and
//! renderer share that order for hit-testing and painting.

use std::cell::RefCell;
use std::rc::Rc;

use crate::pane::Pane;

/// Split orientation: how the tab's area is cut between the two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dir {
    /// Side-by-side children: the ratio splits the width.
    Vertical,
    /// Stacked children: the ratio splits the height.
    Horizontal,
}

/// Keyboard pane-focus movement direction (SPLIT-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Move focus to the pane on the left.
    Left,
    /// Move focus to the pane on the right.
    Right,
    /// Move focus to the pane above.
    Up,
    /// Move focus to the pane below.
    Down,
}

/// Pane box in tab-local cell coordinates (origin 0,0, `cols`/`rows` wide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// First column of the box.
    pub col: usize,
    /// First row of the box.
    pub row: usize,
    /// Width in cells.
    pub cols: usize,
    /// Height in cells.
    pub rows: usize,
}

impl Rect {
    /// Does the cell `(col, row)` fall inside this box?
    #[must_use]
    pub fn contains(&self, col: usize, row: usize) -> bool {
        col >= self.col
            && col < self.col + self.cols
            && row >= self.row
            && row < self.row + self.rows
    }
}

/// Where a dragged pane lands on the pane it is dropped on (SPLIT-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// The two panes trade places.
    Swap,
    /// The dragged pane goes beside the target on this side.
    Side(Side),
}

/// Which zone a pointer is in, from its position across the target pane (`0.0`
/// at the left or top, `1.0` at the right or bottom). The middle swaps; a band
/// around the edge docks to the nearest side.
#[must_use]
pub fn drop_zone(across: f32, down: f32) -> DropZone {
    const EDGE: f32 = 0.25;
    let left = across;
    let right = 1.0 - across;
    let up = down;
    let bottom = 1.0 - down;
    let nearest = left.min(right).min(up).min(bottom);
    if nearest >= EDGE {
        DropZone::Swap
    } else if nearest == left {
        DropZone::Side(Side::Left)
    } else if nearest == right {
        DropZone::Side(Side::Right)
    } else if nearest == up {
        DropZone::Side(Side::Up)
    } else {
        DropZone::Side(Side::Down)
    }
}

/// The part of a pane's `frame` its text uses (SPLIT-011): panes that share a tab
/// give their top row to a title. A pane too short to spare a row keeps it.
#[must_use]
pub const fn content_box(frame: Rect, panes: usize) -> Rect {
    if panes < 2 || frame.rows < 2 {
        return frame;
    }
    Rect {
        row: frame.row + 1,
        rows: frame.rows - 1,
        ..frame
    }
}

/// Smallest split ratio the tree accepts, so neither child collapses.
pub const MIN_RATIO: f32 = 0.1;
/// Largest split ratio the tree accepts.
pub const MAX_RATIO: f32 = 0.9;
/// Step for [`PaneTree::adjust_ratio`] (SPLIT-009 keyboard pane resize).
pub const RATIO_STEP: f32 = 0.05;
/// Ratio a new split starts at: the tab area is cut in half.
pub const DEFAULT_RATIO: f32 = 0.5;

/// serde default for a saved split ratio (session files).
pub fn default_ratio_serde() -> f32 {
    DEFAULT_RATIO
}

fn clamp_ratio(r: f32) -> f32 {
    r.clamp(MIN_RATIO, MAX_RATIO)
}

/// One node of the tree: a split with two children, or a pane leaf.
#[derive(Debug)]
pub enum Node {
    /// Vertical split: `ratio` of the width goes to `first`.
    VSplit {
        /// Share of the width given to `first` (clamped 0.1..0.9).
        ratio: f32,
        /// Left child.
        first: Box<Node>,
        /// Right child.
        second: Box<Node>,
    },
    /// Horizontal split: `ratio` of the height goes to `first`.
    HSplit {
        /// Share of the height given to `first` (clamped 0.1..0.9).
        ratio: f32,
        /// Top child.
        first: Box<Node>,
        /// Bottom child.
        second: Box<Node>,
    },
    /// Pane leaf: the shared grid the shells draw into.
    Leaf(Rc<RefCell<Pane>>),
}

/// A tab's pane tree: a root node plus helpers for the operations the UI
/// needs (split, close, focus, ratio adjust, layout).
#[derive(Debug)]
pub struct PaneTree {
    root: Node,
}

impl PaneTree {
    /// Single-leaf tree with the given pane.
    #[must_use]
    pub fn new(pane: Rc<RefCell<Pane>>) -> Self {
        Self {
            root: Node::Leaf(pane),
        }
    }

    /// Number of panes (leaves) in the tree.
    #[must_use]
    pub fn count(&self) -> usize {
        Self::count_node(&self.root)
    }

    /// Each leaf's dot path from the root, in flatten order: `.0` for a lone
    /// pane, then one component per split on the way down (`.1.0`).
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        fn go(n: &Node, path: &str, out: &mut Vec<String>) {
            match n {
                Node::Leaf(_) if path.is_empty() => out.push(".0".to_string()),
                Node::Leaf(_) => out.push(path.to_string()),
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, &format!("{path}.0"), out);
                    go(second, &format!("{path}.1"), out);
                }
            }
        }
        let mut out = Vec::with_capacity(self.count());
        go(&self.root, "", &mut out);
        out
    }

    /// All panes in flatten order (left-to-right, top-to-bottom).
    #[must_use]
    pub fn flatten(&self) -> Vec<Rc<RefCell<Pane>>> {
        fn go(n: &Node, out: &mut Vec<Rc<RefCell<Pane>>>) {
            match n {
                Node::Leaf(p) => out.push(Rc::clone(p)),
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, out);
                    go(second, out);
                }
            }
        }
        let mut out = Vec::with_capacity(self.count());
        go(&self.root, &mut out);
        out
    }

    /// Visit every leaf in flatten order, zipped with its box from `layout`
    /// (which must come from [`PaneTree::layout`], so indices line up).
    /// Allocation-free: the paint hot path uses this (NFR-113).
    pub fn for_each(&self, layout: &[Rect], mut f: impl FnMut(usize, &Rc<RefCell<Pane>>, Rect)) {
        // Trait object, not `impl FnMut`: a generic `f` would re-instantiate
        // `go` with a `&mut`-wrapped closure type at every recursion level.
        #[allow(
            clippy::type_complexity,
            reason = "the dyn visitor signature is the recursion fix, NFR-113"
        )]
        fn go(
            n: &Node,
            i: &mut usize,
            layout: &[Rect],
            f: &mut dyn FnMut(usize, &Rc<RefCell<Pane>>, Rect),
        ) {
            match n {
                Node::Leaf(p) => {
                    if let Some(&rect) = layout.get(*i) {
                        f(*i, p, rect);
                    }
                    *i += 1;
                }
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, i, layout, f);
                    go(second, i, layout, f);
                }
            }
        }
        go(&self.root, &mut 0usize, layout, &mut f);
    }

    /// Split the leaf `target` in two with `fresh` taking the far side
    /// (`right` for vertical, `bottom` for horizontal) at `ratio`.
    ///
    /// `false` when `target` is not a leaf of this tree.
    pub fn split(
        &mut self,
        target: &Rc<RefCell<Pane>>,
        dir: Dir,
        ratio: f32,
        fresh: Rc<RefCell<Pane>>,
    ) -> bool {
        fn go(
            n: &mut Node,
            target: &Rc<RefCell<Pane>>,
            dir: Dir,
            ratio: f32,
            fresh: Rc<RefCell<Pane>>,
        ) -> bool {
            match n {
                Node::Leaf(p) if Rc::ptr_eq(p, target) => {
                    *n = match dir {
                        Dir::Vertical => Node::VSplit {
                            ratio,
                            first: Box::new(Node::Leaf(Rc::clone(p))),
                            second: Box::new(Node::Leaf(fresh)),
                        },
                        Dir::Horizontal => Node::HSplit {
                            ratio,
                            first: Box::new(Node::Leaf(Rc::clone(p))),
                            second: Box::new(Node::Leaf(fresh)),
                        },
                    };
                    true
                }
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, target, dir, ratio, fresh.clone())
                        || go(second, target, dir, ratio, fresh)
                }
                Node::Leaf(_) => false,
            }
        }
        go(&mut self.root, target, dir, clamp_ratio(ratio), fresh)
    }

    /// Split the leaf `target` with `fresh` on `side` of it. `false` when `target`
    /// is not a leaf of this tree.
    pub fn split_side(
        &mut self,
        target: &Rc<RefCell<Pane>>,
        side: Side,
        ratio: f32,
        fresh: Rc<RefCell<Pane>>,
    ) -> bool {
        fn go(
            n: &mut Node,
            target: &Rc<RefCell<Pane>>,
            side: Side,
            ratio: f32,
            fresh: &Rc<RefCell<Pane>>,
        ) -> bool {
            match n {
                Node::Leaf(p) if Rc::ptr_eq(p, target) => {
                    let old = Rc::clone(p);
                    *n = beside(Node::Leaf(old), Rc::clone(fresh), side, ratio);
                    true
                }
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, target, side, ratio, fresh) || go(second, target, side, ratio, fresh)
                }
                Node::Leaf(_) => false,
            }
        }
        go(&mut self.root, target, side, clamp_ratio(ratio), &fresh)
    }

    /// Exchange two leaves, leaving every split as it is. `false` unless they are
    /// two different leaves of this tree.
    pub fn swap(&mut self, a: &Rc<RefCell<Pane>>, b: &Rc<RefCell<Pane>>) -> bool {
        fn go(n: &mut Node, a: &Rc<RefCell<Pane>>, b: &Rc<RefCell<Pane>>) {
            match n {
                Node::Leaf(p) if Rc::ptr_eq(p, a) => *p = Rc::clone(b),
                Node::Leaf(p) if Rc::ptr_eq(p, b) => *p = Rc::clone(a),
                Node::Leaf(_) => {}
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    go(first, a, b);
                    go(second, a, b);
                }
            }
        }
        if Rc::ptr_eq(a, b) || Self::find(&self.root, a).is_none() {
            return false;
        }
        if Self::find(&self.root, b).is_none() {
            return false;
        }
        go(&mut self.root, a, b);
        true
    }

    /// Take the leaf `moved` out and put it on `side` of the leaf `target`, each
    /// getting half of `target`'s old box. `false`, changing nothing, unless they
    /// are two different leaves of this tree.
    pub fn move_beside(
        &mut self,
        moved: &Rc<RefCell<Pane>>,
        target: &Rc<RefCell<Pane>>,
        side: Side,
    ) -> bool {
        if Rc::ptr_eq(moved, target)
            || Self::find(&self.root, moved).is_none()
            || Self::find(&self.root, target).is_none()
        {
            return false;
        }
        self.close(moved) && self.split_side(target, side, DEFAULT_RATIO, Rc::clone(moved))
    }

    /// Take the leaf `moved` out and give it the whole `side` of the tab. `false`,
    /// changing nothing, when it is not a leaf here or is the only one.
    pub fn dock(&mut self, moved: &Rc<RefCell<Pane>>, side: Side) -> bool {
        if self.count() < 2 || Self::find(&self.root, moved).is_none() || !self.close(moved) {
            return false;
        }
        let rest = std::mem::replace(&mut self.root, Node::Leaf(Rc::clone(moved)));
        self.root = beside(rest, Rc::clone(moved), side, DEFAULT_RATIO);
        true
    }

    /// Remove the leaf `target`; its sibling takes the parent's place.
    /// `true` when the leaf was found and removed. Closing the only leaf
    /// leaves the root empty (the tab is gone, the caller tears it down).
    /// Closing a pane keeps every other split's ratio (SPLIT-010).
    pub fn close(&mut self, target: &Rc<RefCell<Pane>>) -> bool {
        if self.count() == 1 && matches!(self.root, Node::Leaf(ref p) if Rc::ptr_eq(p, target)) {
            self.root = Node::Leaf(Rc::new(RefCell::new(Pane::new())));
            return true;
        }
        fn go(n: &mut Node, target: &Rc<RefCell<Pane>>) -> bool {
            let (first, second) = match n {
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    (first, second)
                }
                Node::Leaf(_) => return false,
            };
            match (first.as_mut(), second.as_mut()) {
                (Node::Leaf(p), _) if Rc::ptr_eq(p, target) => {
                    let promoted = std::mem::replace(
                        second,
                        Box::new(Node::Leaf(Rc::new(RefCell::new(Pane::new())))),
                    );
                    *n = *promoted;
                    true
                }
                (_, Node::Leaf(p)) if Rc::ptr_eq(p, target) => {
                    let promoted = std::mem::replace(
                        first,
                        Box::new(Node::Leaf(Rc::new(RefCell::new(Pane::new())))),
                    );
                    *n = *promoted;
                    true
                }
                _ => go(first, target) || go(second, target),
            }
        }
        go(&mut self.root, target)
    }

    /// Add `delta` to the ratio of the nearest split of orientation `dir`
    /// above the leaf `target` (SPLIT-009 keyboard pane resize: Left/Right
    /// widen the focused pane through vertical splits, Up/Down through
    /// horizontal ones). `true` when such a split was adjusted.
    pub fn adjust_ratio(&mut self, target: &Rc<RefCell<Pane>>, dir: Dir, delta: f32) -> bool {
        fn go(n: &mut Node, target: &Rc<RefCell<Pane>>, dir: Dir, delta: f32) -> bool {
            let (ratio, first, second) = match n {
                Node::VSplit {
                    ratio,
                    first,
                    second,
                } if dir == Dir::Vertical => (ratio, first, second),
                Node::HSplit {
                    ratio,
                    first,
                    second,
                } if dir == Dir::Horizontal => (ratio, first, second),
                Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                    return go(first, target, dir, delta) || go(second, target, dir, delta);
                }
                Node::Leaf(_) => return false,
            };
            let under_first = PaneTree::find(first, target).is_some();
            let under_second = PaneTree::find(second, target).is_some();
            if under_first {
                *ratio = clamp_ratio(*ratio + delta);
                true
            } else if under_second {
                *ratio = clamp_ratio(*ratio - delta);
                true
            } else {
                false
            }
        }
        go(&mut self.root, target, dir, delta)
    }

    /// Compute every pane's box for a `cols` x `rows` tab area, in
    /// flatten order. Sizes round from the ratio; each pane keeps at
    /// least one cell in each axis.
    #[must_use]
    pub fn layout(&self, cols: usize, rows: usize) -> Vec<Rect> {
        fn go(n: &Node, col: usize, row: usize, cols: usize, rows: usize, out: &mut Vec<Rect>) {
            match n {
                Node::Leaf(_) => out.push(Rect {
                    col,
                    row,
                    cols,
                    rows,
                }),
                Node::VSplit {
                    ratio,
                    first,
                    second,
                } => {
                    let c = ((cols as f32 * ratio).round() as usize)
                        .clamp(1, cols.saturating_sub(1).max(1));
                    go(first, col, row, c, rows, out);
                    go(second, col + c, row, cols - c, rows, out);
                }
                Node::HSplit {
                    ratio,
                    first,
                    second,
                } => {
                    let r = ((rows as f32 * ratio).round() as usize)
                        .clamp(1, rows.saturating_sub(1).max(1));
                    go(first, col, row, cols, r, out);
                    go(second, col, row + r, cols, rows - r, out);
                }
            }
        }
        let mut out = Vec::with_capacity(self.count());
        go(&self.root, 0, 0, cols, rows, &mut out);
        out
    }

    fn count_node(n: &Node) -> usize {
        match n {
            Node::Leaf(_) => 1,
            Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                Self::count_node(first) + Self::count_node(second)
            }
        }
    }

    /// Serializable shape of this tree with each leaf's OSC 7 cwd
    /// (session save, TAB-026).
    #[must_use]
    pub fn shape(&self) -> crate::session::Shape {
        fn go(n: &Node) -> crate::session::Shape {
            match n {
                Node::Leaf(p) => crate::session::Shape::Leaf {
                    cwd: p.borrow().saved_cwd().map(str::to_owned),
                    agent: p.borrow().saved_agent().map(str::to_owned),
                },
                Node::VSplit {
                    ratio,
                    first,
                    second,
                } => crate::session::Shape::Split {
                    dir: Dir::Vertical,
                    ratio: *ratio,
                    a: Box::new(go(first)),
                    b: Box::new(go(second)),
                },
                Node::HSplit {
                    ratio,
                    first,
                    second,
                } => crate::session::Shape::Split {
                    dir: Dir::Horizontal,
                    ratio: *ratio,
                    a: Box::new(go(first)),
                    b: Box::new(go(second)),
                },
            }
        }
        go(&self.root)
    }

    /// Rebuild a tree from a saved shape (TAB-026). `make_leaf` creates
    /// each leaf pane; nesting deeper than [`crate::session::MAX_DEPTH`]
    /// collapses to a leaf (the file is hand-editable).
    #[must_use]
    pub fn from_shape(
        shape: &crate::session::Shape,
        make_leaf: &mut dyn FnMut(Option<&str>) -> Rc<RefCell<Pane>>,
    ) -> Self {
        fn go(
            s: &crate::session::Shape,
            depth: u32,
            make_leaf: &mut dyn FnMut(Option<&str>) -> Rc<RefCell<Pane>>,
        ) -> Node {
            match s {
                crate::session::Shape::Leaf { cwd, .. } => Node::Leaf(make_leaf(cwd.as_deref())),
                crate::session::Shape::Split { dir, ratio, a, b } if depth > 0 => {
                    let first = Box::new(go(a, depth - 1, make_leaf));
                    let second = Box::new(go(b, depth - 1, make_leaf));
                    let ratio = clamp_ratio(*ratio);
                    match dir {
                        Dir::Vertical => Node::VSplit {
                            ratio,
                            first,
                            second,
                        },
                        Dir::Horizontal => Node::HSplit {
                            ratio,
                            first,
                            second,
                        },
                    }
                }
                // Over-deep split: keep one leaf from its first branch.
                crate::session::Shape::Split { .. } => Node::Leaf(make_leaf(s.leaf_cwd())),
            }
        }
        Self {
            root: go(shape, crate::session::MAX_DEPTH, make_leaf),
        }
    }

    fn find(n: &Node, target: &Rc<RefCell<Pane>>) -> Option<()> {
        match n {
            Node::Leaf(p) if Rc::ptr_eq(p, target) => Some(()),
            Node::VSplit { first, second, .. } | Node::HSplit { first, second, .. } => {
                Self::find(first, target).or_else(|| Self::find(second, target))
            }
            Node::Leaf(_) => None,
        }
    }
}

/// A split holding `node` and the leaf `fresh`, with `fresh` on `side`.
fn beside(node: Node, fresh: Rc<RefCell<Pane>>, side: Side, ratio: f32) -> Node {
    let node = Box::new(node);
    let fresh = Box::new(Node::Leaf(fresh));
    match side {
        Side::Left => Node::VSplit {
            ratio,
            first: fresh,
            second: node,
        },
        Side::Right => Node::VSplit {
            ratio,
            first: node,
            second: fresh,
        },
        Side::Up => Node::HSplit {
            ratio,
            first: fresh,
            second: node,
        },
        Side::Down => Node::HSplit {
            ratio,
            first: node,
            second: fresh,
        },
    }
}

/// Pane index adjacent to `idx` on `side` (SPLIT-008 keyboard pane focus),
/// over a [`PaneTree::layout`] box list: the touching pane with the
/// greatest overlap in that direction. `None` when no pane touches.
#[must_use]
pub fn neighbor(layout: &[Rect], idx: usize, side: Side) -> Option<usize> {
    let cur = *layout.get(idx)?;
    let mut best: Option<(usize, usize)> = None; // (overlap, pane index)
    for (i, &r) in layout.iter().enumerate() {
        if i == idx {
            continue;
        }
        let overlap = match side {
            Side::Left if r.col + r.cols == cur.col => {
                cur.row.max(r.row)..(cur.row + cur.rows).min(r.row + r.rows)
            }
            Side::Right if cur.col + cur.cols == r.col => {
                cur.row.max(r.row)..(cur.row + cur.rows).min(r.row + r.rows)
            }
            Side::Up if r.row + r.rows == cur.row => {
                cur.col.max(r.col)..(cur.col + cur.cols).min(r.col + r.cols)
            }
            Side::Down if cur.row + cur.rows == r.row => {
                cur.col.max(r.col)..(cur.col + cur.cols).min(r.col + r.cols)
            }
            _ => 0usize..0usize,
        };
        let span = overlap.end.saturating_sub(overlap.start);
        if span > 0 && best.is_none_or(|(b, _)| span > b) {
            best = Some((span, i));
        }
    }
    best.map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Rc<RefCell<Pane>> {
        Rc::new(RefCell::new(Pane::new()))
    }

    #[test]
    fn a_title_row_goes_above_the_text_only_when_panes_share_a_tab() {
        let frame = Rect {
            col: 3,
            row: 2,
            cols: 40,
            rows: 10,
        };
        assert_eq!(content_box(frame, 1), frame, "one pane needs no title");
        assert_eq!(
            content_box(frame, 2),
            Rect {
                row: 3,
                rows: 9,
                ..frame
            }
        );
        let short = Rect { rows: 1, ..frame };
        assert_eq!(content_box(short, 2), short, "text wins over the title");
    }

    #[test]
    fn splitting_toward_a_side_puts_the_new_pane_there() {
        for (side, first_is_new, across) in [
            (Side::Left, true, true),
            (Side::Right, false, true),
            (Side::Up, true, false),
            (Side::Down, false, false),
        ] {
            let (a, b) = (pane(), pane());
            let mut tree = PaneTree::new(Rc::clone(&a));
            assert!(tree.split_side(&a, side, 0.5, Rc::clone(&b)));
            let order = tree.flatten();
            let want = if first_is_new { [&b, &a] } else { [&a, &b] };
            assert!(Rc::ptr_eq(&order[0], want[0]), "{side:?}");
            assert!(Rc::ptr_eq(&order[1], want[1]), "{side:?}");
            let boxes = tree.layout(80, 24);
            assert_eq!(boxes[1].col > 0, across, "{side:?}");
            assert_eq!(boxes[1].row > 0, !across, "{side:?}");
        }
    }

    #[test]
    fn swapping_exchanges_two_panes_and_keeps_the_shape() {
        let (a, b, c) = (pane(), pane(), pane());
        let mut tree = PaneTree::new(Rc::clone(&a));
        tree.split(&a, Dir::Vertical, 0.3, Rc::clone(&b));
        tree.split(&b, Dir::Horizontal, 0.5, Rc::clone(&c));
        let shape = tree.layout(100, 40);
        assert!(tree.swap(&a, &c));
        assert_eq!(tree.layout(100, 40), shape);
        let order = tree.flatten();
        assert!(Rc::ptr_eq(&order[0], &c));
        assert!(Rc::ptr_eq(&order[1], &b));
        assert!(Rc::ptr_eq(&order[2], &a));
    }

    #[test]
    fn swapping_needs_two_different_panes_from_this_tree() {
        let (a, b, stranger) = (pane(), pane(), pane());
        let mut tree = PaneTree::new(Rc::clone(&a));
        tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b));
        assert!(!tree.swap(&a, &a));
        assert!(!tree.swap(&a, &stranger));
        assert!(Rc::ptr_eq(&tree.flatten()[0], &a), "nothing moved");
    }

    #[test]
    fn moving_a_pane_beside_another_rebuilds_around_it() {
        // [a | b] with c under b: move c to a's left.
        let (a, b, c) = (pane(), pane(), pane());
        let mut tree = PaneTree::new(Rc::clone(&a));
        tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b));
        tree.split(&b, Dir::Horizontal, 0.5, Rc::clone(&c));
        assert!(tree.move_beside(&c, &a, Side::Left));
        assert_eq!(tree.count(), 3);
        let order = tree.flatten();
        assert!(Rc::ptr_eq(&order[0], &c));
        assert!(Rc::ptr_eq(&order[1], &a));
        assert!(Rc::ptr_eq(&order[2], &b));
        let boxes = tree.layout(100, 40);
        assert_eq!(boxes[2].rows, 40, "b took the whole height back");
    }

    #[test]
    fn moving_a_pane_onto_itself_or_a_stranger_changes_nothing() {
        let (a, b, stranger) = (pane(), pane(), pane());
        let mut tree = PaneTree::new(Rc::clone(&a));
        tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b));
        assert!(!tree.move_beside(&a, &a, Side::Up));
        assert!(!tree.move_beside(&a, &stranger, Side::Up));
        assert!(!tree.move_beside(&stranger, &a, Side::Up));
        assert_eq!(tree.count(), 2, "no pane was lost");
    }

    #[test]
    fn docking_gives_a_pane_a_whole_edge_of_the_tab() {
        // [a | b] with a docked up: a across the top, b below.
        let (a, b) = (pane(), pane());
        let mut tree = PaneTree::new(Rc::clone(&a));
        tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b));
        assert!(tree.dock(&a, Side::Up));
        assert_eq!(
            tree.layout(80, 20),
            vec![
                Rect {
                    col: 0,
                    row: 0,
                    cols: 80,
                    rows: 10
                },
                Rect {
                    col: 0,
                    row: 10,
                    cols: 80,
                    rows: 10
                },
            ]
        );
        assert!(Rc::ptr_eq(&tree.flatten()[0], &a));
    }

    #[test]
    fn a_lone_pane_cannot_be_docked() {
        let a = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(!tree.dock(&a, Side::Up));
        assert!(
            Rc::ptr_eq(&tree.flatten()[0], &a),
            "the pane is still there"
        );
    }

    #[test]
    fn the_middle_of_a_pane_swaps_and_its_edges_dock() {
        assert_eq!(drop_zone(0.5, 0.5), DropZone::Swap);
        assert_eq!(drop_zone(0.3, 0.7), DropZone::Swap);
        assert_eq!(drop_zone(0.1, 0.5), DropZone::Side(Side::Left));
        assert_eq!(drop_zone(0.9, 0.5), DropZone::Side(Side::Right));
        assert_eq!(drop_zone(0.5, 0.1), DropZone::Side(Side::Up));
        assert_eq!(drop_zone(0.5, 0.9), DropZone::Side(Side::Down));
        // A corner goes to the nearer edge.
        assert_eq!(drop_zone(0.05, 0.2), DropZone::Side(Side::Left));
        assert_eq!(drop_zone(0.2, 0.05), DropZone::Side(Side::Up));
    }

    #[test]
    fn single_leaf_basics() {
        let p = pane();
        let tree = PaneTree::new(Rc::clone(&p));
        assert_eq!(tree.count(), 1);
        assert_eq!(tree.flatten().len(), 1);
        assert_eq!(
            tree.layout(80, 24),
            vec![Rect {
                col: 0,
                row: 0,
                cols: 80,
                rows: 24
            }]
        );
    }

    #[test]
    fn vertical_split_halves_the_width() {
        let a = pane();
        let b = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b)));
        assert_eq!(tree.count(), 2);
        assert!(Rc::ptr_eq(&tree.flatten()[0], &a));
        assert!(Rc::ptr_eq(&tree.flatten()[1], &b));
        assert_eq!(
            tree.layout(81, 24),
            vec![
                Rect {
                    col: 0,
                    row: 0,
                    cols: 41,
                    rows: 24
                },
                Rect {
                    col: 41,
                    row: 0,
                    cols: 40,
                    rows: 24
                },
            ]
        );
    }

    #[test]
    fn horizontal_split_halves_the_height() {
        let a = pane();
        let b = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Horizontal, 0.5, Rc::clone(&b)));
        assert_eq!(
            tree.layout(80, 25),
            vec![
                Rect {
                    col: 0,
                    row: 0,
                    cols: 80,
                    rows: 13
                },
                Rect {
                    col: 0,
                    row: 13,
                    cols: 80,
                    rows: 12
                },
            ]
        );
    }

    #[test]
    fn split_targets_a_leaf_inside_the_tree() {
        let a = pane();
        let b = pane();
        let c = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b)));
        // Split the right-hand leaf: three panes, b keeps its box's top half.
        assert!(tree.split(&b, Dir::Horizontal, 0.5, Rc::clone(&c)));
        assert_eq!(tree.count(), 3);
        let layout = tree.layout(80, 20);
        assert_eq!(layout[0].cols, 40);
        assert_eq!(
            layout[1],
            Rect {
                col: 40,
                row: 0,
                cols: 40,
                rows: 10
            }
        );
        assert_eq!(
            layout[2],
            Rect {
                col: 40,
                row: 10,
                cols: 40,
                rows: 10
            }
        );
    }

    #[test]
    fn for_each_visits_leaves_in_layout_order() {
        let a = pane();
        let b = pane();
        let c = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b)));
        assert!(tree.split(&b, Dir::Horizontal, 0.5, Rc::clone(&c)));
        let layout = tree.layout(80, 20);
        let seen = std::cell::RefCell::new(Vec::new());
        tree.for_each(&layout, |i, p, rect| {
            seen.borrow_mut().push((i, rect.row));
            assert!(Rc::ptr_eq(p, &tree.flatten()[i]));
        });
        assert_eq!(seen.into_inner(), vec![(0, 0), (1, 0), (2, 10)]);
    }

    #[test]
    fn close_promotes_the_sibling_and_keeps_ratios() {
        let a = pane();
        let b = pane();
        let c = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.25, Rc::clone(&b)));
        assert!(tree.split(&b, Dir::Vertical, 0.5, Rc::clone(&c)));
        // Close b: c takes b's slot at b's parent, a keeps ratio 0.25.
        assert!(tree.close(&b));
        assert_eq!(tree.count(), 2);
        let layout = tree.layout(80, 20);
        assert_eq!(layout[0].cols, 20, "a keeps its 0.25 ratio");
        assert!(Rc::ptr_eq(&tree.flatten()[1], &c));
        assert!(!tree.close(&b), "b is gone: closing it again fails");
    }

    #[test]
    fn close_last_pane_empties_the_tree() {
        let a = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.close(&a));
        assert_eq!(tree.count(), 1, "placeholder keeps the tree shape");
        let placeholder = &tree.flatten()[0];
        assert!(!Rc::ptr_eq(placeholder, &a));
    }

    #[test]
    fn adjust_ratio_moves_the_nearest_split_of_its_orientation() {
        let a = pane();
        let b = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b)));
        // Growing the focused left pane widens first child.
        assert!(tree.adjust_ratio(&a, Dir::Vertical, RATIO_STEP));
        assert_eq!(tree.layout(80, 20)[0].cols, 44);
        // Growing the right pane shrinks first child.
        assert!(tree.adjust_ratio(&b, Dir::Vertical, RATIO_STEP));
        assert_eq!(tree.layout(80, 20)[0].cols, 40);
        // Clamped at MAX_RATIO.
        for _ in 0..30 {
            tree.adjust_ratio(&a, Dir::Vertical, RATIO_STEP);
        }
        assert_eq!(tree.layout(80, 20)[0].cols, 72, "ratio clamps at 0.9");
        // A horizontal ratio key does nothing on a vertical-only tree.
        assert!(!tree.adjust_ratio(&a, Dir::Horizontal, RATIO_STEP));
        assert_eq!(tree.layout(80, 20)[0].rows, 20, "height unchanged");
    }

    #[test]
    fn layout_never_starves_a_pane() {
        let a = pane();
        let b = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, MIN_RATIO, Rc::clone(&b)));
        let layout = tree.layout(2, 1);
        assert!(layout.iter().all(|r| r.cols >= 1 && r.rows >= 1));
    }

    #[test]
    fn neighbor_finds_the_touching_pane_with_the_best_overlap() {
        let a = pane();
        let b = pane();
        let c = pane();
        let mut tree = PaneTree::new(Rc::clone(&a));
        assert!(tree.split(&a, Dir::Vertical, 0.5, Rc::clone(&b)));
        assert!(tree.split(&b, Dir::Horizontal, 0.5, Rc::clone(&c)));
        let layout = tree.layout(80, 20);
        // a's right neighbour overlaps both b and c: the lowest index wins ties.
        assert_eq!(neighbor(&layout, 0, Side::Right), Some(1));
        assert_eq!(neighbor(&layout, 0, Side::Left), None);
        assert_eq!(neighbor(&layout, 1, Side::Down), Some(2));
        assert_eq!(neighbor(&layout, 2, Side::Up), Some(1));
        assert_eq!(neighbor(&layout, 1, Side::Left), Some(0));
        assert_eq!(neighbor(&layout, 2, Side::Left), Some(0));
        assert_eq!(neighbor(&layout, 0, Side::Up), None);
    }
}
