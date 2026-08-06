//! Scoped CPU profiling.
//!
//! ```text
//! Frame
//! ├── Events        0.012 ms
//! ├── Update        0.004 ms
//! │   └── Systems   0.003 ms
//! └── Render        0.041 ms
//! ```
//!
//! A scope records when it opened, when it closed and how deeply it was nested, into a
//! buffer allocated once. Reconstructing the tree is done by the reader, from the depths;
//! the recording path does no tree building, no hashing and no allocation.
//!
//! ## Two modes
//!
//! * **Development** — [`PROFILING_ENABLED`] is true (debug builds, or the `profiling`
//!   feature). Scopes are timed and recorded.
//! * **Release** — false. [`profile_scope!`] expands to a guard that holds nothing and
//!   does nothing on drop, and the profiler stores no records. What remains is a
//!   predictable branch, not a timer.
//!
//! The feature exists so a release build can be profiled deliberately. Profiling a debug
//! build tells you about a debug build.
//!
//! ## Scope
//!
//! CPU only, one thread, one frame of history. GPU time comes from the renderer's
//! timestamp queries and is reported alongside in [`crate::metrics`] — the two are never
//! merged into one number, because GPU timestamps lag the CPU by several frames.
//!
//! Worker threads are not profiled yet: that needs a per-thread buffer and a merge step,
//! and there is no multi-threaded frame work to look at until there is.

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Whether scopes are timed and recorded.
///
/// True in debug builds and in any build with the `profiling` feature.
pub const PROFILING_ENABLED: bool = cfg!(any(debug_assertions, feature = "profiling"));

/// Scope records kept per frame before further ones are dropped.
///
/// Generous for hand-placed scopes and small enough to stay in cache. Overflow is counted
/// and reported rather than silently truncating.
pub const DEFAULT_CAPACITY: usize = 512;

/// One completed scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeRecord {
    /// Static label given at the call site.
    pub name: &'static str,
    /// Nesting depth; zero is a top-level scope.
    pub depth: u16,
    /// Offset from the start of the frame to when the scope opened.
    pub start: Duration,
    /// How long the scope was open.
    pub duration: Duration,
}

/// Records scope timings for the current frame.
///
/// Allocates its buffers once, in [`Profiler::new`]. Opening and closing a scope only
/// writes into them.
///
/// Interior mutability is deliberate: nested scopes are live at the same time, and each
/// holds a reference to the profiler. A `&mut` API would allow only one open scope.
#[derive(Debug)]
pub struct Profiler {
    state: RefCell<State>,
    enabled: bool,
}

/// The mutable half, behind one `RefCell`.
#[derive(Debug)]
struct State {
    records: Vec<ScopeRecord>,
    /// Indices of currently open scopes, innermost last.
    open: Vec<usize>,
    frame_start: Instant,
    frame_index: u64,
    /// Scopes dropped this frame because the buffer was full.
    overflow: u32,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl Profiler {
    /// Creates a profiler holding up to `capacity` scopes per frame.
    ///
    /// Enabled according to [`PROFILING_ENABLED`].
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: RefCell::new(State {
                records: Vec::with_capacity(capacity),
                open: Vec::with_capacity(32),
                frame_start: Instant::now(),
                frame_index: 0,
                overflow: 0,
            }),
            enabled: PROFILING_ENABLED,
        }
    }

    /// Creates a profiler that records nothing, whatever the build.
    ///
    /// For tests and for measuring the cost of the instrumentation itself.
    #[must_use]
    pub fn disabled() -> Self {
        let mut profiler = Self::new(0);
        profiler.enabled = false;
        profiler
    }

    /// Whether scopes are being recorded.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Starts a new frame, discarding the previous frame's records.
    ///
    /// One frame of history only. Keeping more would need a ring of buffers and a policy
    /// for reading it; the profiler exists to answer "where did this frame go", and the
    /// frame-time percentiles in [`crate::metrics`] answer the long-run question.
    /// Takes `&self` rather than `&mut self` so a profiler shared through an `Arc` — which
    /// is how the engine hands it to an application — can still start a frame.
    pub fn begin_frame(&self, frame_index: u64) {
        if !self.enabled {
            return;
        }
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        state.records.clear();
        state.open.clear();
        state.frame_start = Instant::now();
        state.frame_index = frame_index;
        state.overflow = 0;
    }

    /// Opens a scope; it closes when the returned guard drops.
    ///
    /// Prefer [`profile_scope!`], which also compiles the call away in a release build.
    #[must_use]
    pub fn scope(&self, name: &'static str) -> ScopeGuard<'_> {
        if !self.enabled {
            return ScopeGuard {
                profiler: None,
                index: 0,
            };
        }

        let mut state = self.state.borrow_mut();
        let start = state.frame_start.elapsed();
        let depth = state.open.len() as u16;

        if state.records.len() == state.records.capacity() {
            // Counted rather than grown: allocating mid-frame is exactly what the
            // profiler is supposed to help find, not to cause.
            state.overflow = state.overflow.saturating_add(1);
            return ScopeGuard {
                profiler: None,
                index: 0,
            };
        }

        let index = state.records.len();
        state.records.push(ScopeRecord {
            name,
            depth,
            start,
            duration: Duration::ZERO,
        });
        state.open.push(index);

        ScopeGuard {
            profiler: Some(self),
            index,
        }
    }

    /// Closes the scope at `index`.
    fn close(&self, index: usize) {
        let mut state = self.state.borrow_mut();
        let elapsed = state.frame_start.elapsed();
        if let Some(record) = state.records.get_mut(index) {
            record.duration = elapsed.saturating_sub(record.start);
        }
        // Scopes close in reverse order because the guards are dropped in reverse order,
        // so the innermost open index is the one being closed.
        state.open.pop();
    }

    /// Runs `body` inside a scope and returns its result.
    ///
    /// Convenient where a guard's lifetime would be awkward, such as around an expression
    /// whose value is returned immediately.
    pub fn profile<T>(&self, name: &'static str, body: impl FnOnce() -> T) -> T {
        let _scope = self.scope(name);
        body()
    }

    /// Copies this frame's records out.
    ///
    /// Allocates, so it belongs behind a key press or a reporting interval, not in the
    /// frame path.
    #[must_use]
    pub fn records(&self) -> Vec<ScopeRecord> {
        self.state.borrow().records.clone()
    }

    /// Number of scopes recorded this frame.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.state.borrow().records.len()
    }

    /// Scopes dropped this frame because the buffer was full.
    #[must_use]
    pub fn overflow_count(&self) -> u32 {
        self.state.borrow().overflow
    }

    /// Frame index the current records belong to.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.state.borrow().frame_index
    }

    /// Total time spent in scopes with this name, this frame.
    ///
    /// Nested scopes of the same name are counted once each, so a recursive scope sums to
    /// more than wall-clock time. Nothing in the engine nests a name today.
    #[must_use]
    pub fn total_for(&self, name: &str) -> Duration {
        self.state
            .borrow()
            .records
            .iter()
            .filter(|record| record.name == name)
            .map(|record| record.duration)
            .sum()
    }

    /// Renders the frame as an indented tree.
    ///
    /// Allocates; for a log line or a debug overlay, never for the frame path.
    #[must_use]
    pub fn format_frame(&self) -> String {
        use std::fmt::Write as _;

        let state = self.state.borrow();
        let mut text = format!("frame {}", state.frame_index);
        if !self.enabled {
            text.push_str(" (profiling disabled)");
            return text;
        }
        for record in &state.records {
            let indent = "  ".repeat(usize::from(record.depth) + 1);
            let _ = write!(
                text,
                "\n{indent}{:<24} {:>8.3} ms",
                record.name,
                record.duration.as_secs_f64() * 1e3
            );
        }
        if state.overflow > 0 {
            let _ = write!(text, "\n  ({} scopes dropped: buffer full)", state.overflow);
        }
        text
    }
}

/// Closes its scope when dropped.
///
/// A disabled profiler, or one whose buffer is full, yields a guard holding `None`: the
/// drop is then a null check with nothing behind it.
#[derive(Debug)]
pub struct ScopeGuard<'a> {
    profiler: Option<&'a Profiler>,
    index: usize,
}

impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        if let Some(profiler) = self.profiler {
            profiler.close(self.index);
        }
    }
}

/// Times the enclosing block, from the macro to the end of the scope.
///
/// ```
/// # use engine_core::{profile_scope, profiling::Profiler};
/// let profiler = Profiler::default();
/// {
///     profile_scope!(profiler, "Physics");
///     // ... work ...
/// }
/// ```
///
/// In a build without profiling the guard is created but holds nothing, so the cost is a
/// branch on a constant that the optimizer folds away.
#[macro_export]
macro_rules! profile_scope {
    ($profiler:expr, $name:literal) => {
        let _profile_scope_guard = if $crate::profiling::PROFILING_ENABLED {
            $profiler.scope($name)
        } else {
            $crate::profiling::ScopeGuard::inactive()
        };
    };
}

impl ScopeGuard<'static> {
    /// A guard that records nothing.
    ///
    /// Used by [`profile_scope!`] where profiling is compiled out.
    #[must_use]
    pub fn inactive() -> Self {
        Self {
            profiler: None,
            index: 0,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Burns a little time so a scope has a non-zero duration.
    fn spin(iterations: u32) -> u64 {
        let mut value = 1_u64;
        for index in 0..u64::from(iterations) {
            value = value
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(index);
        }
        value
    }

    fn enabled_profiler() -> Profiler {
        let profiler = Profiler::new(16);
        assert!(
            profiler.is_enabled(),
            "test builds must have profiling compiled in"
        );
        profiler
    }

    #[test]
    fn a_scope_is_recorded_with_its_name_and_duration() {
        let profiler = enabled_profiler();
        profiler.begin_frame(1);
        {
            let _scope = profiler.scope("Work");
            std::hint::black_box(spin(10_000));
        }

        let records = profiler.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "Work");
        assert_eq!(records[0].depth, 0);
        assert!(records[0].duration > Duration::ZERO);
        assert_eq!(profiler.frame_index(), 1);
    }

    #[test]
    fn nested_scopes_record_their_depth_and_nest_in_time() {
        let profiler = enabled_profiler();
        profiler.begin_frame(0);
        {
            let _outer = profiler.scope("Update");
            std::hint::black_box(spin(1_000));
            {
                let _inner = profiler.scope("Systems");
                std::hint::black_box(spin(5_000));
            }
            std::hint::black_box(spin(1_000));
        }

        let records = profiler.records();
        assert_eq!(records.len(), 2);
        assert_eq!((records[0].name, records[0].depth), ("Update", 0));
        assert_eq!((records[1].name, records[1].depth), ("Systems", 1));

        assert!(
            records[1].start >= records[0].start,
            "the inner scope opens after the outer one"
        );
        assert!(
            records[1].duration <= records[0].duration,
            "a nested scope cannot outlast its parent"
        );
    }

    #[test]
    fn sibling_scopes_are_recorded_at_the_same_depth() {
        let profiler = enabled_profiler();
        profiler.begin_frame(0);
        {
            let _first = profiler.scope("Events");
        }
        {
            let _second = profiler.scope("Render");
        }

        let records = profiler.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].depth, 0);
        assert_eq!(records[1].depth, 0);
        assert!(records[1].start >= records[0].start);
    }

    #[test]
    fn beginning_a_frame_discards_the_previous_one() {
        let profiler = enabled_profiler();
        profiler.begin_frame(1);
        {
            let _scope = profiler.scope("Work");
        }
        assert_eq!(profiler.record_count(), 1);

        profiler.begin_frame(2);
        assert_eq!(profiler.record_count(), 0, "one frame of history only");
        assert_eq!(profiler.frame_index(), 2);
    }

    #[test]
    fn overflowing_the_buffer_is_counted_rather_than_growing_it() {
        let profiler = Profiler::new(4);
        profiler.begin_frame(0);
        for _ in 0..10 {
            let _scope = profiler.scope("Work");
        }

        assert_eq!(profiler.record_count(), 4, "the buffer does not grow");
        assert_eq!(profiler.overflow_count(), 6);
        assert!(profiler.format_frame().contains("6 scopes dropped"));
    }

    #[test]
    fn totals_sum_every_scope_with_a_name() {
        let profiler = enabled_profiler();
        profiler.begin_frame(0);
        for _ in 0..3 {
            let _scope = profiler.scope("Repeated");
            std::hint::black_box(spin(1_000));
        }
        {
            let _other = profiler.scope("Other");
        }

        let repeated = profiler.total_for("Repeated");
        let single = profiler
            .records()
            .into_iter()
            .find(|record| record.name == "Repeated")
            .expect("the scope was recorded")
            .duration;

        assert!(repeated >= single, "three scopes sum to at least one");
        assert_eq!(profiler.total_for("Missing"), Duration::ZERO);
    }

    #[test]
    fn the_profile_helper_returns_the_body_result() {
        let profiler = enabled_profiler();
        profiler.begin_frame(0);
        let value = profiler.profile("Compute", || 6 * 7);
        assert_eq!(value, 42);
        assert_eq!(profiler.record_count(), 1);
    }

    #[test]
    fn a_disabled_profiler_records_nothing() {
        let profiler = Profiler::disabled();
        profiler.begin_frame(1);
        {
            let _outer = profiler.scope("Update");
            let _inner = profiler.scope("Systems");
            std::hint::black_box(spin(1_000));
        }

        assert!(!profiler.is_enabled());
        assert_eq!(profiler.record_count(), 0);
        assert_eq!(profiler.overflow_count(), 0);
        assert!(profiler.format_frame().contains("profiling disabled"));
    }

    #[test]
    fn the_macro_records_a_scope_where_profiling_is_enabled() {
        let profiler = enabled_profiler();
        profiler.begin_frame(0);
        {
            crate::profile_scope!(profiler, "Macro");
            std::hint::black_box(spin(1_000));
        }

        let records = profiler.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "Macro");
    }

    #[test]
    fn the_formatted_frame_indents_by_depth() {
        let profiler = enabled_profiler();
        profiler.begin_frame(7);
        {
            let _outer = profiler.scope("Update");
            let _inner = profiler.scope("Systems");
        }

        let text = profiler.format_frame();
        assert!(text.starts_with("frame 7"), "{text}");
        assert!(text.contains("\n  Update"), "{text}");
        assert!(text.contains("\n    Systems"), "{text}");
    }
}
