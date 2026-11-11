//! Process memory on macOS, from the Mach task and the kernel's resource accounting.

use super::ProcessMemory;

/// The process's memory now, or `None` if the OS would not say.
///
/// - `working_set` and `peak_working_set` are the Mach task's `resident_size` and
///   `resident_size_max`: physical memory mapped into the process, now and at its most.
/// - `private_bytes` is the kernel's `ri_phys_footprint`, the number Activity Monitor calls
///   "Memory": dirty and compressed pages charged to this process alone. It is the closest
///   macOS has to Windows' private usage and the one that grows when something leaks;
///   `resident_size` would also count shared libraries and clean file mappings.
///
/// Both calls are required: a half-filled report would be a made-up number.
#[must_use]
pub fn process_memory() -> Option<ProcessMemory> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::uninit();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // `libc` marks `mach_task_self_` deprecated in favour of the `mach2` crate, which this one
    // line does not justify depending on; the static itself is the stable system ABI.
    #[allow(deprecated)]
    // SAFETY: `mach_task_self_` is initialised by the system before `main` and only read.
    // `info` is a writable buffer of `count` 32-bit words, which is what the flavor asks
    // for, and `count` is a valid in/out parameter.
    let status = unsafe {
        libc::task_info(
            libc::mach_task_self_,
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if status != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: task_info returned KERN_SUCCESS, so it filled the structure.
    let info = unsafe { info.assume_init() };

    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v2>::uninit();
    // SAFETY: the pid is this process's own; the buffer is a writable `rusage_info_v2`, the
    // structure flavor `RUSAGE_INFO_V2` fills, passed through the API's pointer-to-pointer
    // typing.
    let status = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            usage.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return None;
    }
    // SAFETY: proc_pid_rusage returned 0, so it filled the structure.
    let usage = unsafe { usage.assume_init() };

    // The task structure is `packed(4)`: its fields are copied out, never borrowed.
    let resident = info.resident_size;
    let resident_max = info.resident_size_max;
    Some(ProcessMemory {
        working_set: resident,
        peak_working_set: resident_max.max(resident),
        private_bytes: usage.ri_phys_footprint,
    })
}
