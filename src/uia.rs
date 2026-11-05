//! UI Automation provider (spec UIA-001..005, ACCESS-002..005, WIN-007).
//!
//! The tree: window root → tab bar → one [`Node::Tab`] per tab; window root
//! → one [`Node::Grid`] per pane of the active tab; window root →
//! [`Node::Search`] while the search overlay is open.
//!
//! FFI boundary (NFR-401): the [`Provider`] COM shell answers UIA calls by
//! asking `window.rs` for a live [`Snapshot`] of the window state. The pure
//! layer (ids, navigation, hit, rects, properties) is unit-tested; the COM
//! glue needs Inspector and a screen reader (A-12).
//!
//! shortcut: no `ITextProvider`/`ITextRangeProvider` — grid text is served
//! through the `Value`/`LegacyIAccessibleValue` properties (UIA-002). Add a
//! text pattern when a screen-reader walk of a scrolled pane proves too
//! slow.
//! shortcut: no property-change events; a client re-reads on demand. Raise
//! `UiaRaiseAutomationEvent` when Inspector shows stale state.
//! shortcut: `ProviderOptions(0)` (calls arrive on the UI thread, which
//! owns `WindowState`); switch to `ProviderOptions_UseComThreading` if a
//! cross-thread read is ever observed.
// The `#[implement]` macro generates a public `Provider_Impl` struct with no
// doc of its own; this file's own items are all documented.
#![allow(
    missing_docs,
    reason = "#[implement] generates an undocumented Provider_Impl"
)]

use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, HWND};
use windows::Win32::System::Com::{SAFEARRAY, SAFEARRAYBOUND};
use windows::Win32::System::Ole::{SafeArrayAccessData, SafeArrayCreate, SafeArrayUnaccessData};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Accessibility::{
    IRawElementProviderFragment, IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
    IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
    IRawElementProviderSimple_Impl, NavigateDirection, NavigateDirection_FirstChild,
    NavigateDirection_LastChild, NavigateDirection_NextSibling, NavigateDirection_Parent,
    NavigateDirection_PreviousSibling, ProviderOptions, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_TabControlTypeId,
    UIA_TabItemControlTypeId, UIA_WindowControlTypeId, UiaHostProviderFromHwnd, UiaRect,
};
use windows::core::{ComObject, IUnknown, Result, implement};

/// A node of the exposed UIA tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Node {
    /// The top-level window.
    Root,
    /// The tab-bar strip.
    TabBar,
    /// Tab `i` (0-based).
    Tab(usize),
    /// Pane `i` of the active tab (flatten order).
    Grid(usize),
    /// The search overlay strip.
    Search,
    /// The command palette overlay (issue #39).
    Palette,
    /// The settings overlay (#91).
    Settings,
}

/// Runtime id for `node` (unique per node; UIA appends `UiaAppendRuntimeId`).
#[must_use]
pub fn runtime_id(node: Node) -> Vec<i32> {
    match node {
        Node::Root => vec![1, 0],
        Node::TabBar => vec![1, 1],
        Node::Tab(i) => vec![1, 2, i as i32],
        Node::Grid(i) => vec![1, 3, i as i32],
        Node::Search => vec![1, 4],
        Node::Palette => vec![1, 5],
        Node::Settings => vec![1, 6],
    }
}

/// An axis-aligned client-DIP rectangle: `(left, top, right, bottom)`.
pub type DipRect = (f32, f32, f32, f32);

/// Tab summary for the snapshot.
#[derive(Clone, Debug)]
pub struct TabInfo {
    /// Tab title.
    pub title: String,
    /// Profile name (UIA-001).
    pub profile: String,
    /// Working directory of the focused pane, when the shell reported one.
    pub cwd: Option<String>,
    /// OSC 7 host that is not this machine (IDENT-001): remote sessions.
    pub host: Option<String>,
    /// `true` while a bell is pending (shell activity).
    pub activity: bool,
}

/// Pane summary for the snapshot (active tab only).
#[derive(Clone, Debug)]
pub struct PaneInfo {
    /// Viewport text, one line per row, `\n`-joined (UIA-002).
    pub screen_text: String,
    /// Scrollback text above the viewport, oldest first, capped by the
    /// builder (UIA-002: best-effort).
    pub scrollback_text: String,
    /// Cursor cell (row, col), when the cursor is on screen.
    pub cursor: Option<(usize, usize)>,
    /// Scrollback lines available above the viewport.
    pub scrollback_lines: usize,
    /// Selected text, when a selection exists (UIA-002).
    pub selection: Option<String>,
}

/// Everything the UIA tree needs, captured from `WindowState` on the UI
/// thread. All rects are client DIPs; `client_w`/`client_h`/`dpi` map them
/// to physical screen pixels together with the window origin.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Window caption.
    pub title: String,
    /// App version string (served as root HelpText).
    pub version: String,
    /// Tabs, in order.
    pub tabs: Vec<TabInfo>,
    /// Active tab index.
    pub active: usize,
    /// `true` when the tab-bar strip is visible.
    pub bar_visible: bool,
    /// Tab-bar strip bounds (client DIPs).
    pub bar_bounds: DipRect,
    /// Per-tab x-extents `(left, right)` from the renderer layout, inside
    /// the tab strip.
    pub tab_bounds: Vec<(f32, f32)>,
    /// Client width (DIPs).
    pub client_w: f32,
    /// Client height (DIPs).
    pub client_h: f32,
    /// DPI value (`GetDpiForWindow`, 96 at 100%).
    pub dpi: f32,
    /// Bounds of the active tab's panes, flatten order (client DIPs).
    pub grid_bounds: Vec<DipRect>,
    /// Panes of the active tab, flatten order.
    pub panes: Vec<PaneInfo>,
    /// Flatten index of the focused pane.
    pub focused_pane: usize,
    /// Search overlay bounds, `None` while the overlay is closed.
    pub search_bounds: Option<DipRect>,
    /// Search query, `None` while the overlay is closed.
    pub search_query: Option<String>,
    /// `(current match, match count)` while the overlay is open.
    pub search_matches: Option<(usize, usize)>,
    /// Command palette overlay bounds, `None` while closed (issue #39).
    pub palette_bounds: Option<DipRect>,
    /// Command palette query, `None` while closed.
    pub palette_query: Option<String>,
    /// `(selected row, hit count)` while the palette is open.
    pub palette_matches: Option<(usize, usize)>,
    /// Settings overlay bounds, `None` while closed (#91).
    pub settings_bounds: Option<DipRect>,
    /// `(selected setting, setting count)` while settings overlay is open.
    pub settings_selected: Option<(usize, usize)>,
    /// Typed TOML value in the settings overlay.
    pub settings_input: Option<String>,
    /// Label of the selected setting.
    pub settings_label: Option<String>,
}

/// UIA children of `node`, in tree order (tab bar, then panes, then
/// search; the tab bar's children are the tabs).
#[must_use]
pub fn children_of(node: Node, s: &Snapshot) -> Vec<Node> {
    match node {
        Node::Root => {
            let mut v = Vec::new();
            if s.bar_visible {
                v.push(Node::TabBar);
            }
            v.extend((0..s.grid_bounds.len()).map(Node::Grid));
            if s.search_bounds.is_some() {
                v.push(Node::Search);
            }
            if s.palette_bounds.is_some() {
                v.push(Node::Palette);
            }
            if s.settings_bounds.is_some() {
                v.push(Node::Settings);
            }
            v
        }
        Node::TabBar => (0..s.tabs.len()).map(Node::Tab).collect(),
        Node::Tab(_) | Node::Grid(_) | Node::Search | Node::Palette | Node::Settings => Vec::new(),
    }
}

/// UIA parent of `node`, `None` for the root.
#[must_use]
pub fn parent_of(node: Node) -> Option<Node> {
    match node {
        Node::Root => None,
        Node::TabBar | Node::Grid(_) | Node::Search | Node::Palette | Node::Settings => {
            Some(Node::Root)
        }
        Node::Tab(_) => Some(Node::TabBar),
    }
}

/// `node`'s parent, with `node` removed (its UIA siblings).
#[must_use]
pub fn siblings_of(node: Node, s: &Snapshot) -> Vec<Node> {
    let mut v = children_of(parent_of(node).unwrap_or(Node::Root), s);
    v.retain(|n| *n != node);
    v
}

/// Navigate to `direction` from `node` (UIA-001/002 tree walk).
#[must_use]
pub fn navigate(node: Node, direction: NavigateDirection, s: &Snapshot) -> Option<Node> {
    if direction == NavigateDirection_Parent {
        return parent_of(node);
    }
    let kids = children_of(node, s);
    // Compare with `==`, not patterns: the Win32 constants are PascalCase
    // and would trip the naming lints as patterns.
    if direction == NavigateDirection_FirstChild {
        kids.first().copied()
    } else if direction == NavigateDirection_LastChild {
        kids.last().copied()
    } else if direction == NavigateDirection_NextSibling
        || direction == NavigateDirection_PreviousSibling
    {
        // Walk the parent's full child list (the node is one of its own
        // UIA siblings); `siblings_of` excludes it, so use `children_of`.
        let sibs = children_of(parent_of(node).unwrap_or(Node::Root), s);
        let pos = sibs.iter().position(|n| *n == node)?;
        if direction == NavigateDirection_NextSibling {
            sibs.get(pos + 1).copied()
        } else {
            sibs.get(pos.checked_sub(1)?).copied()
        }
    } else {
        None
    }
}

/// `true` when the physical-screen-px point `(x, y)` lands inside `rect`
/// (client DIPs) of a window whose client origin is `origin` (physical px),
/// scaled by `dpi` (96 at 100%).
#[must_use]
pub fn contains_px(rect: DipRect, origin: (f32, f32), dpi: f32, x: f32, y: f32) -> bool {
    let s = dpi / 96.0;
    let l = origin.0 + rect.0 * s;
    let t = origin.1 + rect.1 * s;
    let r = origin.0 + rect.2 * s;
    let b = origin.1 + rect.3 * s;
    x >= l && x < r && y >= t && y < b
}

/// Deepest node containing the physical-px point `(x, y)` (UIA-005
/// `ElementProviderFromPoint`).
#[must_use]
pub fn hit_test(s: &Snapshot, origin: (f32, f32), x: f32, y: f32) -> Node {
    if s.bar_visible {
        for (i, &(lo, hi)) in s.tab_bounds.iter().enumerate() {
            let rect = (lo, s.bar_bounds.1, hi, s.bar_bounds.3);
            if contains_px(rect, origin, s.dpi, x, y) {
                return Node::Tab(i);
            }
        }
        if contains_px(s.bar_bounds, origin, s.dpi, x, y) {
            return Node::TabBar;
        }
    }
    for (i, rect) in s.grid_bounds.iter().copied().enumerate() {
        if contains_px(rect, origin, s.dpi, x, y) {
            return Node::Grid(i);
        }
    }
    if let Some(rect) = s.search_bounds
        && contains_px(rect, origin, s.dpi, x, y)
    {
        return Node::Search;
    }
    if let Some(rect) = s.palette_bounds
        && contains_px(rect, origin, s.dpi, x, y)
    {
        return Node::Palette;
    }
    if let Some(rect) = s.settings_bounds
        && contains_px(rect, origin, s.dpi, x, y)
    {
        return Node::Settings;
    }
    Node::Root
}

/// Bounding rectangle of `node` in physical screen px (UIA expects
/// absolute screen coordinates). `win` is the window rect, `origin` the
/// client-area origin, both physical px. Returns `(left, top, right, bottom)`.
#[must_use]
pub fn bounds_px(node: Node, s: &Snapshot, win: DipRect, origin: (f32, f32)) -> DipRect {
    let rect = match node {
        Node::Root => return win,
        Node::TabBar => s.bar_bounds,
        Node::Tab(i) => {
            let Some(&(lo, hi)) = s.tab_bounds.get(i) else {
                return (0.0, 0.0, 0.0, 0.0);
            };
            (lo, s.bar_bounds.1, hi, s.bar_bounds.3)
        }
        Node::Grid(i) => s
            .grid_bounds
            .get(i)
            .copied()
            .unwrap_or((0.0, 0.0, 0.0, 0.0)),
        Node::Search => s.search_bounds.unwrap_or((0.0, 0.0, 0.0, 0.0)),
        Node::Palette => s.palette_bounds.unwrap_or((0.0, 0.0, 0.0, 0.0)),
        Node::Settings => s.settings_bounds.unwrap_or((0.0, 0.0, 0.0, 0.0)),
    };
    let sc = s.dpi / 96.0;
    (
        origin.0 + rect.0 * sc,
        origin.1 + rect.1 * sc,
        origin.0 + rect.2 * sc,
        origin.1 + rect.3 * sc,
    )
}

fn control_type(node: Node) -> i32 {
    match node {
        Node::Root => UIA_WindowControlTypeId.0,
        Node::TabBar => UIA_TabControlTypeId.0,
        Node::Tab(_) => UIA_TabItemControlTypeId.0,
        Node::Grid(_) => UIA_DocumentControlTypeId.0,
        Node::Search => UIA_EditControlTypeId.0,
        Node::Palette => UIA_EditControlTypeId.0,
        Node::Settings => UIA_EditControlTypeId.0,
    }
}

// UIA property ids (`UIA_PROPERTY_ID` is a newtype of `i32`).
const P_CONTROL_TYPE: i32 = 30003;
const P_NAME: i32 = 30005;
const P_HELP_TEXT: i32 = 30013;
const P_HAS_KEYBOARD_FOCUS: i32 = 30008;
const P_IS_KEYBOARD_FOCUSABLE: i32 = 30009;
const P_IS_OFFSCREEN: i32 = 30015;
const P_IS_CONTROL: i32 = 30016;
const P_IS_CONTENT: i32 = 30017;
const P_VALUE: i32 = 30045;
const P_IS_SELECTED: i32 = 30079;
const P_POSITION_IN_SET: i32 = 30088;
const P_SIZE_OF_SET: i32 = 30089;
const P_LEGACY_VALUE: i32 = 30093;
const P_IS_WINDOW_ROOT: i32 = 30103;
const P_FULL_DESCRIPTION: i32 = 30159;

/// UIA properties of `node` (UIA-001/002/004) as `(property id, value)`
/// pairs; `GetPropertyValue` answers from this list and returns
/// `VT_EMPTY` for everything else.
#[must_use]
pub fn props(node: Node, s: &Snapshot) -> Vec<(i32, VARIANT)> {
    let mut out: Vec<(i32, VARIANT)> = Vec::new();
    let mut add = |id: i32, v: VARIANT| out.push((id, v));
    match node {
        Node::Root => {
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from(s.title.as_str()));
            add(P_HELP_TEXT, VARIANT::from(s.version.as_str()));
            add(P_IS_WINDOW_ROOT, VARIANT::from(true));
        }
        Node::TabBar => {
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from("Tabs"));
            add(P_IS_CONTROL, VARIANT::from(true));
            add(P_IS_CONTENT, VARIANT::from(true));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
        }
        Node::Tab(i) => {
            let Some(t) = s.tabs.get(i) else {
                return out;
            };
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from(t.title.as_str()));
            add(
                P_POSITION_IN_SET,
                VARIANT::from(i32::try_from(i + 1).unwrap_or(i32::MAX)),
            );
            add(
                P_SIZE_OF_SET,
                VARIANT::from(i32::try_from(s.tabs.len()).unwrap_or(i32::MAX)),
            );
            add(P_IS_SELECTED, VARIANT::from(i == s.active));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
            let mut help = format!("profile: {}", t.profile);
            if let Some(host) = &t.host {
                help.push_str(&format!(" · host: {host}"));
            }
            if let Some(cwd) = &t.cwd {
                help.push_str(&format!(" · cwd: {cwd}"));
            }
            help.push_str(if t.activity {
                " · activity: a command is waiting"
            } else {
                " · activity: idle"
            });
            add(P_HELP_TEXT, VARIANT::from(help.as_str()));
            add(
                P_FULL_DESCRIPTION,
                VARIANT::from(format!("Ctrl+Alt+{}", i + 1).as_str()),
            );
        }
        Node::Grid(i) => {
            let Some(p) = s.panes.get(i) else {
                return out;
            };
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(
                P_NAME,
                VARIANT::from(format!("Terminal pane {}", i + 1).as_str()),
            );
            add(P_IS_CONTROL, VARIANT::from(true));
            add(P_IS_CONTENT, VARIANT::from(true));
            add(P_IS_KEYBOARD_FOCUSABLE, VARIANT::from(true));
            add(P_HAS_KEYBOARD_FOCUS, VARIANT::from(i == s.focused_pane));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
            let mut value = p.scrollback_text.clone();
            if !value.is_empty() && !p.screen_text.is_empty() {
                value.push('\n');
            }
            value.push_str(&p.screen_text);
            add(P_VALUE, VARIANT::from(value.as_str()));
            add(P_LEGACY_VALUE, VARIANT::from(value.as_str()));
            let mut help = format!(
                "cursor row {}, column {} · {} scrollback lines",
                p.cursor
                    .map_or_else(|| "?".to_string(), |c| (c.0 + 1).to_string()),
                p.cursor
                    .map_or_else(|| "?".to_string(), |c| (c.1 + 1).to_string()),
                p.scrollback_lines
            );
            if let Some(sel) = &p.selection {
                let head: String = sel.chars().take(80).collect();
                help.push_str(&format!(" · selection: {head}"));
            }
            add(P_HELP_TEXT, VARIANT::from(help.as_str()));
        }
        Node::Search => {
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from("Search"));
            add(P_IS_CONTROL, VARIANT::from(true));
            add(P_IS_CONTENT, VARIANT::from(true));
            add(P_IS_KEYBOARD_FOCUSABLE, VARIANT::from(true));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
            let query = s.search_query.clone().unwrap_or_default();
            let (cur, total) = s.search_matches.unwrap_or((0, 0));
            let shown = if total == 0 { 0 } else { cur + 1 };
            let value = format!("Search: {query}  {shown} of {total}");
            add(P_VALUE, VARIANT::from(value.as_str()));
            add(P_LEGACY_VALUE, VARIANT::from(value.as_str()));
            add(
                P_HELP_TEXT,
                VARIANT::from("Enter to search, F3 next, Shift+F3 previous, Esc close"),
            );
        }
        Node::Palette => {
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from("Command palette"));
            add(P_IS_CONTROL, VARIANT::from(true));
            add(P_IS_CONTENT, VARIANT::from(true));
            add(P_IS_KEYBOARD_FOCUSABLE, VARIANT::from(true));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
            let query = s.palette_query.clone().unwrap_or_default();
            let (cur, total) = s.palette_matches.unwrap_or((0, 0));
            let shown = if total == 0 { 0 } else { cur + 1 };
            let value = format!("Palette: {query}  {shown} of {total}");
            add(P_VALUE, VARIANT::from(value.as_str()));
            add(P_LEGACY_VALUE, VARIANT::from(value.as_str()));
            add(
                P_HELP_TEXT,
                VARIANT::from("Type to filter, Up/Down to select, Enter to run, Esc close"),
            );
        }
        Node::Settings => {
            add(P_CONTROL_TYPE, VARIANT::from(control_type(node)));
            add(P_NAME, VARIANT::from("Settings"));
            add(P_IS_CONTROL, VARIANT::from(true));
            add(P_IS_CONTENT, VARIANT::from(true));
            add(P_IS_KEYBOARD_FOCUSABLE, VARIANT::from(true));
            add(P_IS_OFFSCREEN, VARIANT::from(false));
            let label = s.settings_label.clone().unwrap_or_default();
            let input = s.settings_input.clone().unwrap_or_default();
            let (cur, total) = s.settings_selected.unwrap_or((0, 0));
            let shown = if total == 0 { 0 } else { cur + 1 };
            let mut value = format!("Settings: {}  {} of {}", label, shown, total);
            if !input.is_empty() {
                value.push_str(&format!(" · value: {}", input));
            }
            add(P_VALUE, VARIANT::from(value.as_str()));
            add(P_LEGACY_VALUE, VARIANT::from(value.as_str()));
            add(
                P_HELP_TEXT,
                VARIANT::from("Type a TOML value, Enter applies, Up/Down selects, Esc close"),
            );
        }
    }
    out
}

/// COM provider for one node (UIA-001/002/004/005). State is never cached:
/// every call rebuilds a fresh [`Snapshot`] from the live window, and
/// navigation returns freshly created providers (UIA compares runtime ids,
/// not pointers), so the shell has no self/parent reference to manage.
#[derive(Clone)]
#[implement(
    IRawElementProviderSimple,
    IRawElementProviderFragment,
    IRawElementProviderFragmentRoot
)]
pub struct Provider(HWND, Node);

impl Provider {
    /// The root provider for a window (the `WM_GETOBJECT` answer, UIA-001).
    #[must_use]
    pub fn new(hwnd: HWND) -> Self {
        Self(hwnd, Node::Root)
    }

    /// Live snapshot for this provider's window, `None` once the window is
    /// gone.
    fn snapshot(&self) -> Option<Snapshot> {
        crate::window::uia_snapshot(self.0)
    }

    /// Window rect + client-area origin in physical px.
    fn geometry(&self) -> Option<(DipRect, (f32, f32))> {
        crate::window::uia_geometry(self.0)
    }

    /// Root provider of this window (the fragment root).
    fn root(&self) -> Result<IRawElementProviderFragmentRoot> {
        ComObject::from(Provider(self.0, Node::Root)).cast()
    }
}

impl IRawElementProviderSimple_Impl for Provider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions(0))
    }

    fn GetPatternProvider(&self, _patternid: UIA_PATTERN_ID) -> Result<IUnknown> {
        // shortcut: no control patterns are implemented. Handing back this
        // object's own IUnknown (which answers no pattern QI) is the
        // documented "no pattern" result.
        ComObject::from(Provider(self.0, self.1)).cast()
    }

    fn GetPropertyValue(&self, propertyid: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let Some(s) = self.snapshot() else {
            return Ok(VARIANT::default());
        };
        let value = props(self.1, &s)
            .into_iter()
            .find(|(pid, _)| *pid == propertyid.0)
            .map(|(_, v)| v);
        Ok(value.unwrap_or_default())
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        // Borrow the host window's own provider (bounds, name, focus).
        unsafe { UiaHostProviderFromHwnd(self.0) }
    }
}

impl IRawElementProviderFragment_Impl for Provider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let s = self.snapshot().ok_or(E_NOTIMPL)?;
        let target = navigate(self.1, direction, &s).ok_or(E_INVALIDARG)?;
        ComObject::from(Provider(self.0, target)).cast()
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        runtime_id_sa(self.1)
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        let rect = match (self.snapshot(), self.geometry()) {
            (Some(s), Some((win, origin))) => bounds_px(self.1, &s, win, origin),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        Ok(UiaRect {
            left: f64::from(rect.0),
            top: f64::from(rect.1),
            width: f64::from((rect.2 - rect.0).max(0.0)),
            height: f64::from((rect.3 - rect.1).max(0.0)),
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        // shortcut: tabs activate via Ctrl+Alt+N and panes via
        // Ctrl+Alt+arrows; programmatic focus needs the window's focus
        // state machine.
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        self.root()
    }
}

impl IRawElementProviderFragmentRoot_Impl for Provider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let s = self.snapshot().ok_or(E_NOTIMPL)?;
        let (_win, origin) = self.geometry().ok_or(E_NOTIMPL)?;
        let node = hit_test(&s, origin, x as f32, y as f32);
        ComObject::from(Provider(self.0, node)).cast()
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        let s = self.snapshot().ok_or(E_NOTIMPL)?;
        ComObject::from(Provider(self.0, Node::Grid(s.focused_pane))).cast()
    }
}

/// `runtime_id(node)` as a `VT_I4` safe array; UIA core owns and frees the
/// returned array.
fn runtime_id_sa(node: Node) -> Result<*mut SAFEARRAY> {
    let ids = runtime_id(node);
    let bound = SAFEARRAYBOUND {
        cElements: u32::try_from(ids.len()).unwrap_or(1),
        lLbound: 0,
    };
    let arr = unsafe { SafeArrayCreate(VT_I4, 1, &bound) };
    if arr.is_null() {
        return Err(E_OUTOFMEMORY.into());
    }
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        SafeArrayAccessData(arr, &mut data)?;
        let cells = data.cast::<i32>();
        for (i, id) in ids.iter().enumerate() {
            *cells.add(i) = *id;
        }
        SafeArrayUnaccessData(arr)?;
    }
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot {
            title: "pwsh — rustty".into(),
            version: "0.1.0".into(),
            tabs: vec![
                TabInfo {
                    title: "pwsh".into(),
                    profile: "PowerShell".into(),
                    cwd: Some("C:/src".into()),
                    host: None,
                    activity: false,
                },
                TabInfo {
                    title: "htop".into(),
                    profile: "cmd".into(),
                    cwd: None,
                    host: Some("db1.example".into()),
                    activity: true,
                },
            ],
            active: 1,
            bar_visible: true,
            bar_bounds: (0.0, 0.0, 500.0, 32.0),
            tab_bounds: vec![(0.0, 120.0), (120.0, 240.0)],
            client_w: 500.0,
            client_h: 400.0,
            dpi: 144.0,
            grid_bounds: vec![(8.0, 32.0, 492.0, 368.0)],
            panes: vec![PaneInfo {
                screen_text: "$ top".into(),
                scrollback_text: "old".into(),
                cursor: Some((0, 5)),
                scrollback_lines: 100,
                selection: Some("top".into()),
            }],
            focused_pane: 0,
            search_bounds: Some((0.0, 376.0, 500.0, 400.0)),
            search_query: Some("top".into()),
            search_matches: Some((0, 2)),
            palette_bounds: None,
            palette_query: None,
            palette_matches: None,
            settings_bounds: None,
            settings_selected: None,
            settings_input: None,
            settings_label: None,
        }
    }

    fn get(p: &[(i32, VARIANT)], id: i32) -> Option<&VARIANT> {
        p.iter().find(|(pid, _)| *pid == id).map(|(_, v)| v)
    }

    fn vstr(v: Option<&VARIANT>) -> String {
        // `BSTR` derefs to its UTF-16 slice; an empty BSTR yields "".
        v.map_or_else(String::new, |v| {
            let bstr = unsafe { &*v.Anonymous.Anonymous.Anonymous.bstrVal };
            String::from_utf16_lossy(bstr)
        })
    }

    fn vi32(v: Option<&VARIANT>) -> i32 {
        unsafe { v.expect("variant").Anonymous.Anonymous.Anonymous.lVal }
    }

    fn vbool(v: Option<&VARIANT>) -> bool {
        unsafe {
            v.expect("variant")
                .Anonymous
                .Anonymous
                .Anonymous
                .boolVal
                .as_bool()
        }
    }

    #[test]
    fn runtime_ids_are_unique() {
        let nodes = [
            Node::Root,
            Node::TabBar,
            Node::Tab(0),
            Node::Tab(1),
            Node::Grid(0),
            Node::Search,
        ];
        for (i, a) in nodes.iter().enumerate() {
            for (j, b) in nodes.iter().enumerate() {
                if i != j {
                    assert_ne!(runtime_id(*a), runtime_id(*b));
                }
            }
        }
    }

    #[test]
    fn root_children_skip_hidden_bar_and_search() {
        let s = snap();
        assert_eq!(
            children_of(Node::Root, &s),
            vec![Node::TabBar, Node::Grid(0), Node::Search]
        );
        let mut s = snap();
        s.bar_visible = false;
        s.search_bounds = None;
        assert_eq!(children_of(Node::Root, &s), vec![Node::Grid(0)]);
    }

    #[test]
    fn tab_bar_children_are_tabs() {
        let s = snap();
        assert_eq!(
            children_of(Node::TabBar, &s),
            vec![Node::Tab(0), Node::Tab(1)]
        );
    }

    #[test]
    fn parent_links() {
        assert_eq!(parent_of(Node::Root), None);
        assert_eq!(parent_of(Node::TabBar), Some(Node::Root));
        assert_eq!(parent_of(Node::Tab(1)), Some(Node::TabBar));
        assert_eq!(parent_of(Node::Grid(0)), Some(Node::Root));
        assert_eq!(parent_of(Node::Search), Some(Node::Root));
    }

    #[test]
    fn navigate_siblings() {
        let s = snap();
        assert_eq!(
            navigate(Node::TabBar, NavigateDirection_NextSibling, &s),
            Some(Node::Grid(0))
        );
        assert_eq!(
            navigate(Node::Grid(0), NavigateDirection_PreviousSibling, &s),
            Some(Node::TabBar)
        );
        assert_eq!(
            navigate(Node::Grid(0), NavigateDirection_NextSibling, &s),
            Some(Node::Search)
        );
        assert_eq!(
            navigate(Node::Search, NavigateDirection_NextSibling, &s),
            None
        );
        assert_eq!(
            navigate(Node::Search, NavigateDirection_PreviousSibling, &s),
            Some(Node::Grid(0))
        );
        assert_eq!(
            navigate(Node::Tab(0), NavigateDirection_Parent, &s),
            Some(Node::TabBar)
        );
        assert_eq!(
            navigate(Node::Tab(0), NavigateDirection_NextSibling, &s),
            Some(Node::Tab(1))
        );
        assert_eq!(
            navigate(Node::Tab(1), NavigateDirection_NextSibling, &s),
            None
        );
    }

    #[test]
    fn navigate_first_last_child() {
        let s = snap();
        assert_eq!(
            navigate(Node::Root, NavigateDirection_FirstChild, &s),
            Some(Node::TabBar)
        );
        assert_eq!(
            navigate(Node::Root, NavigateDirection_LastChild, &s),
            Some(Node::Search)
        );
        assert_eq!(
            navigate(Node::TabBar, NavigateDirection_FirstChild, &s),
            Some(Node::Tab(0))
        );
        assert_eq!(
            navigate(Node::TabBar, NavigateDirection_LastChild, &s),
            Some(Node::Tab(1))
        );
        assert_eq!(
            navigate(Node::Tab(0), NavigateDirection_FirstChild, &s),
            None
        );
        assert_eq!(navigate(Node::Root, NavigateDirection_Parent, &s), None);
    }

    #[test]
    fn hit_test_tabs_and_pane() {
        let s = snap();
        let origin = (10.0, 20.0); // client origin, physical px
        let sc = s.dpi / 96.0; // 1.5
        // Tab 1 spans DIP x 120..240 in the bar → px 190..370.
        assert_eq!(
            hit_test(&s, origin, 10.0 + 130.0 * sc, 20.0 + 16.0 * sc),
            Node::Tab(1)
        );
        // Bar strip away from tabs → TabBar.
        assert_eq!(
            hit_test(&s, origin, 10.0 + 300.0 * sc, 20.0 + 16.0 * sc),
            Node::TabBar
        );
        // Pane interior → Grid(0).
        assert_eq!(
            hit_test(&s, origin, 10.0 + 250.0 * sc, 20.0 + 200.0 * sc),
            Node::Grid(0)
        );
        // Search strip (DIP y 376..400).
        assert_eq!(
            hit_test(&s, origin, 10.0 + 250.0 * sc, 20.0 + 390.0 * sc),
            Node::Search
        );
        // A hidden bar falls through to the tab's pane: with the bar
        // hidden the grid starts at the client top (what the renderer
        // reports as grid_bounds).
        let mut s2 = snap();
        s2.bar_visible = false;
        s2.grid_bounds = vec![(8.0, 0.0, 492.0, 336.0)];
        assert_eq!(
            hit_test(&s2, origin, 10.0 + 130.0 * sc, 20.0 + 16.0 * sc),
            Node::Grid(0)
        );
        // Window frame corner → Root.
        assert_eq!(hit_test(&s, origin, 2.0, 2.0), Node::Root);
    }

    #[test]
    fn bounds_px_tab_and_grid() {
        let s = snap();
        let win = (0.0, 0.0, 800.0, 640.0);
        let origin = (0.0, 0.0);
        // Tab 1: DIP (120, 0, 240, 32) at 144 dpi → px (180, 0, 360, 48).
        assert_eq!(
            bounds_px(Node::Tab(1), &s, win, origin),
            (180.0, 0.0, 360.0, 48.0)
        );
        // Grid pane: DIP (8, 32, 492, 368) → px (12, 48, 738, 552).
        assert_eq!(
            bounds_px(Node::Grid(0), &s, win, origin),
            (12.0, 48.0, 738.0, 552.0)
        );
        // Root is the window rect verbatim.
        assert_eq!(bounds_px(Node::Root, &s, win, origin), win);
    }

    #[test]
    fn props_tab_items() {
        let s = snap();
        let p = props(Node::Tab(1), &s);
        assert_eq!(vstr(get(&p, P_NAME)), "htop");
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_TabItemControlTypeId.0);
        assert_eq!(vi32(get(&p, P_POSITION_IN_SET)), 2);
        assert_eq!(vi32(get(&p, P_SIZE_OF_SET)), 2);
        assert!(vbool(get(&p, P_IS_SELECTED)));
        let help = vstr(get(&p, P_HELP_TEXT));
        assert!(help.contains("cmd"));
        assert!(help.contains("activity: a command is waiting"));
        assert_eq!(vstr(get(&p, P_FULL_DESCRIPTION)), "Ctrl+Alt+2");
    }

    #[test]
    fn props_grid_value() {
        let s = snap();
        let p = props(Node::Grid(0), &s);
        assert_eq!(vstr(get(&p, P_VALUE)), "old\n$ top");
        assert_eq!(vstr(get(&p, P_LEGACY_VALUE)), "old\n$ top");
        let help = vstr(get(&p, P_HELP_TEXT));
        assert!(help.contains("cursor row 1, column 6"));
        assert!(help.contains("100 scrollback lines"));
        assert!(help.contains("selection: top"));
        assert!(vbool(get(&p, P_HAS_KEYBOARD_FOCUS)));
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_DocumentControlTypeId.0);
    }

    #[test]
    fn props_search() {
        let s = snap();
        let p = props(Node::Search, &s);
        assert_eq!(vstr(get(&p, P_NAME)), "Search");
        assert_eq!(vstr(get(&p, P_VALUE)), "Search: top  1 of 2");
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_EditControlTypeId.0);
    }

    #[test]
    fn props_palette() {
        let mut s = snap();
        s.palette_bounds = Some((0.0, 340.0, 500.0, 400.0));
        s.palette_query = Some("pa".into());
        s.palette_matches = Some((1, 7));
        let p = props(Node::Palette, &s);
        assert_eq!(vstr(get(&p, P_NAME)), "Command palette");
        assert_eq!(vstr(get(&p, P_VALUE)), "Palette: pa  2 of 7");
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_EditControlTypeId.0);
        // The open palette joins the root's children after the search strip.
        assert_eq!(children_of(Node::Root, &s).last(), Some(&Node::Palette));
        assert_eq!(parent_of(Node::Palette), Some(Node::Root));
        assert_eq!(runtime_id(Node::Palette), vec![1, 5]);
    }

    #[test]
    fn props_root() {
        let s = snap();
        let p = props(Node::Root, &s);
        assert_eq!(vstr(get(&p, P_NAME)), "pwsh — rustty");
        assert_eq!(vstr(get(&p, P_HELP_TEXT)), "0.1.0");
        assert!(vbool(get(&p, P_IS_WINDOW_ROOT)));
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_WindowControlTypeId.0);
    }

    #[test]
    fn props_unknown_indexes_are_empty() {
        let s = snap();
        assert!(props(Node::Tab(9), &s).is_empty());
        assert!(props(Node::Grid(9), &s).is_empty());
    }
    #[test]
    fn props_settings_overlay() {
        let mut s = snap();
        s.settings_bounds = Some((0.0, 340.0, 500.0, 400.0));
        s.settings_selected = Some((1, 3));
        s.settings_input = Some("copper-dark".into());
        s.settings_label = Some("theme = predawn (global)".to_string());
        let p = props(Node::Settings, &s);
        assert_eq!(vstr(get(&p, P_NAME)), "Settings");
        assert_eq!(
            vstr(get(&p, P_VALUE)),
            "Settings: theme = predawn (global)  2 of 3 · value: copper-dark"
        );
        assert_eq!(vi32(get(&p, P_CONTROL_TYPE)), UIA_EditControlTypeId.0);
        let help = vstr(get(&p, P_HELP_TEXT));
        assert!(help.contains("TOML"));
        assert_eq!(children_of(Node::Root, &s).last(), Some(&Node::Settings));
        assert_eq!(parent_of(Node::Settings), Some(Node::Root));
        assert_eq!(runtime_id(Node::Settings), vec![1, 6]);
    }
}
