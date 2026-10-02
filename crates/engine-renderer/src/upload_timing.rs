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
/// Nanoseconds spent filtering mip chains on the CPU.
#[cfg(feature = "startup-timing")]
static MIPS_NS: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds spent hashing texels for deduplication and scanning them for transparency.
#[cfg(feature = "startup-timing")]
static INSPECT_NS: AtomicU64 = AtomicU64::new(0);
/// Textures that reached the upload (deduplicated hits excluded), and their level-0 texels.
#[cfg(feature = "startup-timing")]
static TEXTURES: AtomicU64 = AtomicU64::new(0);
/// See [`TEXTURES`].
#[cfg(feature = "startup-timing")]
static TEXELS: AtomicU64 = AtomicU64::new(0);

/// A running digest of every texture level uploaded, in upload order.
///
/// Off unless `MYGAME_TEXTURE_DIGEST` is set: hashing every byte of every level costs tens of
/// milliseconds, which would distort the startup timing this module exists for. Turned on, it
/// is how a change to the mip filter or the block compressor proves it produces the same
/// bytes as before - run both builds with it, compare one number.
#[cfg(feature = "startup-timing")]
static DIGEST: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

/// Folds one texture's final levels - after filtering and compression, exactly what is
/// copied to the GPU - into the digest. Nothing without `startup-timing` or the variable.
#[inline]
pub fn digest_texture(levels: &[&[u8]]) {
    #[cfg(feature = "startup-timing")]
    {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ENABLED.get_or_init(|| std::env::var_os("MYGAME_TEXTURE_DIGEST").is_some()) {
            return;
        }
        let Ok(mut digest) = DIGEST.lock() else {
            return;
        };
        // FNV-1a, a word at a time: it only has to be deterministic and to notice a change.
        let mut hash = digest.unwrap_or(0xcbf2_9ce4_8422_2325);
        for level in levels {
            hash = (hash ^ level.len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
            let mut words = level.chunks_exact(8);
            for word in &mut words {
                let mut bytes = [0_u8; 8];
                bytes.copy_from_slice(word);
                hash = (hash ^ u64::from_le_bytes(bytes)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            for byte in words.remainder() {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        *digest = Some(hash);
    }
    #[cfg(not(feature = "startup-timing"))]
    let _ = levels;
}

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

    /// Adds the elapsed time to the mip-filtering total, for one texture of `texels`
    /// level-0 texels.
    #[inline]
    pub fn mips(self, texels: u64) {
        #[cfg(feature = "startup-timing")]
        {
            MIPS_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
            TEXTURES.fetch_add(1, Ordering::Relaxed);
            TEXELS.fetch_add(texels, Ordering::Relaxed);
        }
        #[cfg(not(feature = "startup-timing"))]
        let _ = texels;
    }

    /// Adds the elapsed time to the texel-inspection total: hashing and the alpha scan.
    #[inline]
    pub fn inspect(self) {
        #[cfg(feature = "startup-timing")]
        {
            INSPECT_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Ordering::Relaxed);
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
        if let Ok(digest) = DIGEST.lock()
            && let Some(digest) = *digest
        {
            log::info!("[startup] texture digest so far: {digest:016x}");
        }
        log::info!(
            "[startup] textures so far: {} uploaded, {:.1} Mtexels at level 0 | {:.1} ms \
             hashing and scanning, {:.1} ms filtering mips",
            TEXTURES.load(Ordering::Relaxed),
            TEXELS.load(Ordering::Relaxed) as f64 / 1e6,
            INSPECT_NS.load(Ordering::Relaxed) as f64 / 1e6,
            MIPS_NS.load(Ordering::Relaxed) as f64 / 1e6,
        );
    }
}

/// The counters at one moment, for diffing across a frame.
///
/// A frame that hitches and also created a pipeline or flushed an upload has an obvious
/// suspect; this is what lets the application's hitch log say so. All zero without the
/// `startup-timing` feature, which is the only build that counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Pipeline state objects created so far.
    pub pipelines: u64,
    /// Nanoseconds spent creating them.
    pub pipeline_ns: u64,
    /// Resources created so far.
    pub resources: u64,
    /// Flushes that waited for the GPU so far.
    pub flushes: u64,
    /// Nanoseconds those waits took.
    pub flush_ns: u64,
}

impl Counters {
    /// What happened between `earlier` and `self`.
    #[must_use]
    pub fn since(self, earlier: Self) -> Self {
        Self {
            pipelines: self.pipelines.saturating_sub(earlier.pipelines),
            pipeline_ns: self.pipeline_ns.saturating_sub(earlier.pipeline_ns),
            resources: self.resources.saturating_sub(earlier.resources),
            flushes: self.flushes.saturating_sub(earlier.flushes),
            flush_ns: self.flush_ns.saturating_sub(earlier.flush_ns),
        }
    }
}

/// The counters now.
#[must_use]
#[inline]
pub fn counters() -> Counters {
    #[cfg(feature = "startup-timing")]
    {
        Counters {
            pipelines: PIPELINES.load(Ordering::Relaxed),
            pipeline_ns: PIPELINES_NS.load(Ordering::Relaxed),
            resources: RESOURCES.load(Ordering::Relaxed),
            flushes: FLUSHES.load(Ordering::Relaxed),
            flush_ns: FLUSH_NS.load(Ordering::Relaxed),
        }
    }
    #[cfg(not(feature = "startup-timing"))]
    {
        Counters::default()
    }
}
