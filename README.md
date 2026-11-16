# catch_unwind aborting on Windows ARM64

A minimal reproducer for a test that tries to catch a panic with
`std::panic::catch_unwind` and instead aborts the whole process, observed
on `aarch64-pc-windows-msvc` (GitHub's `windows-11-arm` runner) in an
optimised build with debug assertions enabled.

This repository is an investigation. The cause is not known yet; the
hypothesis below is unconfirmed.

## What was observed

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

What makes it interesting:

- the same test passes in the debug profile on the same runner;
- it passes in release on Linux x86-64, Linux AArch64, macOS AArch64 and
  Windows x86-64;
- in the same release binary on `windows-11-arm`, `#[should_panic]` tests
  pass, and replacing this test with a `#[should_panic]` test made CI green.

## It is not a stack overflow

`STATUS_STACK_BUFFER_OVERRUN` is a misleading name. `0xc0000409` is the code
Windows reports for `__fastfail`, which is how a process is terminated when an
exception finds no handler. A Rust stack overflow on Windows reports
`0xc00000fd` (`STATUS_STACK_OVERFLOW`) instead.

The stack is also far from exhausted: on macOS AArch64, with the same flags,
every test of that crate passes with the test threads' stack cut from the
default 2 MiB to 64 KiB.

## How the panic is supposed to be caught

On MSVC targets:

1. A Rust panic is raised as a C++ exception (`_CxxThrowException`).
2. `catch_unwind` compiles to a C++-style `try`/`catch`. LLVM emits the catch
   block as a small separate function (a funclet), and describes every frame
   in the unwind tables that Windows reads (`.pdata`/`.xdata`).
3. Windows walks the stack using those tables to find a frame with a
   matching catch.
4. If it finds none, or a table is wrong, nothing catches the exception and
   the C++ runtime terminates the process through `__fastfail`: `0xc0000409`.

The log fits step 4: the panic hook printed the message (the exception was
raised), and the process died without Rust's own "fatal runtime error" line
(no handler was found).

## Why `#[should_panic]` behaves differently

`#[should_panic]` also relies on `catch_unwind`: libtest runs every test
inside it and checks that it returned `Err`. The difference is where the
catch is compiled:

| version | catch compiled in | between the panic and the catch |
|---|---|---|
| `catch_unwind` in the test | this crate, inlined into the test function, with the job's flags | the asserting function, the closure and `std::panicking::try`, likely inlined together |
| `#[should_panic]` | libtest, precompiled with the toolchain | the test function, the asserting function, then libtest's frames |

## Hypothesis (unconfirmed)

For `aarch64-pc-windows-msvc`, the optimised code generated for an inlined
`catch_unwind` (its funclet or its unwind tables) is wrong in some case the
original test hits, so Windows does not find the catch. Neither the generated
assembly nor a matching known issue has been checked yet.

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

`debug_assertions` is a configuration flag the compiler sets when it is run
with `-C debug-assertions=on`. Cargo passes that in the `dev` profile (plain
`cargo build` and `cargo test`) and not in `release`, unless the profile
overrides it: `debug-assertions = true` in `Cargo.toml`, or the
`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` environment variable this
workflow sets.

Code reads the flag in two ways:

- `debug_assert!(cond, ...)` expands to roughly
  `if cfg!(debug_assertions) { assert!(cond, ...) }`. `cfg!` is a constant
  `true` or `false`, so without the flag the check is still type-checked but
  never runs, and the optimiser removes it. That is the point of the macro:
  checks for development builds that cost nothing in release.
- `#[cfg(debug_assertions)]` on an item (a function, a test) removes the item
  from the build entirely when the flag is off, as if it were not written.

This repository needs the second form because of the first: without the flag
the `debug_assert!` under test never fires, `catch_unwind` returns `Ok`, and
the test would fail for a reason unrelated to the bug. Gating the test on the
same flag makes it run exactly when the assertion it relies on exists.

The flag also matters to the bug itself. A plain release build has no failing
assertion, hence no panic and nothing to catch, so the problem can only show
up in an optimised build that keeps the debug assertions. That is an unusual
combination, which `pasta_curves` CI uses on purpose.

`-C overflow-checks`, which makes integer overflow panic, is a separate
flag. When it is not given, it follows `debug-assertions`; the workflow sets
it explicitly to match the original configuration.

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

## Reading the results

- `original_two_catches` aborts only on `windows-11-arm` at `opt-level` above
  0: reproduced outside `pasta_curves`.
- `minimal_u64` aborts too: the dependency is irrelevant and the reproducer
  can shrink further.
- `noinline_callee` or `one_catch_*` pass where the original aborts: inlining,
  or two catches in one frame, is involved.
- Nothing aborts anywhere: the trigger needs something from the original crate
  that is missing here (its other features, its own arithmetic), and the next
  step is to move closer to it.

## Results

Not run yet.

## License

MIT OR Apache-2.0, like `pasta_curves`.
