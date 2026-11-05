//! The D3D12 performance test. Windows only: it drives the D3D12 renderer's benchmark
//! harness directly. On macOS this binary exists so the workspace builds, and says so.

use std::process::ExitCode;

#[cfg(windows)]
#[path = "performance_test_windows.rs"]
mod imp;

#[cfg(windows)]
fn main() -> ExitCode {
    imp::main()
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("performance_test drives the D3D12 renderer and runs on Windows only");
    ExitCode::FAILURE
}
