# catch_unwind aborting on Windows ARM64

A minimal reproducer for a test that tries to catch a panic with
`std::panic::catch_unwind` and instead aborts the whole process, observed
on `aarch64-pc-windows-msvc` (GitHub's `windows-11-arm` runner) in an
optimised build with debug assertions enabled.

This repository is an investigation. The root cause is not known; the
hypothesis below is unconfirmed. Every factual claim links to a source in
[Sources](#sources), as `[S1]`, `[S2]`, and so on.

## Summary of the results so far

Reproduced outside the original crate (see [Results](#results)): the test
`original_two_catches` aborts with `0xc0000409` on `windows-11-arm` with Rust
1.85.0 (LLVM 19.1.7) at `opt-level = 3`, and only there. It passes at
`opt-level` 0 to 2, with Rust 1.100.0 (LLVM 23.1.5), and on the Windows x86-64
and Linux AArch64 controls. The same check split into one `catch_unwind` per
test function passes everywhere.

## What was observed first

The failure came from a test added in
[zcash/pasta_curves#182](https://github.com/zcash/pasta_curves/pull/182). It
called a generic function whose `debug_assert!` fails on purpose, and caught
the panic:

```rust
assert!(std::panic::catch_unwind(|| {
    affine_from_xy_unchecked::<pallas::Point>(Fp::ONE, Fp::ONE)
})
.is_err());
```

With `cargo test --release`, `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`,
`CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true` and Rust 1.85.0, the
`windows-11-arm` runner printed the panic message and then the test binary
died:

```
thread '...reconstructed_points_must_be_on_curve' panicked at src\glv.rs:139:5:
reconstructed point is on curve
process didn't exit successfully: `...pasta_curves-....exe --test-threads=1 --nocapture`
(exit code: 0xc0000409, STATUS_STACK_BUFFER_OVERRUN)
```

At the time:

- the same test passed in the debug profile on the same runner;
- it passed in release on Linux x86-64, Linux AArch64, macOS AArch64 and
  Windows x86-64;
- in the same release binary on `windows-11-arm`, `#[should_panic]` tests
  passed, and replacing this test with a `#[should_panic]` test made CI green.

## What `0xc0000409` means, and why it is not a stack overflow

`STATUS_STACK_BUFFER_OVERRUN` is a misleading name here:

- Windows lists `0xC0000409` as `STATUS_STACK_BUFFER_OVERRUN` [S1], but it is
  also the code the `__fastfail` intrinsic terminates a process with [S2].
  `__fastfail` ends the process immediately, without running exception
  handlers [S2].
- Rust's own `abort` on Windows is a `__fastfail` with
  `FAST_FAIL_FATAL_APP_EXIT`; on AArch64 it is the instruction `brk 0xF003`
  [S3].
- A stack overflow is reported differently: Windows uses `0xC00000FD`
  (`STATUS_STACK_OVERFLOW`) [S1], and Rust's handler prints
  `thread '...' has overflowed its stack` before letting it through [S4].
  Neither appeared.
- The stack is far from exhausted anyway. Rust's default stack for spawned
  threads, which is where libtest runs tests, is 2 MiB on Tier-1 platforms
  and can be changed with `RUST_MIN_STACK` [S5]. The MSVC default for the
  main thread is 1 MB [S6]. On macOS AArch64, with the same flags, every test
  of the original crate passed with `RUST_MIN_STACK` down to 64 KiB.

## Where the abort did not come from

Every abort path of the Rust runtime that a panic can reach prints a message
first, and none of them appeared:

- `failed to initiate panic` when the unwinder cannot start [S7];
- `thread panicked while processing panic. aborting.` on a double panic [S7];
- `panic in a function that cannot unwind` and
  `panic in a destructor during cleanup` [S8].

So the termination most likely happened below Rust: in the Windows unwinder
or in the MSVC C++ runtime that Rust's MSVC panics go through. That is an
inference from the absence of these messages, not an observation.

## How the panic is supposed to be caught

On MSVC targets:

1. A Rust panic is raised as a C++ exception, with `_CxxThrowException` [S9].
2. `catch_unwind` compiles to the `try` intrinsic, which on MSVC is a
   `catchswitch` plus a `catchpad`, left with `catchret` [S9]. LLVM turns
   these pads into funclets, small functions the runtime calls for the catch
   body [S10].
3. On ARM64 Windows, each function is described by `.pdata` records and,
   unless it is simple enough for packed data, `.xdata` unwind codes [S11].
   The system walks the stack with them to find a handler.
4. If no handler is found, or the walk cannot proceed, the exception is not
   caught. The MSVC runtime's `abort` reports through Windows Error Reporting
   or exits with code 3 [S12] [S13]; which path produced `0xc0000409` here is
   not established.

## Why `#[should_panic]` behaves differently

`#[should_panic]` also relies on `catch_unwind`: libtest runs every test
inside it [S14] and then checks the expected message [S15]. The difference is
where the catch is compiled:

| version | catch compiled in | between the panic and the catch |
|---|---|---|
| `catch_unwind` in the test | this crate, inlined into the test function, with the job's flags | the asserting function, the closure and `std::panicking::try`, likely inlined together |
| `#[should_panic]` | libtest, precompiled with the toolchain | the test function, the asserting function, then libtest's frames |

## Hypothesis (unconfirmed)

With Rust 1.85.0 (LLVM 19.1.7), for `aarch64-pc-windows-msvc` at
`opt-level = 3`, the code generated for two inlined `catch_unwind`s in one
function (their funclets or the function's unwind data) is wrong, so the
exception finds no handler. The results fit this: one catch per function
passes, lower opt-levels pass, and a toolchain with LLVM 23.1.5 passes. Not
checked yet: the generated assembly, the exact toolchain release that stops
reproducing, and whether a known LLVM or Rust issue matches.

## The variants

Each test in `src/lib.rs` changes one thing relative to the original:

| test | what it isolates |
|---|---|
| `original_two_catches` | the original test, unchanged: two catches in one function |
| `one_catch_pallas`, `one_catch_vesta` | one catch per test function |
| `noinline_callee` | the asserting function is `#[inline(never)]` |
| `opaque_inputs` | inputs behind `black_box`, so the failure is not known at compile time |
| `plain_assert` | `assert!` instead of `debug_assert!`; runs in every profile |
| `minimal_u64` | no `pasta_curves` at all: a failing `debug_assert!` on a computed `u64` |
| `control_should_panic` | the `#[should_panic]` replacement that passed |

The tests relying on a failing `debug_assert!` are compiled only when debug
assertions are on, which the workflow guarantees.

## What `cfg(debug_assertions)` means

`debug_assertions` is a configuration option the compiler sets when debug
assertions are enabled [S16]. Unless told otherwise, `rustc` enables them only
at `opt-level = 0` [S17]. Cargo enables them in the `dev` profile (plain
`cargo build` and `cargo test`) and disables them in `release` [S18], unless
the profile overrides it: `debug-assertions = true` in `Cargo.toml`, or the
`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` environment variable this
workflow sets [S19].

Code reads the option in two ways:

- `debug_assert!(...)` expands to
  `if cfg!(debug_assertions) { assert!(...) }` [S20]. `cfg!` is a constant
  `true` or `false`, so without the option the check is still type-checked
  but never runs, and the optimiser removes it. That is the point of the
  macro: checks for development builds that cost nothing in release.
- `#[cfg(debug_assertions)]` on an item (a function, a test) removes the item
  from the build entirely when the option is off, as if it were not written
  [S16].

This repository needs the second form because of the first: without the
option, the `debug_assert!` under test never fires, `catch_unwind` returns
`Ok`, and the test would fail for a reason unrelated to the bug. Gating the
test on the same option makes it run exactly when the assertion it relies on
exists.

The option also matters to the bug itself. A plain release build has no
failing assertion, hence no panic and nothing to catch, so the problem can
only show up in an optimised build that keeps the debug assertions. That is
an unusual combination, which `pasta_curves` CI uses on purpose.

`-C overflow-checks`, which makes integer overflow panic, is a separate
option. When it is not given, it follows the debug assertions [S17]; the
workflow sets it explicitly to match the original configuration.

## How CI runs them

`.github/workflows/repro.yml` builds the tests in release with debug
assertions and overflow checks, over this matrix:

- runners: `windows-11-arm` (under suspicion), `windows-latest` and
  `ubuntu-24.04-arm` (controls);
- toolchains: `1.85.0` (the one that failed) and `stable`;
- `opt-level`: 0, 1, 2 and 3.

An abort takes the whole test binary down, so each test runs in its own
process. Every job writes a table of `pass` / `abort (exit code ...)` /
`fail` to its step summary, and prints the output of every test that did not
pass. The jobs succeed whatever the results: the tables are the outcome.

## Results

Two runs on identical code, [43948372932] and [43948538058] (commits
`74da345` and `d423269`), gave identical results: 24 jobs, 8 tests each, 192
outcomes per run. All passed except one:

| runner | toolchain | opt-level | test | result |
|---|---|---|---|---|
| `windows-11-arm` | 1.85.0 (`4d91de4e4`, LLVM 19.1.7) | 3 | `original_two_catches` | abort, `0xc0000409` |

Everything else passed, including:

- `original_two_catches` on `windows-11-arm` with 1.85.0 at `opt-level` 0, 1
  and 2;
- `original_two_catches` on `windows-11-arm` with stable 1.100.0
  (`0ef1bc386`, LLVM 23.1.5) at every `opt-level`;
- `one_catch_pallas` and `one_catch_vesta`, the same check with one catch per
  function, everywhere;
- every test on `windows-latest` and `ubuntu-24.04-arm`.

What this says:

- The trigger is not specific to `pasta_curves` CI or its features: a crate
  depending on the published `pasta_curves` reproduces it.
- It needs two catches in the same function, at least with this code.
- It needs `opt-level = 3`.
- It does not reproduce with Rust 1.100.0.

`minimal_u64` did not abort, so the dependency-free version does not yet
reproduce; it has only one catch, so the next step is a dependency-free test
with two.

[43948372932]: https://github.com/dannywillems/windows-arm64-catch-unwind-abort/actions/runs/43948372932
[43948538058]: https://github.com/dannywillems/windows-arm64-catch-unwind-abort/actions/runs/43948538058

## Next steps

- A dependency-free variant with two catches in one function.
- Run with `--nocapture` to see whether the first or the second catch fails.
- Bisect the toolchain between 1.85.0 and 1.100.0 to find the release that
  stops reproducing, and note its LLVM version.
- Compare the assembly (`--emit=asm`) of `original_two_catches` at
  `opt-level` 2 and 3, looking at the funclets and the unwind data.

## Sources

Rust source code is cited at the `1.85.0` tag, the toolchain that aborts.

- [S1] Microsoft, *[MS-ERREF] NTSTATUS Values*:
  `0xC0000409 STATUS_STACK_BUFFER_OVERRUN`, `0xC00000FD STATUS_STACK_OVERFLOW`.
  <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-erref/596a1078-e883-4972-9bbc-49e60bebca55>
- [S2] Microsoft, *`__fastfail`*.
  <https://learn.microsoft.com/en-us/cpp/intrinsics/fastfail>
- [S3] Rust 1.85.0, `library/std/src/sys/pal/windows/mod.rs`,
  `abort_internal` (`brk 0xF003` with `FAST_FAIL_FATAL_APP_EXIT` on AArch64).
  <https://github.com/rust-lang/rust/blob/1.85.0/library/std/src/sys/pal/windows/mod.rs>
- [S4] Rust 1.85.0, `library/std/src/sys/pal/windows/stack_overflow.rs`,
  `vectored_handler`.
  <https://github.com/rust-lang/rust/blob/1.85.0/library/std/src/sys/pal/windows/stack_overflow.rs>
- [S5] Rust standard library, `std::thread`, section "Stack size".
  <https://doc.rust-lang.org/1.85.0/std/thread/index.html#stack-size>
- [S6] Microsoft, *`/STACK` (Stack allocations)*.
  <https://learn.microsoft.com/en-us/cpp/build/reference/stack-stack-allocations>
- [S7] Rust 1.85.0, `library/std/src/panicking.rs` (`failed to initiate
  panic`, `thread panicked while processing panic. aborting.`).
  <https://github.com/rust-lang/rust/blob/1.85.0/library/std/src/panicking.rs>
- [S8] Rust 1.85.0, `library/core/src/panicking.rs` (`panic in a function
  that cannot unwind`, `panic in a destructor during cleanup`).
  <https://github.com/rust-lang/rust/blob/1.85.0/library/core/src/panicking.rs>
- [S9] Rust 1.85.0, `library/panic_unwind/src/seh.rs`, module documentation:
  panics as C++ exceptions through `_CxxThrowException`, caught by a
  `catchswitch` and `catchpad` in the `try` intrinsic.
  <https://github.com/rust-lang/rust/blob/1.85.0/library/panic_unwind/src/seh.rs>
- [S10] LLVM, *Exception Handling in LLVM*, section on Windows runtimes and
  funclets. <https://llvm.org/docs/ExceptionHandling.html>; background:
  R. Kleckner and D. Majnemer, *Exception handling in LLVM, from Itanium to
  MSVC*, LLVM Developers' Meeting 2015,
  <https://llvm.org/devmtg/2015-10/slides/KlecknerMajnemer-ExceptionHandling.pdf>
- [S11] Microsoft, *ARM64 exception handling* (`.pdata`, `.xdata`, unwind
  codes). <https://learn.microsoft.com/en-us/cpp/build/arm64-exception-handling>
- [S12] Microsoft, *`abort`*.
  <https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/abort>
- [S13] Microsoft, *`_set_abort_behavior`*.
  <https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/set-abort-behavior>
- [S14] Rust 1.85.0, `library/test/src/lib.rs`: each test runs inside
  `catch_unwind`.
  <https://github.com/rust-lang/rust/blob/1.85.0/library/test/src/lib.rs>
- [S15] Rust 1.85.0, `library/test/src/test_result.rs`, `calc_result`
  (`ShouldPanic::YesWithMessage`).
  <https://github.com/rust-lang/rust/blob/1.85.0/library/test/src/test_result.rs>
- [S16] *The Rust Reference*, "Conditional compilation", `debug_assertions`.
  <https://doc.rust-lang.org/reference/conditional-compilation.html#debug_assertions>
- [S17] *The rustc book*, "Codegen options", `debug-assertions` and
  `overflow-checks`.
  <https://doc.rust-lang.org/rustc/codegen-options/index.html>
- [S18] *The Cargo Book*, "Profiles" (defaults of `dev` and `release`).
  <https://doc.rust-lang.org/cargo/reference/profiles.html>
- [S19] *The Cargo Book*, "Configuration", `profile.<name>.debug-assertions`
  and its `CARGO_PROFILE_<name>_DEBUG_ASSERTIONS` environment variable.
  <https://doc.rust-lang.org/cargo/reference/config.html>
- [S20] Rust 1.85.0, `library/core/src/macros/mod.rs`, `debug_assert!`; and
  the macro's documentation.
  <https://github.com/rust-lang/rust/blob/1.85.0/library/core/src/macros/mod.rs>,
  <https://doc.rust-lang.org/1.85.0/std/macro.debug_assert.html>

## Authorship

This investigation, the reproducer and this README were written with the
help of Claude (Anthropic), through Claude Code. Claims are verified against
the sources above; the hypothesis is marked as unconfirmed on purpose.

## License

MIT OR Apache-2.0, like `pasta_curves`.
