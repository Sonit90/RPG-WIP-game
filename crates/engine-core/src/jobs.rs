//! A minimal job system: submit work, wait for it, shut down cleanly.
//!
//! ## Design
//!
//! One shared queue behind a mutex, a condition variable to wake workers, and a completion
//! counter per submission. That is the simplest design that is correct, and it is where
//! this starts.
//!
//! Work stealing, lock-free queues, fibers and dependency graphs are all deliberately
//! absent. Each of them costs real complexity and each is justified by a measurement that
//! has not been taken yet. `docs/threading.md` records what the current design costs, so
//! the case for replacing it can be made from numbers.
//!
//! ## What it is good for
//!
//! Jobs that are large enough to pay for their submission. The benchmark in
//! `engine-bench` measures where that line is on this machine; below it, sequential
//! execution wins and the job system should not be used.
//!
//! ## Deadlock
//!
//! A job that submits another job and waits for it can deadlock: if every worker is
//! blocked waiting, nobody is left to run the work. Jobs must not wait on other jobs. This
//! is a real limitation of the simple design and the reason a fiber scheduler exists in
//! engines that need nested parallelism.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::error::EngineError;
use crate::threading::{ShutdownSignal, Worker, recommended_worker_count};

/// A unit of work. Boxed because jobs of different types share one queue.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// Completion state shared between a [`JobHandle`] and the workers running its jobs.
#[derive(Debug)]
struct Completion {
    /// Jobs not yet finished. Reaching zero wakes every waiter.
    remaining: Mutex<usize>,
    finished: Condvar,
    /// Set if any job in this submission panicked.
    panicked: AtomicBool,
}

impl Completion {
    fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            remaining: Mutex::new(count),
            finished: Condvar::new(),
            panicked: AtomicBool::new(false),
        })
    }

    /// Marks one job finished, waking waiters when the last one lands.
    fn complete_one(&self) {
        let mut remaining = match self.remaining.lock() {
            Ok(guard) => guard,
            // A poisoned mutex means a panic happened while it was held. The counter is a
            // plain integer, so its value is still meaningful and progress matters more
            // than the poison flag: refusing to decrement would hang every waiter.
            Err(poisoned) => poisoned.into_inner(),
        };
        *remaining = remaining.saturating_sub(1);
        if *remaining == 0 {
            self.finished.notify_all();
        }
    }
}

/// A receipt for submitted work.
///
/// Cloning it is cheap and gives another view of the same submission, so several places
/// can wait for the same work.
#[derive(Debug, Clone)]
pub struct JobHandle {
    completion: Arc<Completion>,
}

impl JobHandle {
    /// A handle for work that is already finished.
    ///
    /// Returned when a batch is submitted with no jobs in it, so a caller never has to
    /// special-case "nothing to do".
    #[must_use]
    pub fn completed() -> Self {
        Self {
            completion: Completion::new(0),
        }
    }

    /// Blocks until every job in this submission has finished.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] in the `InvalidState` category if any job panicked. The
    /// work is finished either way — the error reports what happened, it does not mean
    /// something is still running.
    pub fn wait(&self) -> Result<(), EngineError> {
        let mut remaining = match self.completion.remaining.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        while *remaining > 0 {
            remaining = match self.completion.finished.wait(remaining) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
        drop(remaining);

        if self.completion.panicked.load(Ordering::Acquire) {
            return Err(EngineError::invalid_state("a job panicked"));
        }
        Ok(())
    }

    /// Whether every job in this submission has finished. Never blocks.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        let remaining = match self.completion.remaining.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *remaining == 0
    }
}

impl std::fmt::Debug for JobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JobEntry")
    }
}

/// A queued job together with the submission it belongs to.
struct JobEntry {
    job: Job,
    completion: Arc<Completion>,
}

/// A pool of worker threads served by one shared queue.
///
/// Dropping the system shuts it down: workers are asked to stop, woken, and joined.
/// Jobs still queued at that point are dropped without running.
#[derive(Debug)]
pub struct JobSystem {
    queue: Arc<SharedQueue>,
    workers: Vec<Worker>,
    /// Jobs submitted since construction, for reporting.
    submitted: Arc<AtomicU64>,
}

/// A boxed job whose captures may borrow.
///
/// The queue stores `'static` jobs, because a job submitted through [`JobSystem::submit`]
/// can outlive its submitter. [`JobSystem::map_chunks`] cannot outlive its submitter — it
/// waits — so it builds jobs of this type and launders the lifetime in exactly one place.
type BorrowedJob<'a> = Box<dyn FnOnce() + Send + 'a>;

/// The queue plus its condition variable, shared with every worker.
#[derive(Debug, Default)]
struct SharedQueue {
    entries: Mutex<VecDeque<JobEntry>>,
    available: Condvar,
}

impl SharedQueue {
    /// Takes the next job, blocking until one exists or shutdown is requested.
    fn pop_blocking(&self, shutdown: &ShutdownSignal) -> Option<JobEntry> {
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        loop {
            if let Some(entry) = entries.pop_front() {
                return Some(entry);
            }
            if shutdown.is_set() {
                return None;
            }
            entries = match self.available.wait(entries) {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }
}

impl JobSystem {
    /// Starts a job system with the recommended number of workers.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] if a worker thread cannot be created.
    pub fn new() -> Result<Self, EngineError> {
        Self::with_workers(recommended_worker_count())
    }

    /// Starts a job system with exactly `worker_count` workers.
    ///
    /// Zero workers is legal and useful: submitted jobs then run on the calling thread at
    /// submission time, so a single-threaded build or a determinism test can use the same
    /// API without a branch at every call site.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] if a worker thread cannot be created. Workers started
    /// before the failure are shut down before returning.
    pub fn with_workers(worker_count: usize) -> Result<Self, EngineError> {
        let queue = Arc::new(SharedQueue::default());
        let mut workers = Vec::with_capacity(worker_count);

        for index in 0..worker_count {
            let worker_queue = Arc::clone(&queue);
            let worker = Worker::spawn(format!("job-worker-{index}"), move |shutdown| {
                while let Some(entry) = worker_queue.pop_blocking(&shutdown) {
                    run_entry(entry);
                }
            });

            match worker {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    // Dropping the workers built so far stops and joins them, so a partial
                    // pool never survives a failed construction.
                    for worker in &workers {
                        worker.request_shutdown();
                    }
                    queue.available.notify_all();
                    drop(workers);
                    return Err(error.context("start the job system"));
                }
            }
        }

        log::info!(target: "Jobs", "job system started with {worker_count} workers");

        Ok(Self {
            queue,
            workers,
            submitted: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Number of worker threads.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Jobs submitted since the system started.
    #[must_use]
    pub fn submitted_count(&self) -> u64 {
        self.submitted.load(Ordering::Relaxed)
    }

    /// Submits one job and returns a handle for it.
    ///
    /// With no workers the job runs here, before this returns.
    pub fn submit<F>(&self, job: F) -> JobHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let completion = Completion::new(1);
        self.enqueue(
            JobEntry {
                job: Box::new(job),
                completion: Arc::clone(&completion),
            },
            1,
        );
        JobHandle { completion }
    }

    /// Submits several jobs under one handle.
    ///
    /// Waiting once for a batch is both cheaper and clearer than waiting for each job:
    /// one counter, one wake-up when the last job lands.
    pub fn submit_batch<I, F>(&self, jobs: I) -> JobHandle
    where
        I: IntoIterator<Item = F>,
        F: FnOnce() + Send + 'static,
    {
        let jobs: Vec<F> = jobs.into_iter().collect();
        if jobs.is_empty() {
            return JobHandle::completed();
        }

        let completion = Completion::new(jobs.len());
        let count = jobs.len();

        if self.workers.is_empty() {
            for job in jobs {
                run_entry(JobEntry {
                    job: Box::new(job),
                    completion: Arc::clone(&completion),
                });
            }
            self.submitted.fetch_add(count as u64, Ordering::Relaxed);
            return JobHandle { completion };
        }

        {
            let mut entries = match self.queue.entries.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            entries.reserve(count);
            for job in jobs {
                entries.push_back(JobEntry {
                    job: Box::new(job),
                    completion: Arc::clone(&completion),
                });
            }
        }
        // Notified outside the lock: a worker woken while the queue is still locked would
        // immediately block on it again.
        self.queue.available.notify_all();
        self.submitted.fetch_add(count as u64, Ordering::Relaxed);

        JobHandle { completion }
    }

    /// Runs `body` once per element of `outputs`, in parallel, and waits for all of them.
    ///
    /// This is the chunked-work primitive `docs/threading.md` says to parallelize with: one
    /// job per chunk, each writing into its own output slot, never one job per item. The
    /// caller sizes the chunks; this only runs them.
    ///
    /// ```text
    /// outputs: [ A ][ B ][ C ][ D ]      one &mut per job, disjoint
    ///             \    |    |    /
    ///              body(i, &mut outputs[i])   in parallel
    ///                    |
    ///              all complete before this returns
    /// ```
    ///
    /// Unlike [`JobSystem::submit`], `body` and `outputs` may borrow: nothing outlives the
    /// call, because the wait below is unconditional. That is what makes it usable for work
    /// that reads a `&RenderWorld` and writes a per-chunk result — which `submit`'s
    /// `'static` bound makes impossible.
    ///
    /// With no workers, every chunk runs here on the calling thread, in order. That is the
    /// determinism configuration, and it exercises the same code path.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] if any chunk panicked. Every chunk is still waited on, so
    /// no job is running when this returns whether it succeeded or not.
    ///
    /// # Panics
    ///
    /// Does not panic itself; a panicking `body` is caught per chunk and reported.
    pub fn map_chunks<T, F>(&self, outputs: &mut [T], body: F) -> Result<(), EngineError>
    where
        T: Send,
        F: Fn(usize, &mut T) + Sync,
    {
        if outputs.is_empty() {
            return Ok(());
        }

        // Sequential path: no workers, no pointers, no unsafe. Also the path a determinism
        // test takes, so the parallel one has something to be compared against.
        if self.workers.is_empty() {
            for (index, output) in outputs.iter_mut().enumerate() {
                body(index, output);
            }
            self.submitted
                .fetch_add(outputs.len() as u64, Ordering::Relaxed);
            return Ok(());
        }

        // Disjointness is the borrow checker's job, not a comment's: `iter_mut` hands each
        // closure a distinct `&mut T` and no two can name the same element. What the
        // compiler cannot see is that these jobs do not outlive this call, which is the one
        // thing the transmute below asserts.
        let body = &body;
        let jobs: Vec<BorrowedJob<'_>> = outputs
            .iter_mut()
            .enumerate()
            .map(|(index, output)| {
                let job: BorrowedJob<'_> = Box::new(move || body(index, output));
                job
            })
            .collect();

        let jobs: Vec<Box<dyn FnOnce() + Send + 'static>> = jobs
            .into_iter()
            .map(|job| {
                // SAFETY: the only thing changing is the lifetime. Every job is enqueued
                // below and waited for before this function returns — the wait is
                // unconditional and its result is the return value, so there is no path out
                // of here that leaves a job running. The borrows the job holds therefore
                // cannot outlive the borrows this call itself holds, which is precisely what
                // `'static` would otherwise be needed to prove.
                //
                // This is the standard scoped-pool laundering. It is confined to this one
                // expression, and `map_chunks` is the only thing that performs it; every
                // other submission path really is `'static`.
                unsafe {
                    std::mem::transmute::<BorrowedJob<'_>, Box<dyn FnOnce() + Send + 'static>>(job)
                }
            })
            .collect();

        self.submit_batch(jobs).wait()
    }

    /// Waits for several submissions.
    ///
    /// # Errors
    ///
    /// Returns the first error any handle reports; every handle is waited on regardless,
    /// so no work is left running when this returns.
    pub fn wait_all(handles: &[JobHandle]) -> Result<(), EngineError> {
        let mut first_error = None;
        for handle in handles {
            if let Err(error) = handle.wait()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Queues one entry and wakes a worker, or runs it here if there are no workers.
    fn enqueue(&self, entry: JobEntry, count: u64) {
        self.submitted.fetch_add(count, Ordering::Relaxed);

        if self.workers.is_empty() {
            run_entry(entry);
            return;
        }

        {
            let mut entries = match self.queue.entries.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            entries.push_back(entry);
        }
        // One job, one worker: `notify_one` avoids waking the whole pool to fight over it.
        self.queue.available.notify_one();
    }

    /// Stops the workers and waits for them.
    ///
    /// Jobs still queued are dropped without running. Called by `Drop`; call it directly
    /// only to observe worker failures.
    ///
    /// # Errors
    ///
    /// Returns an [`EngineError`] if a worker panicked.
    pub fn shutdown(mut self) -> Result<(), EngineError> {
        self.stop_workers();
        let workers = std::mem::take(&mut self.workers);
        let mut first_error = None;
        for worker in workers {
            if let Err(error) = worker.join()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Signals every worker and wakes them all so they observe it.
    fn stop_workers(&self) {
        for worker in &self.workers {
            worker.request_shutdown();
        }
        // Workers parked on the condition variable never look at the flag until woken.
        self.queue.available.notify_all();
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        self.stop_workers();
        // The workers' own Drop joins them; this only has to make sure they can wake up.
    }
}

/// Runs one job, keeping a panic inside the worker.
///
/// A panicking job must not kill the worker: the pool would silently lose a thread, and
/// every later submission would be slower for a reason nothing reports. The panic is
/// recorded on the submission and surfaced by [`JobHandle::wait`].
fn run_entry(entry: JobEntry) {
    let JobEntry { job, completion } = entry;
    // AssertUnwindSafe: the job owns whatever it captured, and the only state shared with
    // this function is the completion counter, which is updated after the unwind either
    // way. Nothing here observes a half-mutated value.
    if catch_unwind(AssertUnwindSafe(job)).is_err() {
        completion.panicked.store(true, Ordering::Release);
        log::error!(target: "Jobs", "a job panicked; the worker continues");
    }
    completion.complete_one();
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    fn counter() -> Arc<AtomicU32> {
        Arc::new(AtomicU32::new(0))
    }

    #[test]
    fn a_submitted_job_runs_and_the_handle_reports_completion() {
        let jobs = JobSystem::with_workers(2).expect("the pool must start");
        let ran = counter();
        let flag = Arc::clone(&ran);

        let handle = jobs.submit(move || {
            flag.fetch_add(1, Ordering::Relaxed);
        });
        handle.wait().expect("the job must not panic");

        assert_eq!(ran.load(Ordering::Relaxed), 1);
        assert!(handle.is_complete());
        assert_eq!(jobs.submitted_count(), 1);
    }

    #[test]
    fn a_batch_runs_every_job_exactly_once() {
        let jobs = JobSystem::with_workers(4).expect("the pool must start");
        let ran = counter();

        let handle = jobs.submit_batch((0..1000).map(|_| {
            let flag = Arc::clone(&ran);
            move || {
                flag.fetch_add(1, Ordering::Relaxed);
            }
        }));
        handle.wait().expect("no job must panic");

        assert_eq!(ran.load(Ordering::Relaxed), 1000);
        assert_eq!(jobs.submitted_count(), 1000);
    }

    #[test]
    fn waiting_twice_is_harmless() {
        let jobs = JobSystem::with_workers(2).expect("the pool must start");
        let handle = jobs.submit(|| {});
        handle.wait().expect("the job must not panic");
        handle
            .wait()
            .expect("waiting again must return immediately");
        assert!(handle.is_complete());
    }

    #[test]
    fn a_cloned_handle_waits_for_the_same_work() {
        let jobs = JobSystem::with_workers(2).expect("the pool must start");
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);

        let handle = jobs.submit(move || {
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::Release);
        });
        let clone = handle.clone();

        clone.wait().expect("the job must not panic");
        assert!(done.load(Ordering::Acquire));
        assert!(handle.is_complete());
    }

    #[test]
    fn several_submissions_can_be_waited_on_together() {
        let jobs = JobSystem::with_workers(3).expect("the pool must start");
        let ran = counter();

        let handles: Vec<JobHandle> = (0..8)
            .map(|_| {
                let flag = Arc::clone(&ran);
                jobs.submit(move || {
                    flag.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();

        JobSystem::wait_all(&handles).expect("no job must panic");
        assert_eq!(ran.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn an_empty_batch_completes_immediately() {
        let jobs = JobSystem::with_workers(1).expect("the pool must start");
        let handle = jobs.submit_batch(Vec::<fn()>::new());
        assert!(handle.is_complete());
        handle.wait().expect("there was nothing to run");
        assert_eq!(jobs.submitted_count(), 0);
    }

    #[test]
    fn work_is_spread_across_workers() {
        // Each job parks briefly, so one worker alone could not finish them within the
        // wait; completing at all proves more than one worker ran jobs.
        let jobs = JobSystem::with_workers(4).expect("the pool must start");
        let concurrent = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));

        let handle = jobs.submit_batch((0..4).map(|_| {
            let concurrent = Arc::clone(&concurrent);
            let peak = Arc::clone(&peak);
            move || {
                let now = concurrent.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(now, Ordering::AcqRel);
                std::thread::sleep(Duration::from_millis(50));
                concurrent.fetch_sub(1, Ordering::AcqRel);
            }
        }));
        handle.wait().expect("no job must panic");

        assert!(
            peak.load(Ordering::Acquire) > 1,
            "four 50 ms jobs on four workers must overlap"
        );
    }

    #[test]
    fn a_panicking_job_is_reported_and_the_pool_survives() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let jobs = JobSystem::with_workers(2).expect("the pool must start");
        let handle = jobs.submit(|| panic!("deliberate test panic"));
        let result = handle.wait();

        // The pool must still work afterwards: a lost worker would be invisible except as
        // unexplained slowness later.
        let ran = counter();
        let flag = Arc::clone(&ran);
        jobs.submit(move || {
            flag.fetch_add(1, Ordering::Relaxed);
        })
        .wait()
        .expect("the pool must still run jobs");

        std::panic::set_hook(previous);

        let error = result.expect_err("a panicked job must be reported");
        assert_eq!(error.category(), crate::error::ErrorCategory::InvalidState);
        assert_eq!(ran.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn one_panicking_job_does_not_hide_the_rest_of_its_batch() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let jobs = JobSystem::with_workers(2).expect("the pool must start");
        let ran = counter();

        let handle = jobs.submit_batch((0..10).map(|index| {
            let flag = Arc::clone(&ran);
            move || {
                if index == 3 {
                    panic!("deliberate test panic");
                }
                flag.fetch_add(1, Ordering::Relaxed);
            }
        }));
        let result = handle.wait();
        std::panic::set_hook(previous);

        assert!(result.is_err());
        assert_eq!(
            ran.load(Ordering::Relaxed),
            9,
            "the other nine jobs must still have run"
        );
        assert!(handle.is_complete());
    }

    #[test]
    fn a_pool_with_no_workers_runs_jobs_on_the_calling_thread() {
        let jobs = JobSystem::with_workers(0).expect("a zero-worker pool is legal");
        assert_eq!(jobs.worker_count(), 0);

        let this_thread = std::thread::current().id();
        let observed = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&observed);

        let handle = jobs.submit(move || {
            *sink.lock().expect("the mutex is not poisoned") = Some(std::thread::current().id());
        });
        assert!(
            handle.is_complete(),
            "with no workers the job ran during submit"
        );
        assert_eq!(
            *observed.lock().expect("the mutex is not poisoned"),
            Some(this_thread)
        );

        let ran = counter();
        let batch = jobs.submit_batch((0..5).map(|_| {
            let flag = Arc::clone(&ran);
            move || {
                flag.fetch_add(1, Ordering::Relaxed);
            }
        }));
        batch.wait().expect("no job must panic");
        assert_eq!(ran.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn shutdown_joins_every_worker() {
        let jobs = JobSystem::with_workers(3).expect("the pool must start");
        let ran = counter();
        let handle = jobs.submit_batch((0..30).map(|_| {
            let flag = Arc::clone(&ran);
            move || {
                flag.fetch_add(1, Ordering::Relaxed);
            }
        }));
        handle.wait().expect("no job must panic");

        jobs.shutdown().expect("no worker must panic");
        assert_eq!(ran.load(Ordering::Relaxed), 30);
    }

    #[test]
    fn dropping_the_system_shuts_the_workers_down() {
        let ran = counter();
        {
            let jobs = JobSystem::with_workers(2).expect("the pool must start");
            let flag = Arc::clone(&ran);
            jobs.submit(move || {
                flag.fetch_add(1, Ordering::Relaxed);
            })
            .wait()
            .expect("the job must not panic");
        }
        // Drop returned, so the workers are stopped and joined. If they were not, the test
        // binary would hang at exit rather than fail here.
        assert_eq!(ran.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn jobs_can_write_to_disjoint_parts_of_a_shared_buffer() {
        // The shape parallel systems will actually use: split the work, no locking inside
        // the jobs, join once at the end.
        let jobs = JobSystem::with_workers(4).expect("the pool must start");
        const COUNT: usize = 4096;
        let data = Arc::new(Mutex::new(vec![0_u32; COUNT]));

        let chunks = 8;
        let handle = jobs.submit_batch((0..chunks).map(|chunk| {
            let data = Arc::clone(&data);
            move || {
                let range = (chunk * COUNT / chunks)..((chunk + 1) * COUNT / chunks);
                let mut guard = data.lock().expect("the mutex is not poisoned");
                for index in range {
                    guard[index] = index as u32 * 2;
                }
            }
        }));
        handle.wait().expect("no job must panic");

        let guard = data.lock().expect("the mutex is not poisoned");
        assert!(
            guard
                .iter()
                .enumerate()
                .all(|(index, value)| *value == index as u32 * 2)
        );
    }

    #[test]
    fn map_chunks_fills_every_output_slot() {
        let Ok(system) = JobSystem::with_workers(4) else {
            panic!("four workers must start");
        };
        let mut outputs = vec![0_usize; 64];
        let Ok(()) = system.map_chunks(&mut outputs, |index, output| *output = index * 3) else {
            panic!("no chunk panics");
        };
        assert!(outputs.iter().enumerate().all(|(i, v)| *v == i * 3));
    }

    #[test]
    fn map_chunks_lends_borrowed_data_to_its_jobs() {
        // The reason this method exists. `submit` requires `'static`, so a job cannot read
        // a local or write into one; a chunked cull needs both.
        let Ok(system) = JobSystem::with_workers(4) else {
            panic!("four workers must start");
        };
        let source: Vec<usize> = (0..1000).collect();
        let mut sums = vec![0_usize; 10];

        let Ok(()) = system.map_chunks(&mut sums, |chunk, output| {
            // Borrows `source`, which is a local of this function and very much not
            // 'static, and writes into a slot of another local.
            *output = source[chunk * 100..(chunk + 1) * 100].iter().sum();
        }) else {
            panic!("no chunk panics");
        };

        assert_eq!(sums.iter().sum::<usize>(), source.iter().sum::<usize>());
    }

    #[test]
    fn map_chunks_with_no_workers_runs_everything_on_the_caller() {
        // The determinism configuration: same API, same results, no threads.
        let Ok(system) = JobSystem::with_workers(0) else {
            panic!("a zero-worker system is legal");
        };
        let mut outputs = vec![0_usize; 8];
        let Ok(()) = system.map_chunks(&mut outputs, |index, output| *output = index + 1) else {
            panic!("no chunk panics");
        };
        assert_eq!(outputs, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(system.submitted_count(), 8);
    }

    #[test]
    fn map_chunks_over_nothing_does_nothing() {
        let Ok(system) = JobSystem::with_workers(2) else {
            panic!("two workers must start");
        };
        let mut empty: Vec<u32> = Vec::new();
        let Ok(()) = system.map_chunks(&mut empty, |_, _| panic!("never called")) else {
            panic!("an empty slice submits no jobs");
        };
        assert_eq!(system.submitted_count(), 0);
    }

    #[test]
    fn a_panicking_chunk_is_reported_and_the_rest_still_finish() {
        // Every job must be waited for even when one dies, or the transmute in
        // `map_chunks` would be unsound: a job still running after the call returns would
        // hold borrows that have gone away.
        let Ok(system) = JobSystem::with_workers(4) else {
            panic!("four workers must start");
        };
        let mut outputs = vec![0_usize; 16];

        let result = system.map_chunks(&mut outputs, |index, output| {
            assert_ne!(index, 7, "chunk seven fails on purpose");
            *output = index + 1;
        });

        assert!(result.is_err(), "the failure must be reported");
        // Everything except the one that panicked still ran to completion.
        for (index, value) in outputs.iter().enumerate() {
            let expected = if index == 7 { 0 } else { index + 1 };
            assert_eq!(*value, expected, "chunk {index}");
        }
    }

    #[test]
    fn map_chunks_matches_the_sequential_result_at_every_worker_count() {
        // Chunked work has to produce the same answer however many threads run it, or a
        // parallel cull would draw a different scene from a serial one.
        let source: Vec<u64> = (0..4096).map(|value| value * 7 % 101).collect();
        let expected: Vec<u64> = source.chunks(256).map(|chunk| chunk.iter().sum()).collect();

        for workers in [0, 1, 4, 8] {
            let Ok(system) = JobSystem::with_workers(workers) else {
                panic!("{workers} workers must start");
            };
            let mut sums = vec![0_u64; expected.len()];
            let Ok(()) = system.map_chunks(&mut sums, |chunk, output| {
                *output = source[chunk * 256..(chunk + 1) * 256].iter().sum();
            }) else {
                panic!("no chunk panics");
            };
            assert_eq!(sums, expected, "with {workers} workers");
        }
    }
}
