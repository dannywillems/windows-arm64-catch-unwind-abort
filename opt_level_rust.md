# What Rust's `opt-level` 1, 2 and 3 change

In Rust, `opt-level` 1, 2 and 3 select LLVM's O1, O2 and O3 pipelines. Each
level runs more optimisation passes than the one below. This file records
what changes between them, because the abort this repository investigates
appears at `opt-level = 3` and not at 2 (see [README.md](README.md)).

Everything below is read from the source of the versions that abort:
rustc 1.85.0 and LLVM 19.1.7. Line numbers refer to those versions. Later
versions may differ.

## How rustc maps the levels

In `compiler/rustc_codegen_llvm/src/back/write.rs`, lines 136-156 [R1]:

| `opt-level` | LLVM pass pipeline | LLVM code generation level |
|---|---|---|
| 0 | O0 | None |
| 1 | O1 | Less |
| 2 | O2 | Default |
| 3 | O3 | Aggressive |
| `s` | Os | Default, optimising for size |
| `z` | Oz | Default, optimising aggressively for size |

The pass pipeline transforms LLVM's intermediate representation. The code
generation level then controls the back end that turns it into machine code
(instruction selection, scheduling, register allocation).

rustc also decides auto-vectorisation itself, in
`compiler/rustc_codegen_ssa/src/back/write.rs`, lines 234-240 [R2], with the
comment "Copy what clang does by turning on loop vectorization at O2 and slp
vectorization at O3":

- **loop vectorisation**: on at `opt-level` 2 and 3, unless
  `-C no-vectorize-loops`;
- **SLP vectorisation**, which combines independent scalar operations into
  vector instructions: on at `opt-level` 3 only, unless `-C no-vectorize-slp`.

## Level 1 compared with level 2

In LLVM's `llvm/lib/Passes/PassBuilderPipelines.cpp` [L1], level 1 uses a
separate, reduced function-simplification pipeline:
`buildFunctionSimplificationPipeline` returns
`buildO1FunctionSimplificationPipeline` when the speed level is 1 (line 552;
the O1 pipeline starts at line 405).

The O2/O3 simplification pipeline adds, unconditionally, among others:

- `GVNPass`: global value numbering, which removes redundant computations;
- `JumpThreadingPass`: jump threading;
- `CorrelatedValuePropagationPass`: uses facts known from branches;
- `DSEPass`: dead store elimination;
- `TailCallElimPass`: tail-call elimination;
- `AggressiveInstCombinePass`;
- `MergedLoadStoreMotionPass`.

Passes in that function behind a command-line option that is off by default,
such as `GVNHoistPass`, `GVNSinkPass` and `DFAJumpThreadingPass`, are not
counted here.

Both levels inline. At level 1 and level 2 the inliner uses the same default
threshold, 225 (see below).

## Level 2 compared with level 3

This is where the abort appears. The two pipelines are almost identical.
Level 3 differs in these ways:

- **More inlining.** The inliner's threshold, the cost below which a call
  gets inlined, is 250 at O3 instead of 225. `computeThresholdFromOptLevels`
  returns `InlineConstants::OptAggressiveThreshold` when the speed level is
  above 2 (`llvm/lib/Analysis/InlineCost.cpp`, line 3219 [L2]);
  `OptAggressiveThreshold = 250` (`llvm/include/llvm/Analysis/InlineCost.h`,
  line 44 [L3]); otherwise it returns the default, the option
  `inlinedefault-threshold` with initial value 225 (`InlineCost.cpp`,
  line 57 [L2]). Larger functions get inlined into their callers. Also at O3
  only, `getInlineParams` sets `LocallyHotCallSiteThreshold` from the
  `-locally-hot-callsite-threshold` option; below O3 that option applies only
  when given explicitly (`InlineCost.cpp`, lines 3228-3236 [L2]).
- **Argument promotion** (`ArgumentPromotionPass`, `PassBuilderPipelines.cpp`
  line 955 [L1]): turns arguments passed by reference into arguments passed
  by value. The source's own comment says: "FIXME: It isn't at all clear why
  this should be limited to O3."
- **Call-site splitting** (`CallSiteSplittingPass`, line 1090 [L1]):
  duplicates a call so each copy sees more precise arguments.
- **Non-trivial loop unswitching** (`SimpleLoopUnswitchPass` with
  `NonTrivial` set to `Level == O3`, lines 652, 662 and 1286 [L1]): moves a
  condition out of a loop by duplicating the loop.
- **Control height reduction** (`ControlHeightReductionPass`, line 1461
  [L1]): O3 only, and only with profile data, so it does not apply here.
- **SLP vectorisation**, through the rustc setting above [R2].
- **The back end** runs at "Aggressive" instead of "Default" [R1], which
  changes passes after the IR pipeline, such as instruction scheduling and
  register allocation. Which ones, and how, is not covered here.

## What this suggests for the bug

The most plausible suspect is the inlining threshold. That is a hypothesis.

Only `original_two_catches` aborts, and it is the only test with two
`catch_unwind` closures in one function; the one-catch variants never abort.
With the higher threshold, both closures, the asserting function and
`std::panicking::try` could all end up inlined into the test function,
putting two catch regions in one frame. Whether they do is not checked yet;
the assembly would show it.

## Experiments that can separate the candidates

Each is one more entry in the reproducer's CI matrix:

| experiment | flags | if it aborts | if it passes |
|---|---|---|---|
| 1 | `opt-level = 2`, `-C llvm-args=-inlinedefault-threshold=250` | the higher inlining threshold alone is enough | something else at O3 is needed |
| 2 | `opt-level = 3`, `-C no-vectorize-slp` | SLP vectorisation is not needed | SLP vectorisation is involved |
| 3 | `opt-level = 3`, `-C llvm-args=-inline-threshold=225` | inlining is not the cause | inlining is involved |

Caveat for experiment 3: when `-inline-threshold` is given, it replaces the
default threshold at every level (`InlineCost.cpp`, lines 3168-3171 [L2]),
and it leaves O3's other inlining settings in place, so it does not exactly
restore O2's behaviour. Experiment 1 is the cleaner test.

## Sources

rustc, at tag `1.85.0`:

- [R1] `compiler/rustc_codegen_llvm/src/back/write.rs`, `to_llvm_opt_settings`
  and `to_pass_builder_opt_level`, lines 136-156.
  <https://github.com/rust-lang/rust/blob/1.85.0/compiler/rustc_codegen_llvm/src/back/write.rs#L136-L156>
- [R2] `compiler/rustc_codegen_ssa/src/back/write.rs`, `vectorize_loop` and
  `vectorize_slp`, lines 234-240.
  <https://github.com/rust-lang/rust/blob/1.85.0/compiler/rustc_codegen_ssa/src/back/write.rs#L234-L240>
- *The rustc book*, "Codegen options" (`opt-level`, `no-vectorize-loops`,
  `no-vectorize-slp`, `llvm-args`).
  <https://doc.rust-lang.org/rustc/codegen-options/index.html>

LLVM, at tag `llvmorg-19.1.7`:

- [L1] `llvm/lib/Passes/PassBuilderPipelines.cpp`.
  <https://github.com/llvm/llvm-project/blob/llvmorg-19.1.7/llvm/lib/Passes/PassBuilderPipelines.cpp>
- [L2] `llvm/lib/Analysis/InlineCost.cpp`.
  <https://github.com/llvm/llvm-project/blob/llvmorg-19.1.7/llvm/lib/Analysis/InlineCost.cpp>
- [L3] `llvm/include/llvm/Analysis/InlineCost.h`.
  <https://github.com/llvm/llvm-project/blob/llvmorg-19.1.7/llvm/include/llvm/Analysis/InlineCost.h>

Background:

- LLVM, `PassBuilder.h` documentation: the default pipelines correspond to
  the front end's `-O1`, `-O2` and `-O3`.
  <https://llvm.org/docs/doxygen/llvm_2Passes_2PassBuilder_8h_source.html>
- llvm-dev, "llc -O2 vs. llc -O3 --> same debug-pass=Executions but
  output.obj differs?" (2020): the level also changes thresholds inside
  passes, not only the list of passes.
  <https://lists.llvm.org/pipermail/llvm-dev/2020-August/144033.html>
- LLVM, *Auto-Vectorization in LLVM* (the loop and SLP vectorisers).
  <https://llvm.org/docs/Vectorizers.html>

## Authorship

Written with the help of Claude (Anthropic), through Claude Code, from the
sources above.
