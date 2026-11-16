//! Reproducer for a `catch_unwind` that does not catch.
//!
//! On `aarch64-pc-windows-msvc`, in an optimised build with debug assertions
//! enabled, catching the panic of a failing `debug_assert!` with
//! `std::panic::catch_unwind` aborted the whole test binary with
//! `0xc0000409` (`STATUS_STACK_BUFFER_OVERRUN`, the `__fastfail` code) instead
//! of returning `Err`. The same test passed in the debug profile, on every
//! other platform, and as a `#[should_panic]` test.
//!
//! Every test below is one variant. A test that aborts takes its whole binary
//! down, so CI runs each test in its own process and records its exit status.
//! Tests that need a failing `debug_assert!` are compiled only with debug
//! assertions (`#[cfg(debug_assertions)]`): without them the assertion never
//! runs, `catch_unwind` returns `Ok`, and the test would fail for an unrelated
//! reason. See the README for what the flag is and how CI sets it.

use pasta_curves::arithmetic::{CurveAffine, CurveExt};
use pasta_curves::{pallas, vesta, Fp, Fq};

/// The base field of a curve, as the original code named it.
type Base<C> = <<C as CurveExt>::AffineExt as CurveAffine>::Base;

/// Mirrors the private `Sealed` trait of `pasta_curves::glv`: a per-curve
/// unchecked affine constructor reached generically.
pub trait Unchecked: CurveExt {
    /// The affine point $(x, y)$, without checking that it is on the curve.
    fn from_xy_unchecked(x: Base<Self>, y: Base<Self>) -> Self::AffineExt;
}

impl Unchecked for pallas::Point {
    fn from_xy_unchecked(x: Fp, y: Fp) -> pallas::Affine {
        pallas::Affine::from_xy_unchecked(x, y)
    }
}

impl Unchecked for vesta::Point {
    fn from_xy_unchecked(x: Fq, y: Fq) -> vesta::Affine {
        vesta::Affine::from_xy_unchecked(x, y)
    }
}

/// The function from `pasta_curves::glv` whose assertion was caught:
/// generic, inlinable, with a `debug_assert!` on the result.
pub fn affine_from_xy_unchecked<C: Unchecked>(x: Base<C>, y: Base<C>) -> C::AffineExt {
    let point = C::from_xy_unchecked(x, y);
    debug_assert!(
        bool::from(point.is_on_curve()),
        "reconstructed point is on curve"
    );
    point
}

/// The same, but never inlined.
#[inline(never)]
pub fn affine_from_xy_unchecked_noinline<C: Unchecked>(x: Base<C>, y: Base<C>) -> C::AffineExt {
    affine_from_xy_unchecked::<C>(x, y)
}

/// The same, with `assert!`, so it fails whatever the debug assertions.
pub fn affine_from_xy_checked<C: Unchecked>(x: Base<C>, y: Base<C>) -> C::AffineExt {
    let point = C::from_xy_unchecked(x, y);
    assert!(
        bool::from(point.is_on_curve()),
        "reconstructed point is on curve"
    );
    point
}

/// No dependency on `pasta_curves`: a generic function whose `debug_assert!`
/// fails on a value computed at run time.
pub trait Check: Copy {
    /// Whether the value passes.
    fn ok(self) -> bool;
}

impl Check for u64 {
    fn ok(self) -> bool {
        self.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(17) % 3 == 0
    }
}

/// The dependency-free analogue of [`affine_from_xy_unchecked`].
pub fn checked_value<T: Check>(x: T) -> T {
    debug_assert!(x.ok(), "value passes the check");
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use pasta_curves::group::ff::Field;
    use std::hint::black_box;
    use std::panic::catch_unwind;

    /// The original test, unchanged: two `catch_unwind`s in one function.
    #[cfg(debug_assertions)]
    #[test]
    fn original_two_catches() {
        assert!(
            catch_unwind(|| { affine_from_xy_unchecked::<pallas::Point>(Fp::ONE, Fp::ONE) })
                .is_err()
        );
        assert!(
            catch_unwind(|| { affine_from_xy_unchecked::<vesta::Point>(Fq::ONE, Fq::ONE) })
                .is_err()
        );
    }

    /// One `catch_unwind` per test function.
    #[cfg(debug_assertions)]
    #[test]
    fn one_catch_pallas() {
        assert!(
            catch_unwind(|| { affine_from_xy_unchecked::<pallas::Point>(Fp::ONE, Fp::ONE) })
                .is_err()
        );
    }

    /// One `catch_unwind` per test function.
    #[cfg(debug_assertions)]
    #[test]
    fn one_catch_vesta() {
        assert!(
            catch_unwind(|| { affine_from_xy_unchecked::<vesta::Point>(Fq::ONE, Fq::ONE) })
                .is_err()
        );
    }

    /// The asserting function is never inlined into the closure.
    #[cfg(debug_assertions)]
    #[test]
    fn noinline_callee() {
        assert!(catch_unwind(|| {
            affine_from_xy_unchecked_noinline::<pallas::Point>(Fp::ONE, Fp::ONE)
        })
        .is_err());
    }

    /// Coordinates hidden from the optimiser, so the failing check is not
    /// known at compile time.
    #[cfg(debug_assertions)]
    #[test]
    fn opaque_inputs() {
        assert!(catch_unwind(|| {
            affine_from_xy_unchecked::<pallas::Point>(black_box(Fp::ONE), black_box(Fp::ONE))
        })
        .is_err());
    }

    /// `assert!` instead of `debug_assert!`: runs in every profile.
    #[test]
    fn plain_assert() {
        assert!(
            catch_unwind(|| { affine_from_xy_checked::<pallas::Point>(Fp::ONE, Fp::ONE) }).is_err()
        );
    }

    /// No `pasta_curves`: a failing `debug_assert!` on a computed `u64`.
    #[cfg(debug_assertions)]
    #[test]
    fn minimal_u64() {
        assert!(!1u64.ok(), "the input must fail the check");
        assert!(catch_unwind(|| checked_value(black_box(1u64))).is_err());
    }

    /// Control: the replacement that passed in CI.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "reconstructed point is on curve")]
    fn control_should_panic() {
        affine_from_xy_unchecked::<pallas::Point>(Fp::ONE, Fp::ONE);
    }
}
