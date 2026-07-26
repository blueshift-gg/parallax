//! Reusable assertion values over execution outcomes.
//!
//! The verification dual of [`Fixture`]: fixtures make world setup a value you
//! can name, share, and compose; [`Check`] does the same for assertions. A
//! check runs one-off through [`Outcome::check`], or on every committed send
//! once registered with [`Test::invariant`].
//!
//! [`Fixture`]: crate::fixture::Fixture
//! [`Test::invariant`]: crate::Test::invariant

use crate::Outcome;

/// An assertion over an execution [`Outcome`], panicking with an actionable
/// message when it does not hold.
///
/// Closures are checks (`outcome.check(|o: &Outcome| ...)`), and so are arrays
/// and tuples of checks, so one `check` call can carry several. Implement the
/// trait on a struct to give a protocol invariant a name:
///
/// ```rust,ignore
/// struct Solvent { pool: Pubkey }
///
/// impl Check for Solvent {
///     fn check(&self, outcome: &Outcome) {
///         outcome.has_state::<Pool>(self.pool, |pool| {
///             assert!(pool.reserves >= pool.obligations, "pool {} insolvent", self.pool);
///         });
///     }
/// }
///
/// test.send(swap).succeeds().check(Solvent { pool });
/// test.invariant(Solvent { pool }); // or: verified after every send
/// ```
pub trait Check {
    /// Assert against the outcome, panicking when the check fails.
    fn check(&self, outcome: &Outcome);
}

impl<F: Fn(&Outcome)> Check for F {
    fn check(&self, outcome: &Outcome) {
        self(outcome);
    }
}

impl<C: Check, const N: usize> Check for [C; N] {
    fn check(&self, outcome: &Outcome) {
        for check in self {
            check.check(outcome);
        }
    }
}

macro_rules! impl_check_for_tuple {
    ($($name:ident),+) => {
        impl<$($name: Check),+> Check for ($($name,)+) {
            fn check(&self, outcome: &Outcome) {
                #[allow(non_snake_case)]
                let ($($name,)+) = self;
                $($name.check(outcome);)+
            }
        }
    };
}

impl_check_for_tuple!(A, B);
impl_check_for_tuple!(A, B, C);
impl_check_for_tuple!(A, B, C, D);

/// A compute-unit budget as a check value.
///
/// The check-shaped sibling of [`Outcome::cu_at_most`], for budgets that are
/// passed around, grouped with other checks, or registered as invariants.
#[derive(Debug, Clone, Copy)]
pub struct CuBudget {
    limit: u64,
}

impl CuBudget {
    /// Budget an inclusive compute-unit ceiling.
    pub fn at_most(limit: u64) -> Self {
        Self { limit }
    }
}

impl Check for CuBudget {
    fn check(&self, outcome: &Outcome) {
        assert!(
            outcome.compute_units() <= self.limit,
            "CU budget exceeded: expected at most {}, consumed {}",
            self.limit,
            outcome.compute_units()
        );
    }
}
