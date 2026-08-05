# Phase 0 — Rust / D3D12 technical spike

Phase 0 validates the technical foundation only. No gameplay, no ECS, no assets,
no lighting, no UI framework.

Milestone status:

| # | Milestone | Status |
| --- | --- | --- |
| 0 | Project bootstrap | done |
| 1 | Windows platform layer | not started |
| 2 | D3D12 initialization | not started |
| 3 | Render loop | not started |
| 4 | Triangle renderer | not started |
| 5 | GPU timing | not started |
| 6 | Debug performance overlay | not started |
| 7 | Rendering stress tests | not started |
| 8 | Instancing test | not started |
| 9 | Automated GPU integration test | not started |

---

## Milestone 0 — Project bootstrap

### What was built

* Cargo workspace at the repository root (`resolver = "3"`, edition 2024).
* `engine-core`: a stderr logging backend for the `log` facade, with level parsing and
  an `MYGAME_LOG` environment override.
* `engine-app`: the `mygame` binary. Installs the logger and exits. No graphics.
* `rustfmt.toml`, `rust-toolchain.toml` (pins stable + `rustfmt` + `clippy` +
  the `x86_64-pc-windows-msvc` target).
* Workspace lints: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::todo`,
  `clippy::undocumented_unsafe_blocks` and `unsafe_op_in_unsafe_fn` are denied;
  `missing_docs` warns.
* Release profile: `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, `debug = 1`
  (line tables retained so release profiling is possible).
* Docs: `README.md`, `architecture.md`, `phase0.md`, `testing.md`, `performance.md`.

### Acceptance criteria

| Criterion | Result |
| --- | --- |
| `cargo build` succeeds | pass |
| `cargo test` succeeds | pass |
| `cargo fmt --check` succeeds | pass |
| `cargo clippy` succeeds | pass |

### Manual verification

```powershell
cargo run --bin mygame
```

Expect three log lines on stderr (`mygame 0.1.0`, a milestone line, a debug line)
and exit code 0. `$env:MYGAME_LOG = "warn"; cargo run --bin mygame` should
print nothing.

### Known limitations

* No window, no device, no rendering — by design for this milestone.
* The logger writes synchronously to stderr under a lock, and `log!` macros allocate
  while formatting. Acceptable for startup and interval reporting; banned from the
  per-frame path.
* Timestamps are process-relative seconds, not wall clock.
