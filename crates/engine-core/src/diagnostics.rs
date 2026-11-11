//! Assertions and invariant checks.
//!
//! ## When to use which
//!
//! | Situation | Tool |
//! | --- | --- |
//! | A programmer broke an invariant | [`engine_assert!`] |
//! | A check worth keeping in release, cheaply | [`engine_verify!`] |
//! | The user, the OS, the GPU or a file failed | `Result<T, EngineError>` |
//!
//! Assertions are **not** error handling. A missing file, a lost device, an invalid
//! configuration and a failed allocation are all things that happen to correct programs,
//! and they return `Result`. An assertion says "this cannot happen unless the engine
//! itself is wrong", and its failure is a bug report, not a runtime condition to recover
//! from.
//!
//! ## Cost
//!
//! [`engine_assert!`] compiles to nothing unless [`ASSERTIONS_ENABLED`] — that is, in
//! debug builds, or in any build with the `assertions` feature. The condition is not
//! evaluated, so an expensive invariant check costs nothing in a shipping build.
//!
//! [`engine_verify!`] always evaluates its condition and always yields a `bool`. On
//! failure it panics where assertions are enabled, and logs an error and returns `false`
//! where they are not — so a release build reports the problem and keeps running.

/// Whether [`engine_assert!`] checks are compiled in.
///
/// True in debug builds, and in any build with the `assertions` feature enabled. The
/// feature exists so a release build can be shipped to a tester with checks intact.
pub const ASSERTIONS_ENABLED: bool = cfg!(any(debug_assertions, feature = "assertions"));

/// Panics if `condition` is false, in builds where assertions are enabled.
///
/// Compiles to nothing otherwise — including the condition, so the check may be as
/// expensive as it needs to be.
///
/// For programmer errors and broken invariants only. Runtime failures return `Result`.
///
/// ```
/// # use engine_core::engine_assert;
/// let capacity = 16_usize;
/// engine_assert!(capacity.is_power_of_two());
/// engine_assert!(capacity >= 8, "capacity {capacity} is below the minimum");
/// ```
#[macro_export]
macro_rules! engine_assert {
    ($condition:expr $(,)?) => {
        if $crate::diagnostics::ASSERTIONS_ENABLED && !$condition {
            $crate::diagnostics::assertion_failed(
                stringify!($condition),
                ::core::file!(),
                ::core::line!(),
                ::core::format_args!("invariant violated"),
            );
        }
    };
    ($condition:expr, $($argument:tt)+) => {
        if $crate::diagnostics::ASSERTIONS_ENABLED && !$condition {
            $crate::diagnostics::assertion_failed(
                stringify!($condition),
                ::core::file!(),
                ::core::line!(),
                ::core::format_args!($($argument)+),
            );
        }
    };
}

/// Panics if the two values differ, in builds where assertions are enabled.
///
/// Reports both values, which is the whole reason to prefer it over
/// [`engine_assert!`] with a comparison inside.
///
/// ```
/// # use engine_core::engine_assert_eq;
/// let used = 128_usize;
/// engine_assert_eq!(used % 16, 0, "allocations stay 16-byte aligned");
/// ```
#[macro_export]
macro_rules! engine_assert_eq {
    ($left:expr, $right:expr $(,)?) => {
        if $crate::diagnostics::ASSERTIONS_ENABLED {
            let (left, right) = (&$left, &$right);
            if left != right {
                $crate::diagnostics::assertion_failed(
                    concat!(stringify!($left), " == ", stringify!($right)),
                    ::core::file!(),
                    ::core::line!(),
                    ::core::format_args!("left = {:?}, right = {:?}", left, right),
                );
            }
        }
    };
    ($left:expr, $right:expr, $($argument:tt)+) => {
        if $crate::diagnostics::ASSERTIONS_ENABLED {
            let (left, right) = (&$left, &$right);
            if left != right {
                $crate::diagnostics::assertion_failed(
                    concat!(stringify!($left), " == ", stringify!($right)),
                    ::core::file!(),
                    ::core::line!(),
                    ::core::format_args!(
                        "left = {:?}, right = {:?}: {}",
                        left,
                        right,
                        ::core::format_args!($($argument)+),
                    ),
                );
            }
        }
    };
}

/// Evaluates `condition` in every build and yields it as a `bool`.
///
/// On failure: panics where assertions are enabled, logs an error and yields `false`
/// where they are not. Use it for checks that should survive into a release build and
/// whose failure the caller can act on:
///
/// ```
/// # use engine_core::engine_verify;
/// # let buffer_len = 4_usize;
/// # let requested = 2_usize;
/// if !engine_verify!(requested <= buffer_len, "requested {requested} of {buffer_len}") {
///     // Release builds reach here instead of aborting.
///     return;
/// }
/// ```
#[macro_export]
macro_rules! engine_verify {
    ($condition:expr $(,)?) => {
        $crate::diagnostics::verify_result(
            $condition,
            stringify!($condition),
            ::core::file!(),
            ::core::line!(),
            ::core::format_args!("check failed"),
        )
    };
    ($condition:expr, $($argument:tt)+) => {
        $crate::diagnostics::verify_result(
            $condition,
            stringify!($condition),
            ::core::file!(),
            ::core::line!(),
            ::core::format_args!($($argument)+),
        )
    };
}

/// Reports a failed assertion and panics.
///
/// Not called directly; [`engine_assert!`] and [`engine_assert_eq!`] expand to it. Marked
/// cold so the failure path stays out of the hot path's instruction cache.
///
/// # Panics
///
/// Always. That is its purpose.
#[cold]
#[inline(never)]
pub fn assertion_failed(
    condition: &str,
    file: &str,
    line: u32,
    message: core::fmt::Arguments<'_>,
) -> ! {
    // Logged as well as panicked: the panic goes to stderr, but a log sink may be the
    // only thing being captured when this fires on a tester's machine.
    log::error!(target: "Diagnostics", "assertion failed at {file}:{line}: {condition} — {message}");
    // The record is queued, not written: without this the panic can outrun it.
    crate::logging::drain();
    panic!("engine assertion failed at {file}:{line}: {condition} — {message}");
}

/// Implements [`engine_verify!`]: panics or logs, and returns the condition.
///
/// Marked cold on the failure path only; the success path is a single branch.
#[must_use]
pub fn verify_result(
    condition: bool,
    text: &str,
    file: &str,
    line: u32,
    message: core::fmt::Arguments<'_>,
) -> bool {
    if !condition {
        verify_failed(text, file, line, message);
    }
    condition
}

/// Failure half of [`engine_verify!`], kept out of line.
#[cold]
#[inline(never)]
fn verify_failed(text: &str, file: &str, line: u32, message: core::fmt::Arguments<'_>) {
    if ASSERTIONS_ENABLED {
        assertion_failed(text, file, line, message);
    }
    log::error!(target: "Diagnostics", "check failed at {file}:{line}: {text} — {message}");
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Runs `body`, returning the panic message if it panicked.
    ///
    /// The panic hook is silenced first: these tests panic on purpose, and the default
    /// hook would fill the test output with backtraces that look like failures.
    fn panic_message(body: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = std::panic::catch_unwind(body);
        std::panic::set_hook(previous);

        result.err().map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_default()
        })
    }

    #[test]
    fn a_true_assertion_does_nothing() {
        engine_assert!(1 + 1 == 2);
        engine_assert!(true, "with a message");
        engine_assert_eq!(2 + 2, 4);
    }

    #[test]
    fn a_false_assertion_names_the_condition_and_the_location() {
        let message = panic_message(|| {
            engine_assert!(1 > 2);
        })
        .expect("assertions are enabled in test builds");

        assert!(message.contains("1 > 2"), "{message}");
        assert!(message.contains("diagnostics.rs"), "{message}");
    }

    #[test]
    fn an_assertion_message_reaches_the_panic() {
        let capacity = 4_usize;
        let message = panic_message(move || {
            engine_assert!(capacity >= 8, "capacity {capacity} is below the minimum");
        })
        .expect("assertions are enabled in test builds");

        assert!(
            message.contains("capacity 4 is below the minimum"),
            "{message}"
        );
    }

    #[test]
    fn an_equality_assertion_reports_both_values() {
        let message = panic_message(|| {
            engine_assert_eq!(7_u32, 9_u32);
        })
        .expect("assertions are enabled in test builds");

        assert!(message.contains("left = 7"), "{message}");
        assert!(message.contains("right = 9"), "{message}");
    }

    #[test]
    fn an_equality_assertion_appends_its_message() {
        let message = panic_message(|| {
            engine_assert_eq!(1_u8, 2_u8, "alignment must match");
        })
        .expect("assertions are enabled in test builds");

        assert!(message.contains("alignment must match"), "{message}");
    }

    #[test]
    fn verify_yields_its_condition() {
        assert!(engine_verify!(1 + 1 == 2));
        assert!(engine_verify!(true, "with a message"));
    }

    #[test]
    fn a_failing_verify_panics_where_assertions_are_enabled() {
        const { assert!(ASSERTIONS_ENABLED, "test builds compile assertions in") };
        let message = panic_message(|| {
            let _ = engine_verify!(1 > 2, "ordering is broken");
        })
        .expect("verify panics where assertions are enabled");

        assert!(message.contains("ordering is broken"), "{message}");
    }

    #[test]
    fn verify_evaluates_its_condition_exactly_once() {
        // A macro that expanded its argument twice would double every side effect, which
        // is exactly the kind of bug that only shows up in a release build.
        let mut calls = 0;
        let mut check = || {
            calls += 1;
            true
        };
        assert!(engine_verify!(check()));
        assert_eq!(calls, 1);
    }

    #[test]
    fn assertions_are_compiled_in_for_this_build() {
        assert_eq!(
            ASSERTIONS_ENABLED,
            cfg!(debug_assertions) || cfg!(feature = "assertions")
        );
    }
}
