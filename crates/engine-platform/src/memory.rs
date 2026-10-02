//! How much memory the process is using, as the OS counts it.
//!
//! For the developer report and the performance audit: one kernel call, meant for a report
//! interval rather than for every frame.

use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

/// The process's memory, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessMemory {
    /// Physical memory currently mapped into the process.
    pub working_set: u64,
    /// The most the working set has ever been.
    pub peak_working_set: u64,
    /// Memory committed to this process alone: the heap, stacks, private mappings. The
    /// number that grows if something leaks.
    pub private_bytes: u64,
}

/// The process's memory now, or `None` if the OS would not say.
#[must_use]
pub fn process_memory() -> Option<ProcessMemory> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: FFI call. `GetCurrentProcess` returns a pseudo-handle that needs no closing,
    // and the out-parameter is a correctly sized `PROCESS_MEMORY_COUNTERS_EX`, whose layout
    // begins with `PROCESS_MEMORY_COUNTERS` — which is why the cast is valid and why `cb`
    // says how large it really is.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            std::ptr::from_mut(&mut counters).cast(),
            counters.cb,
        )
    };
    if !ok.as_bool() {
        return None;
    }
    Some(ProcessMemory {
        working_set: counters.WorkingSetSize as u64,
        peak_working_set: counters.PeakWorkingSetSize as u64,
        private_bytes: counters.PrivateUsage as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_process_has_memory() {
        let memory = process_memory();
        assert!(memory.is_some_and(|memory| memory.working_set > 0
            && memory.peak_working_set >= memory.working_set
            && memory.private_bytes > 0));
    }
}
