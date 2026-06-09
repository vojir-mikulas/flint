// SPDX-License-Identifier: GPL-3.0-or-later

//! **M0 spike A — streaming → bounded-window → virtualized grid (+ cancel).**
//!
//! Proves the shape RED's result grid (M5) lives or dies on: an unknown-length
//! row source feeding a fixed-size [`WindowBuffer`] that feeds Flint's `Table`,
//! so memory stays flat no matter how many rows the source claims, plus a
//! *working out-of-band cancel* of a long query via SQLite's interrupt handle —
//! the cross-milestone finding that informs M1/M7's real cancel.
//!
//! Two interchangeable [`RowSource`]s behind one trait keep the seam honest:
//! a **synthetic** generator (10M rows, zero I/O — isolates the buffer) and a
//! **real SQLite** table of 2M rows read by keyset-paged windows (proves true
//! streaming over real data). Both are example-only; the Flint *library* stays
//! domain-free — its sole addition is the generic `Table::on_visible_range` hook.
//!
//! Synchronous-on-purpose: window reads run inline in `on_visible_range`, before
//! `render_row` reads the buffer in the same paint. That deliberately *isn't*
//! the production path (M1 moves reads to the `red-service` thread) — but it
//! isolates the buffer/grid interaction from async so the spike answers one
//! question at a time. The cancel demo is the one genuinely concurrent path.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{Hsla, SharedString};
use rusqlite::{Connection, InterruptHandle};

/// Rows kept around the viewport on each side. The buffer never holds more than
/// roughly `visible + 2 * MARGIN` rows — this is the bound that makes memory flat.
const MARGIN: usize = 200;

/// Synthetic source size — large enough that materializing it would be absurd,
/// which is the point.
pub const SYNTHETIC_ROWS: usize = 10_000_000;
/// Real-SQLite source size. Generated once into a temp file on first launch.
pub const SQLITE_ROWS: usize = 2_000_000;

/// How a cell should be colored — a generic, render-oriented tag, *not*
/// `red_core::Value`. RED maps its own `Value` onto something like this later.
#[derive(Clone, Copy, PartialEq)]
pub enum CellKind {
    Null,
    Num,
    Text,
    Json,
    Bool(bool),
}

#[derive(Clone)]
pub struct Cell {
    pub text: SharedString,
    pub kind: CellKind,
}

impl Cell {
    fn new(text: impl Into<SharedString>, kind: CellKind) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }
}

/// Per-column metadata the header and cell alignment read.
#[derive(Clone)]
pub struct ColumnSpec {
    pub name: SharedString,
    pub numeric: bool,
}

/// The generic seam: an unknown-but-here-known-length, randomly-addressable row
/// source. `fetch` returns exactly the requested window; the buffer owns
/// caching and eviction. Object-safe so the active source is an `Rc<dyn>`.
pub trait RowSource {
    fn columns(&self) -> &[ColumnSpec];
    fn total(&self) -> usize;
    fn fetch(&self, range: Range<usize>) -> Vec<Vec<Cell>>;
}

/// The bounded window between a [`RowSource`] and the grid. Holds only rows near
/// the viewport; everything else is evicted, so `len()` plateaus regardless of
/// [`RowSource::total`]. All the stats fields exist to make that *visible* in the
/// gallery readout — proving "flat memory" rather than asserting it.
#[derive(Default)]
pub struct WindowBuffer {
    rows: BTreeMap<usize, Vec<Cell>>,
    window: Range<usize>,
    last_visible: Range<usize>,
    last_fetch: Option<Range<usize>>,
    fetches: u64,
    rows_read: u64,
}

impl WindowBuffer {
    /// Ensure every row in the window around `visible` is buffered, fetching only
    /// the contiguous *missing* spans (a small band at the leading edge while
    /// scrolling), then evict everything outside the window. Runs inline during
    /// the table's paint.
    pub fn ensure(&mut self, source: &dyn RowSource, visible: Range<usize>) {
        let total = source.total();
        self.last_visible = visible.clone();
        let w_start = visible.start.saturating_sub(MARGIN);
        let w_end = (visible.end + MARGIN).min(total);
        let window = w_start..w_end;

        let mut ix = w_start;
        while ix < w_end {
            if self.rows.contains_key(&ix) {
                ix += 1;
                continue;
            }
            let span_start = ix;
            while ix < w_end && !self.rows.contains_key(&ix) {
                ix += 1;
            }
            let span = span_start..ix;
            let fetched = source.fetch(span.clone());
            self.fetches += 1;
            self.rows_read += fetched.len() as u64;
            self.last_fetch = Some(span.clone());
            for (k, row) in span.zip(fetched) {
                self.rows.insert(k, row);
            }
        }

        // Bound memory: drop everything the viewport no longer covers.
        self.rows.retain(|k, _| window.contains(k));
        self.window = window;
    }

    pub fn get(&self, ix: usize) -> Option<&Vec<Cell>> {
        self.rows.get(&ix)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn buffered(&self) -> usize {
        self.rows.len()
    }
    pub fn window(&self) -> Range<usize> {
        self.window.clone()
    }
    pub fn last_fetch(&self) -> Option<Range<usize>> {
        self.last_fetch.clone()
    }
    pub fn fetches(&self) -> u64 {
        self.fetches
    }
    pub fn rows_read(&self) -> u64 {
        self.rows_read
    }
}

// ---------------------------------------------------------------------------
// Synthetic source — deterministic, zero I/O. Isolates the buffer/grid loop.
// ---------------------------------------------------------------------------

pub struct SyntheticSource {
    columns: Vec<ColumnSpec>,
}

impl Default for SyntheticSource {
    fn default() -> Self {
        Self {
            columns: vec![
                col("id", true),
                col("user_id", true),
                col("email", false),
                col("score", true),
                col("active", false),
                col("meta", false),
            ],
        }
    }
}

impl RowSource for SyntheticSource {
    fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }
    fn total(&self) -> usize {
        SYNTHETIC_ROWS
    }
    fn fetch(&self, range: Range<usize>) -> Vec<Vec<Cell>> {
        range
            .map(|i| {
                let id = i as i64 + 1;
                // A cheap deterministic scramble so columns look like real data.
                let h = (id.wrapping_mul(2654435761)) & 0x7fff_ffff;
                vec![
                    Cell::new(id.to_string(), CellKind::Num),
                    Cell::new((100_000 + h % 900_000).to_string(), CellKind::Num),
                    // Sprinkle NULLs so the NULL renderer is exercised.
                    if id % 17 == 0 {
                        Cell::new("NULL", CellKind::Null)
                    } else {
                        Cell::new(format!("user{id}@example.com"), CellKind::Text)
                    },
                    Cell::new(format!("{:.2}", (h % 10_000) as f64 / 100.0), CellKind::Num),
                    Cell::new(
                        if h % 2 == 0 { "true" } else { "false" },
                        CellKind::Bool(h % 2 == 0),
                    ),
                    Cell::new(format!("{{\"shard\":{}}}", h % 64), CellKind::Json),
                ]
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// SQLite source — a real 2M-row table read by keyset-paged windows.
// ---------------------------------------------------------------------------

pub struct SqliteSource {
    conn: Connection,
    columns: Vec<ColumnSpec>,
    total: usize,
}

impl SqliteSource {
    /// Open the generated DB and read its row count. Cheap — the heavy one-time
    /// generation happens off-thread in [`spawn_generate`].
    pub fn open(path: &PathBuf) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let total: usize =
            conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))? as usize;
        Ok(Self {
            conn,
            total,
            columns: vec![
                col("id", true),
                col("user_id", true),
                col("email", false),
                col("score", true),
                col("active", false),
                col("meta", false),
            ],
        })
    }
}

impl RowSource for SqliteSource {
    fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }
    fn total(&self) -> usize {
        self.total
    }
    fn fetch(&self, range: Range<usize>) -> Vec<Vec<Cell>> {
        // Keyset window: ids are a contiguous 1..=N PK, so this is O(window),
        // not O(offset) like LIMIT/OFFSET would be on a huge table.
        let lo = range.start as i64 + 1;
        let hi = range.end as i64; // inclusive upper id == exclusive upper index
        let mut stmt = match self
            .conn
            .prepare_cached("SELECT id, user_id, email, score, active, meta FROM t WHERE id >= ?1 AND id <= ?2 ORDER BY id")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([lo, hi], |row| {
            Ok(vec![
                int_cell(row, 0),
                int_cell(row, 1),
                text_cell(row, 2),
                real_cell(row, 3),
                bool_cell(row, 4),
                json_cell(row, 5),
            ])
        });
        match rows {
            Ok(mapped) => mapped.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn col(name: &str, numeric: bool) -> ColumnSpec {
    ColumnSpec {
        name: name.into(),
        numeric,
    }
}

fn int_cell(row: &rusqlite::Row, i: usize) -> Cell {
    match row.get::<_, Option<i64>>(i) {
        Ok(Some(n)) => Cell::new(n.to_string(), CellKind::Num),
        _ => Cell::new("NULL", CellKind::Null),
    }
}
fn real_cell(row: &rusqlite::Row, i: usize) -> Cell {
    match row.get::<_, Option<f64>>(i) {
        Ok(Some(x)) => Cell::new(format!("{x:.2}"), CellKind::Num),
        _ => Cell::new("NULL", CellKind::Null),
    }
}
fn text_cell(row: &rusqlite::Row, i: usize) -> Cell {
    match row.get::<_, Option<String>>(i) {
        Ok(Some(s)) => Cell::new(s, CellKind::Text),
        _ => Cell::new("NULL", CellKind::Null),
    }
}
fn json_cell(row: &rusqlite::Row, i: usize) -> Cell {
    match row.get::<_, Option<String>>(i) {
        Ok(Some(s)) => Cell::new(s, CellKind::Json),
        _ => Cell::new("NULL", CellKind::Null),
    }
}
fn bool_cell(row: &rusqlite::Row, i: usize) -> Cell {
    match row.get::<_, Option<i64>>(i) {
        Ok(Some(n)) => Cell::new(
            if n != 0 { "true" } else { "false" },
            CellKind::Bool(n != 0),
        ),
        _ => Cell::new("NULL", CellKind::Null),
    }
}

/// Path of the generated spike DB (temp dir, reused across runs).
pub fn db_path() -> PathBuf {
    std::env::temp_dir().join("flint-streaming-spike.sqlite")
}

/// Generate the `t` table off the UI thread if it isn't already present with the
/// right row count. Flips `ready` to `true` when the table is queryable. The
/// connection here is `!Send`-safe because it lives entirely on this thread.
pub fn spawn_generate(ready: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let path = db_path();
        if let Ok(conn) = Connection::open(&path) {
            let count: i64 = conn
                .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
                .unwrap_or(-1);
            if count == SQLITE_ROWS as i64 {
                ready.store(true, Ordering::SeqCst);
                return;
            }
        }
        let _ = std::fs::remove_file(&path);
        let mut conn = match Connection::open(&path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let _ = conn.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
             CREATE TABLE t (id INTEGER PRIMARY KEY, user_id INTEGER, email TEXT, score REAL, active INTEGER, meta TEXT);",
        );
        if let Ok(tx) = conn.transaction() {
            {
                let Ok(mut stmt) = tx.prepare(
                    "INSERT INTO t (id, user_id, email, score, active, meta) VALUES (?1,?2,?3,?4,?5,?6)",
                ) else {
                    return;
                };
                for i in 0..SQLITE_ROWS as i64 {
                    let id = i + 1;
                    let h = (id.wrapping_mul(2654435761)) & 0x7fff_ffff;
                    let email = if id % 17 == 0 {
                        None
                    } else {
                        Some(format!("user{id}@example.com"))
                    };
                    let _ = stmt.execute(rusqlite::params![
                        id,
                        100_000 + h % 900_000,
                        email,
                        (h % 10_000) as f64 / 100.0,
                        (h % 2 == 0) as i64,
                        format!("{{\"shard\":{}}}", h % 64),
                    ]);
                }
            }
            let _ = tx.commit();
        }
        ready.store(true, Ordering::SeqCst);
    });
}

// ---------------------------------------------------------------------------
// Cancel demo — a long, interruptible query on a background thread.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub enum SlowStatus {
    Idle,
    Running,
    Done { rows: i64, ms: u128 },
    Cancelled { ms: u128 },
    Error(String),
}

/// Shared between the worker thread and the UI. The `InterruptHandle` is
/// `Send + Sync`, so the UI can abort an in-flight scan out of band — exactly
/// the mechanism M1/M7 need (a dropped future would *not* stop the scan).
pub struct SlowShared {
    pub status: SlowStatus,
    pub interrupt: Option<InterruptHandle>,
}

impl Default for SlowShared {
    fn default() -> Self {
        Self {
            status: SlowStatus::Idle,
            interrupt: None,
        }
    }
}

/// Kick off a deliberately heavy, cancellable scan on a worker thread.
pub fn run_slow(shared: Arc<Mutex<SlowShared>>) {
    {
        let mut g = shared.lock().unwrap();
        g.status = SlowStatus::Running;
        g.interrupt = None;
    }
    std::thread::spawn(move || {
        let path = db_path();
        let conn = match Connection::open(&path) {
            Ok(c) => c,
            Err(e) => {
                shared.lock().unwrap().status = SlowStatus::Error(e.to_string());
                return;
            }
        };
        // Publish the interrupt handle *before* running so Cancel can reach it.
        shared.lock().unwrap().interrupt = Some(conn.get_interrupt_handle());

        let start = Instant::now();
        // A heavy correlated scan: long enough to cancel by hand.
        let res = conn.query_row(
            "SELECT count(*) FROM t a JOIN t b ON b.id = (a.id % 100000) + 1 WHERE b.score > 0",
            [],
            |r| r.get::<_, i64>(0),
        );
        let ms = start.elapsed().as_millis();
        let mut g = shared.lock().unwrap();
        g.interrupt = None;
        g.status = match res {
            Ok(rows) => SlowStatus::Done { rows, ms },
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                SlowStatus::Cancelled { ms }
            }
            Err(e) => SlowStatus::Error(e.to_string()),
        };
    });
}

/// Trigger the out-of-band cancel. Returns `true` if a scan was in flight.
pub fn cancel_slow(shared: &Arc<Mutex<SlowShared>>) -> bool {
    let g = shared.lock().unwrap();
    if let Some(handle) = g.interrupt.as_ref() {
        handle.interrupt();
        true
    } else {
        false
    }
}

impl SlowStatus {
    pub fn label(&self) -> String {
        match self {
            SlowStatus::Idle => "idle".into(),
            SlowStatus::Running => "running… (Cancel to interrupt)".into(),
            SlowStatus::Done { rows, ms } => format!("done: {rows} rows in {ms} ms"),
            SlowStatus::Cancelled { ms } => format!("cancelled after {ms} ms ✓"),
            SlowStatus::Error(e) => format!("error: {e}"),
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, SlowStatus::Running)
    }
}

/// Resolve a [`CellKind`] to a paint color from a snapshot of theme tokens (the
/// row closure is `'static`, so it can't hold `cx`).
#[derive(Clone, Copy)]
pub struct CellColors {
    pub text: Hsla,
    pub faint: Hsla,
    pub num: Hsla,
    pub json: Hsla,
    pub bool_true: Hsla,
    pub bool_false: Hsla,
}

impl CellColors {
    pub fn for_kind(&self, kind: CellKind) -> Hsla {
        match kind {
            CellKind::Null => self.faint,
            CellKind::Num => self.num,
            CellKind::Text => self.text,
            CellKind::Json => self.json,
            CellKind::Bool(true) => self.bool_true,
            CellKind::Bool(false) => self.bool_false,
        }
    }
}
