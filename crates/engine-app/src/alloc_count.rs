//! Heap allocations counted across the whole process, behind the `alloc-count` feature.
//!
//! "The frame does not allocate in the steady state" is the kind of claim that quietly stops
//! being true, and the headless benches only see the parts of the frame they drive. This puts
//! the same counter `engine-bench` uses into the game itself, so a measured run can say how
//! many allocations a real frame — UI, renderer, presentation and all — makes.
//!
//! Process-wide: the logger's thread, the job workers and the audio thread are counted too.
//! The logger allocates once per record, which is a few per report interval, not per frame.
//!
//! Without the feature the game uses the system allocator directly and [`allocations`]
//! returns zero: a shipping build carries no counter.
//!
//! # Finding the allocations, not just counting them
//!
//! A count says a frame allocates; it does not say where. With `MYGAME_ALLOC_TRACE=<n>` set,
//! the first `n` allocations after the warm-up boundary ([`arm_trace`]) each capture a
//! backtrace, and [`report_traces`] prints them grouped by call site at the end of the run.
//! Capturing a backtrace allocates, so the tracing thread marks itself while it does and
//! those allocations are counted but not traced.

/// Allocations (including reallocations) and bytes requested since the process started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allocations {
    /// Calls to `alloc`, `alloc_zeroed` and `realloc`.
    pub count: u64,
    /// Bytes those calls asked for.
    pub bytes: u64,
}

impl Allocations {
    /// What happened between `earlier` and `self`.
    #[must_use]
    pub fn since(self, earlier: Self) -> Self {
        Self {
            count: self.count.saturating_sub(earlier.count),
            bytes: self.bytes.saturating_sub(earlier.bytes),
        }
    }
}

#[cfg(feature = "alloc-count")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(super) static COUNT: AtomicU64 = AtomicU64::new(0);
    pub(super) static BYTES: AtomicU64 = AtomicU64::new(0);
    /// Allocations still to be traced. Zero, and one relaxed load per allocation, unless
    /// `MYGAME_ALLOC_TRACE` armed it.
    pub(super) static TRACE_LEFT: AtomicU64 = AtomicU64::new(0);
    /// The traces captured so far: the size and the formatted backtrace.
    pub(super) static TRACES: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());

    thread_local! {
        /// Set while this thread is capturing a trace, whose own allocations are not traced.
        static TRACING: Cell<bool> = const { Cell::new(false) };
    }

    /// Captures where this allocation came from, if tracing is armed and this is not the
    /// tracer's own allocation.
    fn maybe_trace(size: usize) {
        if TRACE_LEFT.load(Ordering::Relaxed) == 0 {
            return;
        }
        // `try_with`: a thread being torn down has no thread-locals left, and an allocation
        // during its teardown must not panic inside the allocator.
        let _ = TRACING.try_with(|tracing| {
            if tracing.get() {
                return;
            }
            let claimed = TRACE_LEFT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
            if !claimed {
                return;
            }
            tracing.set(true);
            let trace = std::backtrace::Backtrace::force_capture().to_string();
            if let Ok(mut traces) = TRACES.lock() {
                traces.push((size, trace));
            }
            tracing.set(false);
        });
    }

    /// The system allocator with two relaxed atomic adds in front of it.
    struct CountingAllocator;

    // SAFETY: every method forwards to the system allocator with the same arguments and
    // returns its result unchanged, so the allocator contract is upheld by `System`. The
    // counters are side effects that touch no allocation state.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            maybe_trace(layout.size());
            // SAFETY: `layout` came from the caller and is forwarded unchanged.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            maybe_trace(layout.size());
            // SAFETY: `layout` came from the caller and is forwarded unchanged.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: `pointer` was returned by this allocator with this `layout`.
            unsafe { System.dealloc(pointer, layout) };
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            maybe_trace(new_size);
            // SAFETY: `pointer` was returned by this allocator with this `layout`, and
            // `new_size` is the caller's, forwarded unchanged.
            unsafe { System.realloc(pointer, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}

/// Allocations so far. Zero without the `alloc-count` feature.
#[must_use]
#[inline]
pub fn allocations() -> Allocations {
    #[cfg(feature = "alloc-count")]
    {
        use std::sync::atomic::Ordering;
        Allocations {
            count: counting::COUNT.load(Ordering::Relaxed),
            bytes: counting::BYTES.load(Ordering::Relaxed),
        }
    }
    #[cfg(not(feature = "alloc-count"))]
    {
        Allocations::default()
    }
}

/// Whether this build counts allocations at all.
pub const COUNTING: bool = cfg!(feature = "alloc-count");

/// Starts tracing the next `MYGAME_ALLOC_TRACE` allocations, if the variable is set. Called at
/// the warm-up boundary, so what is traced is the steady state rather than the load.
pub fn arm_trace() {
    #[cfg(feature = "alloc-count")]
    if let Some(count) = std::env::var("MYGAME_ALLOC_TRACE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        counting::TRACE_LEFT.store(count, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Logs the traced allocations grouped by backtrace, most frequent first, keeping only the
/// frames inside this workspace so a group reads as a call path rather than a page of
/// allocator and runtime internals.
pub fn report_traces() {
    #[cfg(feature = "alloc-count")]
    {
        counting::TRACE_LEFT.store(0, std::sync::atomic::Ordering::Relaxed);
        let Ok(mut traces) = counting::TRACES.lock() else {
            return;
        };
        if traces.is_empty() {
            return;
        }
        // The first trace in full, for when the grouping below finds no workspace frame.
        if std::env::var_os("MYGAME_ALLOC_TRACE_RAW").is_some()
            && let Some((size, trace)) = traces.first()
        {
            log::info!(
                "first traced allocation, {size} B, raw:
{trace}"
            );
        }
        let mut groups: Vec<(String, usize, usize)> = Vec::new();
        for (size, trace) in traces.drain(..) {
            // The frames that name our code: `crates\engine-...` source paths, and the line
            // above each, which is the function.
            let lines: Vec<&str> = trace.lines().collect();
            let mut path = String::new();
            for (index, line) in lines.iter().enumerate() {
                if line.contains("crates\\engine-") || line.contains("crates/engine-") {
                    if let Some(function) = index.checked_sub(1).and_then(|at| lines.get(at)) {
                        path.push_str(function.trim());
                        path.push_str("\n      ");
                    }
                    path.push_str(line.trim());
                    path.push_str("\n    ");
                }
            }
            if path.is_empty() {
                path = "(no workspace frame)".to_owned();
            }
            match groups.iter_mut().find(|group| group.0 == path) {
                Some(group) => {
                    group.1 += 1;
                    group.2 += size;
                }
                None => groups.push((path, 1, size)),
            }
        }
        groups.sort_by_key(|group| std::cmp::Reverse(group.1));
        for (path, count, bytes) in groups {
            log::info!("traced allocation x{count} ({bytes} B):\n    {path}");
        }
    }
}
