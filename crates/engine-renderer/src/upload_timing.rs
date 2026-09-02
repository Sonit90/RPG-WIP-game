//! Where the time inside an upload goes: creating resources, compressing, copying.
//!
//! Three atomics and a log line, behind the `startup-timing` feature. Without the feature
//! every function here is an empty inline body and nothing is counted.
//!
//! It answers one question — "now that nothing waits for the GPU, what is a `create_model`
//! still paying for?" — and it is meant to be read once and then left alone.

#[cfg(feature = "startup-timing")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "startup-timing")]
use std::time::Instant;

/// Nanoseconds spent in `CreateCommittedResource`, over every resource.
#[cfg(feature = "startup-timing")]
static RESOURCES_NS: AtomicU64 = AtomicU64::new(0);
/// How many resources that was.
#[cfg(feature = "startup-timing")]
static RESOURCES: AtomicU64 = AtomicU64::new(0);
/// How many of them were buffers rather than textures, and what they cost.
#[cfg(feature = "startup-timing")]
static BUFFERS: AtomicU64 = AtomicU64::new(0);
/// See [`BUFFERS`].
#[cfg(feature = "startup-timing")]
static BUFFERS_NS: AtomicU64 = AtomicU64::new(0);
/// Pipeline state objects created, and what they cost.
#[cfg(feature = "startup-timing")]
static PIPELINES: AtomicU64 = AtomicU64::new(0);
/// See [`PIPELINES`].
#[cfg(feature = "startup-timing")]
static PIPELINES_NS: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds spent compressing texture mips on the CPU.
#[cfg(feature = "startup-timing")]
static COMPRESSION_NS: AtomicU64 = AtomicU64::new(0);
/// Flushes: submissions that waited for the GPU to go idle.
#[cfg(feature = "startup-timing")]
static FLUSHES: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds those waits took.
#[cfg(feature = "startup-timing")]
static FLUSH_NS: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds spent writing into mapped staging memory.
#[cfg(feature = "startup-timing")]
static COPY_NS: AtomicU64 = AtomicU64::new(0);

/// A stopwatch that is a zero-sized nothing without the feature.
#[cfg(feature = "startup-timing")]
pub struct Probe(Instant);
/// See the other definition.
#[cfg(not(feature = "startup-timing"))]
pub struct Probe;

impl Probe {
    /// Starts the stopwatch.
    #[must_use]
    #[inline]
    pub fn start() -> Self {
        #[cfg(feature = "startup-timing")]
        {
            Self(Instant::now())
        }
        #[cfg(not(feature = "startup-timing"))]
        {
            Self
        }
    }

    /// Adds the elapsed time to the resource-creation total.
    #[inline]
    pub fn resource(self) {
        #[cfg(feature = "startup-timing")]
        {
            RESOURCES_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            RESOURCES.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Adds the elapsed time to the resource total, and to the buffers' share of it.
    #[inline]
    pub fn buffer(self) {
        #[cfg(feature = "startup-timing")]
        {
            let elapsed = self.0.elapsed().as_nanos() as u64;
            RESOURCES_NS.fetch_add(elapsed, Ordering::Relaxed);
            RESOURCES.fetch_add(1, Ordering::Relaxed);
            BUFFERS_NS.fetch_add(elapsed, Ordering::Relaxed);
            BUFFERS.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Adds the elapsed time to the pipeline-creation total.
    #[inline]
    pub fn pipeline(self) {
        #[cfg(feature = "startup-timing")]
        {
            PIPELINES_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            PIPELINES.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Adds the elapsed time to the compression total.
    #[inline]
    pub fn compression(self) {
        #[cfg(feature = "startup-timing")]
        {
            COMPRESSION_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }

    /// Adds the elapsed time to the flush total: a submission and a wait for the GPU.
    #[inline]
    pub fn flush(self) {
        #[cfg(feature = "startup-timing")]
        {
            FLUSH_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            FLUSHES.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Adds the elapsed time to the staging-copy total.
    #[inline]
    pub fn copy(self) {
        #[cfg(feature = "startup-timing")]
        {
            COPY_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
}

/// Logs how many pipeline state objects have been created and what they cost.
///
/// Called when the first frame has been presented: mesh pipelines are created the first time
/// a material variant is drawn, so most of them appear during that frame and nowhere else.
#[inline]
pub fn report_pipelines() {
    #[cfg(feature = "startup-timing")]
    {
        log::info!(
            "[startup] pipelines so far: {} created in {:.1} ms",
            PIPELINES.load(Ordering::Relaxed),
            PIPELINES_NS.load(Ordering::Relaxed) as f64 / 1e6,
        );
    }
}

/// Logs the totals so far. Called when an upload batch ends.
#[inline]
pub fn report() {
    #[cfg(feature = "startup-timing")]
    {
        log::info!(
            "[startup] uploads so far: {} resources created in {:.1} ms (of which {} buffers, \
             {:.1} ms), {:.1} ms compressing, {:.1} ms copying into staging, {} flush(es) \
             waiting {:.1} ms",
            RESOURCES.load(Ordering::Relaxed),
            RESOURCES_NS.load(Ordering::Relaxed) as f64 / 1e6,
            BUFFERS.load(Ordering::Relaxed),
            BUFFERS_NS.load(Ordering::Relaxed) as f64 / 1e6,
            COMPRESSION_NS.load(Ordering::Relaxed) as f64 / 1e6,
            COPY_NS.load(Ordering::Relaxed) as f64 / 1e6,
            FLUSHES.load(Ordering::Relaxed),
            FLUSH_NS.load(Ordering::Relaxed) as f64 / 1e6,
        );
    }
}
