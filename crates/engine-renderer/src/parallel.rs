//! One large CPU pass over an image, cut into bands of rows and run on several threads.
//!
//! # Why this exists
//!
//! Filtering a mip chain and block-compressing it are the two largest CPU costs of a launch:
//! the imported hero textures are 2048x2048, and before this the whole of every one of them
//! went through one core while eleven others waited for the device. Both passes compute each
//! output row (or row of blocks) from its own inputs and nothing else, so they split into
//! bands with no change to a single output byte - the bands are written into disjoint slices
//! of the one output buffer, in place.
//!
//! # Why scoped threads rather than the job system
//!
//! This runs while textures are uploaded, inside `Renderer::create_texture`, which every
//! loader in the application calls and which has no job system to hand: the application's
//! pool is built after the startup uploads, and threading one through every `create_model`
//! call site would change a dozen signatures to save a few hundred microseconds of thread
//! creation per large texture. A band is only cut when it carries at least
//! [`MIN_BAND_TEXELS`] of work, so a small texture never pays for a thread at all.

use std::ops::Range;
use std::sync::OnceLock;

/// The least work, in texels, worth handing to a thread of its own.
///
/// Creating a thread on Windows is tens of microseconds. A texel of colour mip filtering or
/// of block compression is roughly twenty nanoseconds, so 16K texels is a band of a few
/// hundred microseconds: an order of magnitude more than it costs to start.
pub const MIN_BAND_TEXELS: usize = 16 * 1024;

/// The most threads one pass is split across, the caller's included.
///
/// Measured on a 12-core, 24-thread Ryzen 9 5900X, dungeon launch, five runs each (median):
///
/// ```text
/// bands          4      8      12     16     24
/// mip filtering  38.7   26.0   23.2   22.1   20.0 ms
/// compression    82.3   51.5   42.9   41.3   35.5 ms
/// launch         448    403    392    389    380 ms
/// ```
///
/// Still improving at every logical processor, so the cap is only there to stop a very wide
/// machine from creating more threads per pass than a 2048x2048 level has bands worth cutting.
pub const MAX_BANDS: usize = 32;

thread_local! {
    /// This thread's own cap on bands, if it set one.
    static THREAD_LIMIT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Caps how many threads a pass started on the *calling* thread is split across; `None`
/// lifts the cap.
///
/// For work done in the background while something on another thread matters more. The
/// application prepares the hero and monster textures on a thread while the main thread
/// creates the D3D12 device, and the driver initializing inside `D3D12CreateDevice` is slowed
/// by every core the preparation takes. Measured on the 24-thread reference machine, dungeon
/// launch, five runs each (median):
///
/// ```text
/// bands on the preload thread    4      8      12     24
/// D3D12CreateDevice + swap chain 115.2  115.9  122.5  128.2 ms
/// preparing the textures         127.8  84.8   75.2   64.6 ms
/// launch                         248.2  246.3  251.4  256.3 ms
/// ```
///
/// Eight leaves the driver its cores and still finishes long before the main thread asks.
pub fn limit_bands_on_this_thread(limit: Option<usize>) {
    THREAD_LIMIT.with(|cell| cell.set(limit));
}

/// How many threads the machine can run at once, capped at [`MAX_BANDS`].
fn workers() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_BANDS)
    })
}

/// Runs `body` over `rows` rows of output, in bands, possibly in parallel.
///
/// `out` is the whole output: `rows` rows of `row_bytes` each. `body` is handed a range of
/// rows and exactly that range's bytes of `out`, and must write those bytes and no others -
/// which the borrow checker enforces, since the slices are disjoint. `texels_per_row` is how
/// much work one row is, and decides how many bands are worth cutting.
///
/// The result does not depend on how many bands were cut: every row is computed by the same
/// call to `body` it would have been computed by serially.
///
/// # Panics
///
/// Panics if `out` is not `rows * row_bytes` long, or if `body` panics on any thread.
pub fn for_each_band<F>(rows: u32, row_bytes: usize, texels_per_row: usize, out: &mut [u8], body: F)
where
    F: Fn(Range<u32>, &mut [u8]) + Sync,
{
    assert_eq!(
        out.len(),
        rows as usize * row_bytes,
        "the output is every row's bytes"
    );
    let min_rows = MIN_BAND_TEXELS.div_ceil(texels_per_row.max(1)).max(1);
    let limit = THREAD_LIMIT
        .with(std::cell::Cell::get)
        .unwrap_or(usize::MAX);
    let bands = (rows as usize / min_rows).clamp(1, workers().min(limit.max(1)));
    if bands <= 1 || row_bytes == 0 {
        body(0..rows, out);
        return;
    }

    let rows_per_band = (rows as usize).div_ceil(bands);
    let body = &body;
    std::thread::scope(|scope| {
        let mut pieces = out.chunks_mut(rows_per_band * row_bytes).enumerate();
        // The calling thread takes the first band rather than waiting idle for the others.
        let first = pieces.next();
        for (index, piece) in pieces {
            let start = (index * rows_per_band) as u32;
            let end = (start + rows_per_band as u32).min(rows);
            scope.spawn(move || body(start..end, piece));
        }
        if let Some((_, piece)) = first {
            body(0..(rows_per_band as u32).min(rows), piece);
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Each row is filled with its own index, so a row written by the wrong band, twice or
    /// not at all is visible in the result.
    fn fill(rows: Range<u32>, out: &mut [u8], row_bytes: usize) {
        for (row, bytes) in rows.zip(out.chunks_exact_mut(row_bytes)) {
            bytes.fill(row as u8);
        }
    }

    #[test]
    fn every_row_is_written_once_by_the_band_it_belongs_to() {
        for (rows, row_bytes, texels) in [
            (1_u32, 4_usize, 1_usize),
            (7, 3, 1),
            (1000, 64, 4096),
            (513, 16, 100_000),
        ] {
            let mut out = vec![0xff_u8; rows as usize * row_bytes];
            for_each_band(rows, row_bytes, texels, &mut out, |range, bytes| {
                fill(range, bytes, row_bytes);
            });
            for (row, bytes) in out.chunks_exact(row_bytes).enumerate() {
                assert!(
                    bytes.iter().all(|byte| *byte == row as u8),
                    "row {row} of {rows}"
                );
            }
        }
    }

    #[test]
    fn small_work_stays_on_the_calling_thread() {
        let caller = std::thread::current().id();
        let mut out = vec![0_u8; 64];
        for_each_band(16, 4, 16, &mut out, |range, _| {
            assert_eq!(range, 0..16);
            assert_eq!(std::thread::current().id(), caller);
        });
    }
}
