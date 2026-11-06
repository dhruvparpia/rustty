//! The event stream (ADR §5, issue #58). Every event is found by comparing the
//! window's state with what the last tick saw, so nothing here touches the PTY
//! read loop, the parser or the render path: pane output is read back from the
//! grid as plain text, in whole lines, after the window has already parsed it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::ops::ClientId;
use super::{OUTPUT_CHUNK_BYTES, encode_cursor, event_frame, read_lines};
use crate::app::App;
use crate::pane::Pane;
use crate::rail::Status;
use crate::shell::ShellState;

const OUTPUT: &str = "pane-output";
const PANE_CLOSED: &str = "pane-closed";

#[derive(Debug)]
struct Subscription {
    pane: Option<u64>,
    events: Option<Vec<String>>,
}

impl Subscription {
    fn wants(&self, event: &str, pane: Option<u64>) -> bool {
        let listed = self
            .events
            .as_ref()
            .map(|events| events.iter().any(|name| name == event));
        match self.pane {
            Some(mine) => pane == Some(mine) && listed.unwrap_or(true),
            // Everything a window prints is a lot to get by default.
            None => listed.unwrap_or(event != OUTPUT),
        }
    }
}

/// One client's subscriptions, and how far it has read each pane's output.
#[derive(Debug, Default)]
pub struct Subscriptions {
    subs: Vec<Subscription>,
    output: BTreeMap<u64, usize>,
}

impl Subscriptions {
    /// Nothing subscribed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    fn wants(&self, event: &str, pane: Option<u64>) -> bool {
        self.subs.iter().any(|sub| sub.wants(event, pane))
    }

    /// Subscribe, replacing any subscription for the same pane (ADR §5: one per
    /// client per pane). Existing panes' output starts now. A pane opened later
    /// starts at its first line, so nothing a short command prints is missed.
    pub fn subscribe(&mut self, app: &App, pane: Option<u64>, events: Option<Vec<String>>) {
        self.subs.retain(|sub| sub.pane != pane);
        self.subs.push(Subscription { pane, events });
        let subs = &self.subs;
        let output = &mut self.output;
        for tab in app.tabs() {
            tab.for_each_pane_ref(|p, _| {
                if subs.iter().any(|sub| sub.wants(OUTPUT, Some(p.id()))) {
                    output.entry(p.id()).or_insert_with(|| output_end(p));
                }
            });
        }
        self.prune_cursors();
    }

    /// Stop a subscription. Whether there was one.
    pub fn unsubscribe(&mut self, pane: Option<u64>) -> bool {
        let before = self.subs.len();
        self.subs.retain(|sub| sub.pane != pane);
        self.prune_cursors();
        self.subs.len() != before
    }

    fn prune_cursors(&mut self) {
        let subs = &self.subs;
        self.output
            .retain(|pane, _| subs.iter().any(|sub| sub.wants(OUTPUT, Some(*pane))));
    }

    fn pane_closed(&mut self, pane: u64) {
        self.subs.retain(|sub| sub.pane != Some(pane));
        self.output.remove(&pane);
    }
}

/// Where a pane's complete lines end (exclusive). The line the cursor is on is
/// still being written, unless the shell has gone and it never will be finished.
fn output_end(pane: &Pane) -> usize {
    let line = pane.cursor_abs();
    let finished = matches!(pane.shell().state(), ShellState::Exited(_));
    let has_text = finished
        && pane
            .grid()
            .line_text_abs(line)
            .is_some_and(|text| !text.is_empty());
    line + usize::from(has_text)
}

#[derive(Debug)]
struct PaneSeen {
    tab: u64,
    status: Status,
    detail: String,
    started: u64,
    finished: u64,
    exit: Option<i32>,
}

impl PaneSeen {
    /// A pane there when the baseline was taken: its past is not news.
    fn existing(pane: &Pane, tab: u64) -> Self {
        let commands = pane.commands();
        Self {
            tab,
            status: pane.rail_status(),
            detail: pane.rail_detail(),
            started: commands.iter().map(|c| c.id).max().unwrap_or(0),
            finished: commands
                .iter()
                .filter(|c| !c.running)
                .map(|c| c.id)
                .max()
                .unwrap_or(0),
            exit: None,
        }
    }

    /// A pane born since: everything it does is news.
    fn new_born(pane: &Pane, tab: u64) -> Self {
        Self {
            tab,
            status: pane.rail_status(),
            detail: pane.rail_detail(),
            started: 0,
            finished: 0,
            exit: None,
        }
    }
}

#[derive(Debug, Default)]
struct Baseline {
    tabs: BTreeSet<u64>,
    panes: BTreeMap<u64, PaneSeen>,
}

impl Baseline {
    fn of(app: &App) -> Self {
        let mut baseline = Self::default();
        for tab in app.tabs() {
            baseline.tabs.insert(tab.id());
            tab.for_each_pane_ref(|pane, _| {
                baseline
                    .panes
                    .insert(pane.id(), PaneSeen::existing(pane, tab.id()));
            });
        }
        baseline
    }
}

/// What the last scan saw, and the event counter. With nobody subscribed it
/// holds nothing and a scan costs one check.
#[derive(Debug, Default)]
pub struct Tracker {
    baseline: Option<Baseline>,
    seq: u64,
}

/// Sends one event to every client that wants it.
struct Sender<'a> {
    seq: &'a mut u64,
    subs: &'a BTreeMap<ClientId, Subscriptions>,
    send: &'a mut dyn FnMut(ClientId, &Value),
}

impl Sender<'_> {
    fn emit(&mut self, event: &str, pane: Option<u64>, fields: Value) {
        let mut frame = None;
        for (client, subs) in self.subs {
            let terminal = event == PANE_CLOSED
                && subs
                    .subs
                    .iter()
                    .any(|sub| sub.pane == pane && pane.is_some());
            if terminal || subs.wants(event, pane) {
                let frame = frame.get_or_insert_with(|| {
                    *self.seq += 1;
                    event_frame(event, *self.seq, fields.clone())
                });
                (self.send)(*client, frame);
            }
        }
    }
}

impl Tracker {
    /// Take the baseline now if nobody was subscribed until this moment, so a
    /// tab opened right after subscribing still counts as news.
    pub fn ensure_baseline(&mut self, app: &App) {
        if self.baseline.is_none() {
            self.baseline = Some(Baseline::of(app));
        }
    }

    /// Send what changed since the last scan.
    pub fn scan(
        &mut self,
        app: &App,
        subs: &mut BTreeMap<ClientId, Subscriptions>,
        send: &mut dyn FnMut(ClientId, &Value),
    ) {
        if subs.values().all(Subscriptions::is_empty) {
            self.baseline = None;
            return;
        }
        let Some(mut baseline) = self.baseline.take() else {
            self.baseline = Some(Baseline::of(app));
            return;
        };
        let mut live_tabs = BTreeSet::new();
        let mut live_panes = BTreeSet::new();
        {
            let mut sender = Sender {
                seq: &mut self.seq,
                subs,
                send: &mut *send,
            };
            for tab in app.tabs() {
                live_tabs.insert(tab.id());
                if baseline.tabs.insert(tab.id()) {
                    let mut panes = Vec::with_capacity(tab.pane_count());
                    tab.for_each_pane_ref(|pane, _| panes.push(pane.id()));
                    sender.emit(
                        "tab-opened",
                        None,
                        json!({
                            "tab": tab.id(),
                            "panes": panes,
                            "title": crate::rail::row_title(tab.title(), tab.profile()),
                        }),
                    );
                }
                tab.for_each_pane_ref(|pane, _| {
                    live_panes.insert(pane.id());
                    let seen = baseline
                        .panes
                        .entry(pane.id())
                        .or_insert_with(|| PaneSeen::new_born(pane, tab.id()));
                    scan_pane(pane, tab.id(), seen, &mut sender);
                });
            }
        }
        // Output is per client: each has read to its own point.
        for tab in app.tabs() {
            tab.for_each_pane_ref(|pane, _| {
                for (client, client_subs) in subs.iter_mut() {
                    send_output(pane, tab.id(), *client, client_subs, &mut self.seq, send);
                }
            });
        }
        let gone: Vec<u64> = baseline
            .panes
            .keys()
            .copied()
            .filter(|pane| !live_panes.contains(pane))
            .collect();
        for pane in gone {
            if let Some(seen) = baseline.panes.remove(&pane) {
                Sender {
                    seq: &mut self.seq,
                    subs,
                    send: &mut *send,
                }
                .emit(
                    PANE_CLOSED,
                    Some(pane),
                    json!({"pane": pane, "tab": seen.tab, "exitCode": seen.exit}),
                );
            }
            for client_subs in subs.values_mut() {
                client_subs.pane_closed(pane);
            }
        }
        let gone: Vec<u64> = baseline
            .tabs
            .iter()
            .copied()
            .filter(|tab| !live_tabs.contains(tab))
            .collect();
        for tab in gone {
            baseline.tabs.remove(&tab);
            Sender {
                seq: &mut self.seq,
                subs,
                send: &mut *send,
            }
            .emit("tab-closed", None, json!({"tab": tab}));
        }
        self.baseline = Some(baseline);
    }
}

/// One pane's bell, commands and status.
fn scan_pane(pane: &Pane, tab: u64, seen: &mut PaneSeen, sender: &mut Sender<'_>) {
    let id = pane.id();
    if let ShellState::Exited(code) = pane.shell().state() {
        seen.exit = code;
    }
    if pane.bell_pending() {
        sender.emit("bell", Some(id), json!({"pane": id, "tab": tab}));
    }
    for command in pane.commands().iter() {
        if command.id > seen.started {
            seen.started = command.id;
            sender.emit(
                "command-started",
                Some(id),
                json!({
                    "pane": id,
                    "tab": tab,
                    "command": command.id,
                    "cwd": command.cwd,
                    "startedAtMs": command.started_at_ms,
                }),
            );
        }
        if !command.running && command.id > seen.finished {
            seen.finished = command.id;
            sender.emit(
                "command-finished",
                Some(id),
                json!({
                    "pane": id,
                    "tab": tab,
                    "command": command.id,
                    "exitCode": command.exit,
                    "durationMs": command.duration_ms(),
                }),
            );
        }
    }
    let status = pane.rail_status();
    let detail = pane.rail_detail();
    if status != seen.status || detail != seen.detail {
        seen.status = status;
        sender.emit(
            "status-changed",
            Some(id),
            json!({
                "pane": id,
                "tab": tab,
                "status": status.as_str(),
                "detail": detail,
            }),
        );
        seen.detail = detail;
    }
}

/// Send one client the pane's new lines, up to one chunk. The alternate screen
/// has no history to report: read it with `pane.read`.
fn send_output(
    pane: &Pane,
    tab: u64,
    client: ClientId,
    subs: &mut Subscriptions,
    seq: &mut u64,
    send: &mut dyn FnMut(ClientId, &Value),
) {
    let id = pane.id();
    if pane.in_alt() || !subs.wants(OUTPUT, Some(id)) {
        return;
    }
    let grid = pane.grid();
    let cursor = *subs.output.entry(id).or_insert(0);
    let end = output_end(pane);
    if cursor >= end {
        return;
    }
    let first = cursor.max(grid.first_abs());
    let skipped = first - cursor;
    let page = if first < end {
        read_lines(first, end - 1, OUTPUT_CHUNK_BYTES, |abs| {
            grid.line_text_abs(abs)
        })
    } else {
        read_lines(first, 0, OUTPUT_CHUNK_BYTES, |_| None)
    };
    subs.output.insert(id, page.next);
    let mut fields = json!({
        "pane": id,
        "tab": tab,
        "text": page.text,
        "fromLine": page.from,
        "nextLine": page.next,
        "cursor": encode_cursor(id, page.next),
    });
    if skipped > 0 {
        fields["skipped"] = json!(skipped);
    }
    *seq += 1;
    send(client, &event_frame(OUTPUT, *seq, fields));
}

#[cfg(test)]
mod tests;
