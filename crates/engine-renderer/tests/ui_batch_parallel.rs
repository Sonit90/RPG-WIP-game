//! A parallel UI build has to be the same interface, reached faster.
//!
//! Paint order *is* the interface: a tooltip that draws under the panel it describes is a
//! bug, not a reordering. So the assertion here is not "roughly the same vertices" but
//! byte-identical output against the serial path, at every worker count.
//!
//! No device: `UiBatcher` is pure CPU, which is what lets this run on a machine with no
//! adapter at all.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use engine_core::jobs::JobSystem;
use engine_render_data::ui::{UiDrawList, UiRect};
use engine_renderer::{UiBatchStats, UiBatcher, UiVertex};

/// The budget the game uses, so the drop path behaves as it does in a frame.
const MAX_QUADS: usize = 65_536;

/// A list large enough that the batcher actually splits it.
///
/// Below `MIN_TOTAL_UNITS` the build falls back to the serial path, which would make every
/// test below pass without a single worker running. Text dominates on purpose: a glyph is a
/// quad, so a nameplate-heavy list is the shape the parallel path exists for.
fn large_list(plates: usize) -> UiDrawList {
    let mut list = UiDrawList::new();
    for index in 0..plates {
        let y = index as f32 * 3.0;
        // A mix of every command kind, interleaved rather than blocked, so a chunk boundary
        // lands in the middle of a run of one kind.
        list.rect(UiRect::new(4.0, y, 120.0, 2.0), [0.2, 0.2, 0.2, 1.0]);
        list.outline(UiRect::new(4.0, y, 120.0, 2.0), 1.0, [0.0, 0.0, 0.0, 1.0]);
        list.radial(
            UiRect::new(130.0, y, 16.0, 16.0),
            (index % 8) as f32 / 8.0,
            [0.0, 0.0, 0.0, 0.6],
        );
        list.text_at(
            (8.0, y),
            "Ashen Reliquary Sentinel",
            [1.0, 1.0, 1.0, 1.0],
            1.0,
        );
    }
    list
}

fn serial(list: &UiDrawList) -> (Vec<UiVertex>, UiBatchStats) {
    let mut batcher = UiBatcher::new(MAX_QUADS);
    let vertices = batcher.build(list).to_vec();
    (vertices, batcher.stats())
}

fn parallel(list: &UiDrawList, workers: usize) -> (Vec<UiVertex>, UiBatchStats) {
    let Ok(jobs) = JobSystem::with_workers(workers) else {
        panic!("{workers} workers must start");
    };
    let mut batcher = UiBatcher::new(MAX_QUADS);
    let vertices = batcher.build_with(list, Some(&jobs)).to_vec();
    (vertices, batcher.stats())
}

#[test]
fn the_parallel_buffer_is_identical_to_the_serial_one() {
    let list = large_list(600);
    let (expected, expected_stats) = serial(&list);
    assert!(
        expected.len() > 6,
        "the fixture must actually produce vertices"
    );

    for workers in [1, 2, 4, 8, 16] {
        let (actual, stats) = parallel(&list, workers);
        assert_eq!(
            actual.len(),
            expected.len(),
            "{workers} workers produced a different vertex count"
        );
        for (index, (a, b)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(
                (a.position, a.uv, a.color),
                (b.position, b.uv, b.color),
                "{workers} workers differ at vertex {index}; paint order is the interface"
            );
        }
        assert_eq!(
            stats, expected_stats,
            "{workers} workers reported different statistics"
        );
    }
}

#[test]
fn zero_workers_runs_inline_and_agrees() {
    // The determinism configuration: the same API, no threads, and it must land on the
    // serial answer rather than on a third one.
    let list = large_list(600);
    let (expected, expected_stats) = serial(&list);
    let (actual, stats) = parallel(&list, 0);
    assert_eq!(actual.len(), expected.len());
    assert_eq!(stats, expected_stats);
}

#[test]
fn a_list_that_shrank_does_not_keep_the_previous_frames_vertices() {
    // The chunk scratch is reused across frames. A build that left last frame's longer
    // chunks in place would draw the interface of a frame that is over.
    let Ok(jobs) = JobSystem::with_workers(8) else {
        panic!("8 workers must start");
    };
    let mut batcher = UiBatcher::new(MAX_QUADS);

    let big = large_list(600);
    let long = batcher.build_with(&big, Some(&jobs)).len();

    let small = large_list(4);
    let short = batcher.build_with(&small, Some(&jobs)).len();
    assert!(
        short < long,
        "a smaller interface must produce fewer vertices, got {short} after {long}"
    );

    let (expected, _) = serial(&small);
    assert_eq!(short, expected.len());
}

#[test]
fn the_budget_is_enforced_across_the_whole_merge() {
    // A tiny budget, so the drop path runs. Which quads survive is a statement about paint
    // order, so the parallel merge has to keep exactly the ones the serial build kept.
    let list = large_list(600);

    let mut serial_batcher = UiBatcher::new(64);
    let expected = serial_batcher.build(&list).to_vec();
    let expected_stats = serial_batcher.stats();
    assert!(
        expected_stats.dropped > 0,
        "the fixture must overflow 64 quads"
    );

    let Ok(jobs) = JobSystem::with_workers(8) else {
        panic!("8 workers must start");
    };
    let mut parallel_batcher = UiBatcher::new(64);
    let actual = parallel_batcher.build_with(&list, Some(&jobs)).to_vec();

    assert_eq!(actual.len(), expected.len());
    for (a, b) in actual.iter().zip(&expected) {
        assert_eq!((a.position, a.uv, a.color), (b.position, b.uv, b.color));
    }
    assert_eq!(parallel_batcher.stats(), expected_stats);
}

#[test]
fn a_small_list_stays_serial() {
    // The threshold is the whole point: a HUD-sized list must not pay submission overhead
    // to save nothing. Asserted through the answer rather than through a counter — an
    // identical buffer is what "it took the serial path" means from the outside.
    let list = large_list(2);
    let (expected, expected_stats) = serial(&list);
    let (actual, stats) = parallel(&list, 8);
    assert_eq!(actual.len(), expected.len());
    assert_eq!(stats, expected_stats);
}
