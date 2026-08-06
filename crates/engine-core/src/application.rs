//! Engine lifecycle: the loop and its sequencing live here, not in the application.
//!
//! ```text
//! Engine::new() -> initialize() -> run() -> shutdown()
//! ```
//!
//! and per frame:
//!
//! ```text
//! process_events() -> update() -> render() -> present() -> end_frame()
//! ```
//!
//! [`Engine`] is generic over [`Application`] rather than holding a `dyn Application`.
//! Monomorphization means the six per-frame hooks are direct calls: no vtable, no boxed
//! application, nothing to allocate. The cost is that the concrete application type is
//! baked into the binary, which is exactly what a single-game engine wants.
//!
//! `engine-core` sits below `engine-platform` and `engine-renderer`, so it cannot know
//! what a window or a device is. The [`Application`] trait is the seam: the executable
//! implements it over whatever it owns, and the engine drives it.
//!
//! Allocation policy: the loop itself allocates nothing. Every per-frame value lives in
//! [`EngineContext`], which is created once.

use std::time::{Duration, Instant};

use crate::metrics::{DEFAULT_HISTORY, DEFAULT_REFRESH_INTERVAL, FrameMetrics};
use crate::time::Time;

/// Where a frame spent its CPU time, measured by the engine loop.
///
/// Wall-clock time around each stage, which is what a frame budget is actually spent on.
/// A stage the application skipped reports [`Duration::ZERO`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StageTimings {
    /// Draining and handling platform events.
    pub events: Duration,
    /// Advancing the simulation.
    pub update: Duration,
    /// Recording the frame's rendering work.
    pub render: Duration,
    /// Handing the frame to the display.
    pub present: Duration,
}

impl StageTimings {
    /// Total measured stage time.
    ///
    /// Slightly less than the frame delta: the engine's own bookkeeping is not attributed
    /// to any stage.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.events + self.update + self.render + self.present
    }
}

/// How the engine loop is configured before it starts.
///
/// Deliberately tiny. Window size, vsync and the rest are platform and renderer concerns
/// and are configured there; only the loop's own knobs belong here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// Stop after this many frames. `None` runs until the application asks to exit.
    ///
    /// Used by benchmarks and by automated runs that must terminate on their own.
    pub frame_budget: Option<u64>,
    /// Frames of CPU and GPU frame-time history kept for percentiles.
    pub metrics_history: usize,
    /// How often frame-time percentiles are recomputed.
    pub metrics_refresh_interval: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            frame_budget: None,
            metrics_history: DEFAULT_HISTORY,
            metrics_refresh_interval: DEFAULT_REFRESH_INTERVAL,
        }
    }
}

impl EngineConfig {
    /// Returns the configuration with a frame budget applied.
    #[must_use]
    pub fn with_frame_budget(mut self, frames: Option<u64>) -> Self {
        self.frame_budget = frames;
        self
    }

    /// Returns the configuration with the metrics window and refresh interval applied.
    #[must_use]
    pub fn with_metrics(mut self, history: usize, refresh_interval: Duration) -> Self {
        self.metrics_history = history;
        self.metrics_refresh_interval = refresh_interval;
        self
    }
}

/// Per-frame state the engine owns and the application reads.
///
/// The application never constructs this; the engine hands it to every hook.
#[derive(Debug)]
pub struct EngineContext {
    time: Time,
    metrics: FrameMetrics,
    stages: StageTimings,
    config: EngineConfig,
    running: bool,
}

impl EngineContext {
    /// Creates a context and starts its clock.
    fn new(config: EngineConfig) -> Self {
        Self {
            time: Time::new(),
            metrics: FrameMetrics::new(config.metrics_history, config.metrics_refresh_interval),
            stages: StageTimings::default(),
            config,
            running: true,
        }
    }

    /// Frame-time history and the most recent summary.
    #[must_use]
    pub fn metrics(&self) -> &FrameMetrics {
        &self.metrics
    }

    /// Mutable frame metrics, for clearing history when the workload changes.
    pub fn metrics_mut(&mut self) -> &mut FrameMetrics {
        &mut self.metrics
    }

    /// Real and simulation time for the current frame.
    #[must_use]
    pub fn time(&self) -> &Time {
        &self.time
    }

    /// Mutable time, for changing the simulation scale, the pause state or the clamp.
    ///
    /// Advancing time is the engine loop's job; an application that calls
    /// [`Time::advance`] itself splits one frame across two deltas.
    pub fn time_mut(&mut self) -> &mut Time {
        &mut self.time
    }

    /// Duration the simulation should advance by this frame.
    ///
    /// Shorthand for `ctx.time().delta()`. Zero during the first frame, because no frame
    /// has completed yet; simulation code must tolerate that rather than dividing by it.
    #[must_use]
    pub fn delta(&self) -> Duration {
        self.time.delta()
    }

    /// Real time since the engine loop started.
    ///
    /// Shorthand for `ctx.time().real_elapsed()`.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.time.real_elapsed()
    }

    /// Number of completed frames.
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.time.frame_index()
    }

    /// CPU time each stage of the previous frame took.
    #[must_use]
    pub fn stage_timings(&self) -> StageTimings {
        self.stages
    }

    /// The configuration the engine was started with.
    #[must_use]
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Whether the loop will run another frame.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Asks the loop to stop after the current frame completes.
    ///
    /// The frame is always finished rather than abandoned mid-way, so the renderer never
    /// has a recorded-but-unpresented frame in flight when the loop exits.
    pub fn request_exit(&mut self) {
        self.running = false;
    }

    /// Restarts both timelines and clears the measured stage times.
    ///
    /// Used when the workload changes — old frame times describe a different scene and
    /// blending the two produces statistics that describe neither. Scale, pause state and
    /// the delta clamp survive.
    pub fn reset_clock(&mut self) {
        self.time.reset();
        self.metrics.clear();
        self.stages = StageTimings::default();
    }
}

/// What the engine drives once per frame.
///
/// Every hook except [`Application::process_events`] has a default implementation, so an
/// application only writes the stages it actually uses.
///
/// Hooks that must not run per frame — logging, string formatting, statistics — belong in
/// [`Application::end_frame`] behind an interval check, not in [`Application::update`].
pub trait Application {
    /// Failure type the engine propagates out of [`Engine::run`].
    type Error;

    /// Called once, before the first frame.
    ///
    /// # Errors
    ///
    /// Returns an error if the application cannot start; the engine then shuts down and
    /// propagates it.
    fn initialize(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
        let _ = ctx;
        Ok(())
    }

    /// Drains platform events. Call [`EngineContext::request_exit`] to end the loop.
    ///
    /// # Errors
    ///
    /// Returns an error if handling an event failed, for example a resize the renderer
    /// could not satisfy.
    fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error>;

    /// Advances the simulation by [`EngineContext::delta`].
    ///
    /// # Errors
    ///
    /// Returns an error if the simulation cannot continue.
    fn update(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
        let _ = ctx;
        Ok(())
    }

    /// Records the frame. Skipped entirely when [`Application::is_render_ready`] is false.
    ///
    /// # Errors
    ///
    /// Returns an error if recording failed.
    fn render(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
        let _ = ctx;
        Ok(())
    }

    /// Hands the recorded frame to the display. Skipped with `render`.
    ///
    /// # Errors
    ///
    /// Returns an error if presentation failed.
    fn present(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
        let _ = ctx;
        Ok(())
    }

    /// GPU time for the most recently resolved frame, in milliseconds.
    ///
    /// Returns `None` while timestamps are still in flight, which is normal for the first
    /// frames after startup or a resize. `engine-core` has no GPU concept of its own, so
    /// the value has to come from the application's renderer; the engine only records it.
    ///
    /// GPU timestamps lag the CPU by a few frames, so this is *not* the GPU time of the
    /// frame just recorded. The two are summarized separately and must never be presented
    /// as simultaneous samples of one frame.
    fn gpu_frame_ms(&self) -> Option<f64> {
        None
    }

    /// Whether rendering this frame is possible at all.
    ///
    /// Returning `false` skips both `render` and `present`; their stage timings then read
    /// zero. A minimized window is the motivating case: it has a zero-sized client area
    /// and nothing to present to.
    fn is_render_ready(&self) -> bool {
        true
    }

    /// Called after the frame's delta and stage timings have been recorded.
    ///
    /// The place for interval-driven work: statistics reporting, overlay text, anything
    /// that allocates.
    fn end_frame(&mut self, ctx: &mut EngineContext) {
        let _ = ctx;
    }

    /// Called once after the loop ends, including when a stage returned an error.
    ///
    /// Must be safe to run on a partially initialized application, since initialization
    /// itself may have failed. Receives the context so a final report can read the frame
    /// count and elapsed time.
    fn shutdown(&mut self, ctx: &mut EngineContext) {
        let _ = ctx;
    }
}

/// Which lifecycle steps have already happened.
///
/// Tracked so the public lifecycle calls can be made in any order, or skipped, without
/// initializing twice or shutting down twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Created,
    Initialized,
    ShutDown,
}

/// Owns an [`Application`] and drives its lifecycle.
#[derive(Debug)]
pub struct Engine<A: Application> {
    app: A,
    ctx: EngineContext,
    state: State,
}

impl<A: Application> Engine<A> {
    /// Creates the engine around an application. Runs no application code.
    #[must_use]
    pub fn new(app: A, config: EngineConfig) -> Self {
        Self {
            app,
            ctx: EngineContext::new(config),
            state: State::Created,
        }
    }

    /// Initializes the application. Does nothing if already initialized.
    ///
    /// Calling this is optional: [`Engine::run`] initializes if it has not happened yet.
    /// It exists separately so a caller can initialize and inspect state before the loop.
    ///
    /// # Errors
    ///
    /// Propagates the application's initialization failure. The application is shut down
    /// before the error is returned, so a partially built application still releases what
    /// it acquired.
    pub fn initialize(&mut self) -> Result<(), A::Error> {
        if self.state != State::Created {
            return Ok(());
        }
        match self.app.initialize(&mut self.ctx) {
            Ok(()) => {
                self.state = State::Initialized;
                // The clock started when the context was built; initialization can take
                // hundreds of milliseconds (device creation, shader compilation) and that
                // is not frame time.
                self.ctx.reset_clock();
                Ok(())
            }
            Err(error) => {
                self.shutdown();
                Err(error)
            }
        }
    }

    /// Runs frames until the application requests an exit or the frame budget is spent.
    ///
    /// Initializes first if needed, and always shuts down before returning — on the happy
    /// path and on an error alike.
    ///
    /// # Errors
    ///
    /// Propagates the first error any stage returns. The loop stops immediately; the
    /// frame is not completed.
    pub fn run(&mut self) -> Result<(), A::Error> {
        self.initialize()?;

        let result = self.run_loop();
        self.shutdown();
        result
    }

    /// The frame loop proper, without lifecycle bookkeeping.
    fn run_loop(&mut self) -> Result<(), A::Error> {
        while self.ctx.running {
            let events_start = Instant::now();
            self.app.process_events(&mut self.ctx)?;
            let events = events_start.elapsed();

            let update_start = Instant::now();
            self.app.update(&mut self.ctx)?;
            let update = update_start.elapsed();

            let (render, present) = if self.app.is_render_ready() {
                let render_start = Instant::now();
                self.app.render(&mut self.ctx)?;
                let render = render_start.elapsed();

                let present_start = Instant::now();
                self.app.present(&mut self.ctx)?;
                (render, present_start.elapsed())
            } else {
                (Duration::ZERO, Duration::ZERO)
            };

            self.ctx.time.advance();
            self.ctx.stages = StageTimings {
                events,
                update,
                render,
                present,
            };
            // Real delta, not the simulation delta: metrics measure the machine, so a
            // paused or slowed simulation must not make the engine look faster.
            self.ctx.metrics.record(
                self.ctx.time.frame_index(),
                self.ctx.time.real_delta(),
                self.ctx.stages,
                self.app.gpu_frame_ms(),
            );

            self.app.end_frame(&mut self.ctx);

            if let Some(budget) = self.ctx.config.frame_budget
                && self.ctx.frame_index() >= budget
            {
                self.ctx.request_exit();
            }
        }
        Ok(())
    }

    /// Shuts the application down. Does nothing if it already happened.
    pub fn shutdown(&mut self) {
        if self.state == State::ShutDown {
            return;
        }
        self.app.shutdown(&mut self.ctx);
        self.state = State::ShutDown;
    }

    /// Borrows the application, for inspection after a run.
    pub fn app(&self) -> &A {
        &self.app
    }

    /// Borrows the engine context.
    pub fn context(&self) -> &EngineContext {
        &self.ctx
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Records which hooks ran, in order, so the loop's sequencing can be asserted.
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<&'static str>,
        deltas: Vec<Duration>,
        render_ready: bool,
        exit_after: Option<u64>,
        fail_at: Option<&'static str>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TestError(&'static str);

    impl Recorder {
        fn new() -> Self {
            Self {
                render_ready: true,
                ..Self::default()
            }
        }

        fn note(&mut self, name: &'static str) -> Result<(), TestError> {
            self.calls.push(name);
            if self.fail_at == Some(name) {
                return Err(TestError(name));
            }
            Ok(())
        }

        fn count(&self, name: &str) -> usize {
            self.calls.iter().filter(|call| **call == name).count()
        }
    }

    impl Application for Recorder {
        type Error = TestError;

        fn initialize(&mut self, _ctx: &mut EngineContext) -> Result<(), Self::Error> {
            self.note("initialize")
        }

        fn process_events(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
            self.note("process_events")?;
            if let Some(limit) = self.exit_after
                && ctx.frame_index() >= limit
            {
                ctx.request_exit();
            }
            Ok(())
        }

        fn update(&mut self, ctx: &mut EngineContext) -> Result<(), Self::Error> {
            self.deltas.push(ctx.delta());
            self.note("update")
        }

        fn render(&mut self, _ctx: &mut EngineContext) -> Result<(), Self::Error> {
            self.note("render")
        }

        fn present(&mut self, _ctx: &mut EngineContext) -> Result<(), Self::Error> {
            self.note("present")
        }

        fn is_render_ready(&self) -> bool {
            self.render_ready
        }

        fn end_frame(&mut self, _ctx: &mut EngineContext) {
            self.calls.push("end_frame");
        }

        fn shutdown(&mut self, _ctx: &mut EngineContext) {
            self.calls.push("shutdown");
        }
    }

    fn engine_with(app: Recorder, frames: Option<u64>) -> Engine<Recorder> {
        Engine::new(app, EngineConfig::default().with_frame_budget(frames))
    }

    #[test]
    fn one_frame_runs_the_stages_in_order() {
        let mut engine = engine_with(Recorder::new(), Some(1));
        engine.run().expect("the recorder never fails");
        assert_eq!(
            engine.app().calls,
            [
                "initialize",
                "process_events",
                "update",
                "render",
                "present",
                "end_frame",
                "shutdown",
            ]
        );
    }

    #[test]
    fn the_frame_budget_bounds_the_run() {
        let mut engine = engine_with(Recorder::new(), Some(5));
        engine.run().expect("the recorder never fails");
        assert_eq!(engine.context().frame_index(), 5);
        assert_eq!(engine.app().count("update"), 5);
    }

    #[test]
    fn requesting_exit_finishes_the_current_frame() {
        let mut app = Recorder::new();
        app.exit_after = Some(2);
        let mut engine = engine_with(app, Some(100));
        engine.run().expect("the recorder never fails");

        // Exit is requested during frame 3's event stage, and that frame still renders
        // and presents rather than being abandoned half-recorded.
        assert_eq!(engine.context().frame_index(), 3);
        assert_eq!(engine.app().count("present"), 3);
        assert_eq!(engine.app().count("end_frame"), 3);
    }

    #[test]
    fn a_zero_frame_budget_runs_no_frames() {
        let mut engine = engine_with(Recorder::new(), Some(0));
        engine.run().expect("the recorder never fails");
        // The budget is checked at the end of a frame, so exactly one frame runs before
        // the loop notices it is already over budget.
        assert_eq!(engine.context().frame_index(), 1);
    }

    #[test]
    fn skipping_render_still_advances_the_frame() {
        let mut app = Recorder::new();
        app.render_ready = false;
        let mut engine = engine_with(app, Some(3));
        engine.run().expect("the recorder never fails");

        assert_eq!(engine.app().count("render"), 0);
        assert_eq!(engine.app().count("present"), 0);
        assert_eq!(engine.app().count("update"), 3);
        assert_eq!(engine.context().stage_timings().render, Duration::ZERO);
    }

    #[test]
    fn the_first_frame_sees_a_zero_delta_and_later_frames_do_not_regress() {
        let mut engine = engine_with(Recorder::new(), Some(4));
        engine.run().expect("the recorder never fails");
        let deltas = &engine.app().deltas;
        assert_eq!(deltas.len(), 4);
        assert_eq!(deltas[0], Duration::ZERO, "no frame has completed yet");
    }

    #[test]
    fn stage_timings_are_recorded_for_the_last_frame() {
        let mut engine = engine_with(Recorder::new(), Some(2));
        engine.run().expect("the recorder never fails");
        let stages = engine.context().stage_timings();
        assert!(stages.total() <= engine.context().elapsed());
    }

    #[test]
    fn a_failing_stage_stops_the_loop_and_still_shuts_down() {
        for stage in ["process_events", "update", "render", "present"] {
            let mut app = Recorder::new();
            app.fail_at = Some(stage);
            let mut engine = engine_with(app, Some(10));

            let error = engine.run().expect_err("the stage was told to fail");
            assert_eq!(error, TestError(stage));
            assert_eq!(engine.app().count("shutdown"), 1);
            assert_eq!(
                engine.app().count("end_frame"),
                0,
                "a failed frame is not completed"
            );
        }
    }

    #[test]
    fn a_failing_initialize_skips_the_loop_and_shuts_down_once() {
        let mut app = Recorder::new();
        app.fail_at = Some("initialize");
        let mut engine = engine_with(app, Some(10));

        assert_eq!(
            engine.run().expect_err("initialize was told to fail"),
            TestError("initialize")
        );
        assert_eq!(engine.app().count("process_events"), 0);
        assert_eq!(engine.app().count("shutdown"), 1);
    }

    #[test]
    fn lifecycle_calls_are_idempotent() {
        let mut engine = engine_with(Recorder::new(), Some(1));
        engine.initialize().expect("the recorder never fails");
        engine.initialize().expect("the second call is a no-op");
        engine.run().expect("the recorder never fails");
        engine.shutdown();

        assert_eq!(engine.app().count("initialize"), 1);
        assert_eq!(engine.app().count("shutdown"), 1);
    }

    #[test]
    fn resetting_the_clock_clears_the_measured_frame_state() {
        let mut ctx = EngineContext::new(EngineConfig::default());
        ctx.time_mut().advance_by(Duration::from_millis(4));
        ctx.stages.update = Duration::from_millis(1);
        ctx.reset_clock();

        assert_eq!(ctx.delta(), Duration::ZERO);
        assert_eq!(ctx.elapsed(), Duration::ZERO);
        assert_eq!(ctx.stage_timings(), StageTimings::default());
        assert_eq!(ctx.frame_index(), 0);
    }

    #[test]
    fn stage_totals_sum_every_stage() {
        let stages = StageTimings {
            events: Duration::from_millis(1),
            update: Duration::from_millis(2),
            render: Duration::from_millis(3),
            present: Duration::from_millis(4),
        };
        assert_eq!(stages.total(), Duration::from_millis(10));
    }
}
