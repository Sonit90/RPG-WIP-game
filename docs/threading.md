# Threading

## Layers

```
Worker            one named OS thread, cooperative shutdown, joined on drop
  |
JobSystem         a pool of workers served by one shared queue
  |
JobHandle         a receipt: wait for one submission
```

`engine-core::threading` knows nothing about jobs; `engine-core::jobs` is built on it.

## Worker

* **Named.** The name reaches the OS, so a debugger, a crash dump and this engine's log
  lines all agree on which thread is which.
* **Cooperative shutdown.** There is no safe way to kill a thread, so shutdown is a flag
  the body polls. A worker that blocks must block on something that can be woken by the
  same code that sets the flag, or it will not notice.
* **Joined on drop.** A detached engine thread outliving the data it borrows is a
  use-after-free waiting for a slow machine. Forgetting to join is not possible.
* **A panicked worker is reported, not resumed.** `Worker::join` returns an `InvalidState`
  error; the caller decides what a dead worker means. `Drop` cannot return, so it logs.

## Job system

One `Mutex<VecDeque>`, one `Condvar`, one completion counter per submission. That is the
simplest design that is correct, and it is deliberately where this starts.

Absent on purpose: work stealing, lock-free queues, fibers, dependency graphs. Each costs
real complexity, and each is justified by a measurement rather than by a list of features
other engines have. The numbers below are what such a case would have to beat.

### Rules

* **Jobs must not wait on other jobs.** If every worker blocks waiting, nothing is left to
  run the work and the pool deadlocks. This is the limitation that eventually motivates a
  fiber scheduler; until then it is a rule.
* **A panicking job does not kill its worker.** The panic is caught, recorded on the
  submission and surfaced by `JobHandle::wait`. Losing a worker silently would show up
  only as unexplained slowness later.
* **Zero workers is legal.** Jobs then run on the calling thread during `submit`, so a
  single-threaded or determinism-testing configuration uses the same API without branching
  at every call site.
* **A batch beats N submissions.** One counter and one wake-up when the last job lands.

## Measured cost

Ryzen 7 5800X-class machine, 24 logical processors reported by `available_parallelism`,
release build, `cargo run --release --bin cpu_bench`. Median of 9 samples.

### Submission overhead

| Measurement | Cost |
| --- | --- |
| submit + wait, 1 empty job | ~3.2 µs |
| submit + wait, 1000 empty jobs | ~115 ns per job |

The single-job figure is dominated by waking a parked worker and waiting for it. Batching
amortizes that by ~28×.

### Scaling, 1000 jobs

| Work per job | Sequential | 1 worker | 11 workers | 23 workers |
| --- | --- | --- | --- | --- |
| 100 iterations (~0.1 µs) | 0.105 ms | 0.208 ms (0.50×) | 0.395 ms (0.26×) | 0.432 ms (0.24×) |
| 10 000 iterations (~12 µs) | 12.67 ms | 12.45 ms (1.02×) | 1.31 ms (9.69×) | 0.68 ms (18.58×) |

**Small jobs are slower in parallel, and more workers make it worse.** At ~0.1 µs of work,
submission and contention cost several times the job itself, and adding workers adds
contention on the one shared queue. This is measured, not assumed, and it is why nothing
in the engine will be "moved onto jobs" without a measurement of the job's size.

The break-even is somewhere between 0.1 µs and 12 µs of work per job on this machine. At
12 µs a job, scaling is 18.6× on 23 workers — most of what the hardware can give.

### What this implies

* Parallelize by **chunking**, not per item. A job per particle is on the wrong side of
  the line; a job per 1000 particles is on the right side.
* The one shared queue is not the bottleneck for jobs of useful size. Work stealing would
  address contention that only appears with many small jobs, which is the case the guidance
  above avoids anyway.
* Re-run `cpu_bench` before and after any change to this system, and update this table.

## What uses the job system

| Caller | Threshold below which it stays serial | Determinism test |
| --- | --- | --- |
| `Culler::cull_parallel` | 4096 slots per chunk | `engine-renderer/tests/render_list_pipeline.rs` |
| `UiBatcher::build_with` | 2048 work units, ~95 µs | `engine-renderer/tests/ui_batch_parallel.rs` |
| `ParticlePool::update_with` | 8192 particles | `engine-vfx/tests/particle_parallel.rs` |
| `ParticlePool::write_with` | 8192 particles | `engine-vfx/tests/particle_parallel.rs` |

`engine-app` owns one pool of `recommended_worker_count() - 1` workers — one core is the game
thread's, and it runs a chunk itself rather than idling. `--serial-jobs` starts the game with
no pool at all, which is the A/B control and the determinism configuration.

**None of these run parallel in the game today.** Every one of them measures under the
break-even above at the scenes the game currently produces, and the thresholds are what keep
them serial rather than paying submission overhead to save nothing. They engage on their own
when a scene grows past them. See `docs/performance.md`, Milestone 25, for the measurements
each threshold was derived from.

Two rules the split has to obey, both learned from the passes above:

* **The serial path calls the same body.** `cull_range`, `build_range`, `integrate_chunk`,
  `write_range` — one implementation, reached two ways. Two implementations drift.
* **The merge must not cost the serial path anything.** The UI batcher's one-chunk merge swaps
  its buffer rather than copying it; before that it put ~4 µs on every serial frame, which is
  the serial path paying for a parallel path it never takes.
