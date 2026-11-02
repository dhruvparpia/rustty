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
                    cwd: p.borrow().cwd().map(str::to_owned),
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
                crate::session::Shape::Leaf { cwd } => Node::Leaf(make_leaf(cwd.as_deref())),
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
