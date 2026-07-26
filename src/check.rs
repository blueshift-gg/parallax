//! Reusable assertion values over execution outcomes.
//!
//! The verification dual of [`Fixture`]: fixtures make world setup a value you
//! can name, share, and compose; [`Check`] does the same for assertions. The
//! verdict (`succeeds`/`fails`/`fails_with`) is a method on the outcome —
//! everything else is a check value, run one-off through [`Outcome::check`] or
//! on every committed send once registered with [`Test::invariant`]:
//!
//! ```rust,ignore
//! test.send(deposit).succeeds().check([
//!     CuBudget::le(5_000),
//!     Lamports::eq(vault, amount),
//!     Changes::eq([signer, vault]),
//!     Changes::created(vault),
//! ]);
//! ```
//!
//! Every built-in constructor returns the same concrete [`Assert`] type, so a
//! plain array groups them. Closures are checks too, and so are arrays and
//! tuples of checks; implement the trait on a struct to give a protocol
//! invariant a name.
//!
//! [`Fixture`]: crate::fixture::Fixture
//! [`Test::invariant`]: crate::Test::invariant

use {
    crate::{
        outcome::{mint_supply, token_amount},
        Outcome, Pubkey,
    },
    wincode::{config::DefaultConfig, SchemaRead},
};

/// An assertion over an execution [`Outcome`], panicking with an actionable
/// message when it does not hold.
///
/// Built-in checks are constructed from the fact namespaces ([`CuBudget`],
/// [`Lamports`], [`Tokens`], [`Supply`], [`State`], [`Data`], [`ReturnData`],
/// [`Owner`], [`Changes`]). Closures, arrays, and tuples of checks all
/// qualify, and applications implement the trait to name their own
/// invariants:
///
/// ```rust,ignore
/// struct Solvent { pool: Pubkey }
///
/// impl Check for Solvent {
///     fn check(&self, outcome: &Outcome) {
///         State::with::<Pool>(self.pool, |p| assert!(p.reserves >= p.obligations))
///             .check(outcome);
///     }
/// }
///
/// test.invariant(Solvent { pool }); // verified after every send
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

/// A comparison a numeric check applies to its observed value.
#[derive(Debug, Clone, Copy)]
enum Cmp {
    Eq,
    Le,
    Lt,
    Ge,
    Gt,
}

impl Cmp {
    fn holds(self, actual: u64, expected: u64) -> bool {
        match self {
            Self::Eq => actual == expected,
            Self::Le => actual <= expected,
            Self::Lt => actual < expected,
            Self::Ge => actual >= expected,
            Self::Gt => actual > expected,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Le => "<=",
            Self::Lt => "<",
            Self::Ge => ">=",
            Self::Gt => ">",
        }
    }
}

/// What a numeric account check reads from the resulting account.
#[derive(Debug, Clone, Copy)]
enum AccountValue {
    Lamports,
    Tokens,
    Supply,
}

impl AccountValue {
    fn label(self) -> &'static str {
        match self {
            Self::Lamports => "lamports",
            Self::Tokens => "token balance",
            Self::Supply => "mint supply",
        }
    }

    fn read(self, account: &crate::Account) -> u64 {
        match self {
            Self::Lamports => account.lamports,
            Self::Tokens => token_amount(account),
            Self::Supply => mint_supply(account),
        }
    }
}

/// A built-in check value. Every fact-namespace constructor returns this one
/// concrete type, so heterogeneous facts group in a plain array:
/// `check([CuBudget::le(5_000), Lamports::eq(vault, n)])`.
pub struct Assert(Inner);

enum Inner {
    Cu(Cmp, u64),
    Account(AccountValue, Pubkey, Cmp, u64),
    Owner(Pubkey, Pubkey),
    Data(Pubkey, Vec<u8>),
    ReturnData(Vec<u8>),
    ChangesEq(Vec<Pubkey>),
    Created(Pubkey),
    Removed(Pubkey),
    Closed(Pubkey),
    Dyn(Box<dyn Fn(&Outcome)>),
}

impl Check for Assert {
    fn check(&self, outcome: &Outcome) {
        match &self.0 {
            Inner::Cu(cmp, expected) => {
                let actual = outcome.compute_units();
                assert!(
                    cmp.holds(actual, *expected),
                    "compute units: expected {} {expected}, consumed {actual}",
                    cmp.symbol()
                );
            }
            Inner::Account(value, address, cmp, expected) => {
                let actual = value.read(required(outcome, *address));
                assert!(
                    cmp.holds(actual, *expected),
                    "{} of {address}: expected {} {expected}, got {actual}",
                    value.label(),
                    cmp.symbol()
                );
            }
            Inner::Owner(address, program) => {
                let owner = required(outcome, *address).owner;
                assert_eq!(
                    owner, *program,
                    "account {address} is owned by {owner}, expected {program}"
                );
            }
            Inner::Data(address, expected) => {
                assert_eq!(
                    required(outcome, *address).data,
                    *expected,
                    "unexpected account data for {address}"
                );
            }
            Inner::ReturnData(expected) => {
                assert_eq!(outcome.return_data(), expected, "unexpected return data");
            }
            Inner::ChangesEq(expected) => {
                let actual: Vec<Pubkey> = outcome
                    .account_changes()
                    .iter()
                    .map(|change| change.address())
                    .collect();
                assert_eq!(
                    actual, *expected,
                    "unexpected changed-account set (first-appearance order)"
                );
            }
            Inner::Created(address) => {
                assert!(
                    change(outcome, *address).was_created(),
                    "account {address} was not created by this transaction"
                );
            }
            Inner::Removed(address) => {
                assert!(
                    change(outcome, *address).was_removed(),
                    "account {address} was not removed by this transaction"
                );
            }
            Inner::Closed(address) => {
                // Solana's closed-account state: a runtime may remove the
                // account entirely or retain its empty system-owned form.
                if let Some(account) = outcome.account(*address) {
                    assert_eq!(
                        account.lamports, 0,
                        "closed account {address} still holds lamports"
                    );
                    assert!(
                        account.data.is_empty(),
                        "closed account {address} still holds data"
                    );
                    assert_eq!(
                        account.owner,
                        crate::system_program::ID,
                        "closed account {address} is not system-owned"
                    );
                }
            }
            Inner::Dyn(check) => check(outcome),
        }
    }
}

fn required(outcome: &Outcome, address: Pubkey) -> &crate::Account {
    outcome
        .account(address)
        .unwrap_or_else(|| panic!("outcome does not contain account {address}"))
}

fn change(outcome: &Outcome, address: Pubkey) -> &crate::AccountChange {
    outcome
        .account_changes()
        .iter()
        .find(|change| change.address() == address)
        .unwrap_or_else(|| panic!("this transaction did not change account {address}"))
}

macro_rules! comparators {
    ($namespace:ident, $doc:literal) => {
        impl $namespace {
            #[doc = concat!("Assert ", $doc, " equals `expected`.")]
            pub fn eq(expected: u64) -> Assert {
                Self::cmp(Cmp::Eq, expected)
            }

            #[doc = concat!("Assert ", $doc, " is at most `expected`.")]
            pub fn le(expected: u64) -> Assert {
                Self::cmp(Cmp::Le, expected)
            }

            #[doc = concat!("Assert ", $doc, " is below `expected`.")]
            pub fn lt(expected: u64) -> Assert {
                Self::cmp(Cmp::Lt, expected)
            }

            #[doc = concat!("Assert ", $doc, " is at least `expected`.")]
            pub fn ge(expected: u64) -> Assert {
                Self::cmp(Cmp::Ge, expected)
            }

            #[doc = concat!("Assert ", $doc, " is above `expected`.")]
            pub fn gt(expected: u64) -> Assert {
                Self::cmp(Cmp::Gt, expected)
            }
        }
    };
    ($namespace:ident, $value:ident, $doc:literal) => {
        impl $namespace {
            #[doc = concat!("Assert ", $doc, " equals `expected`.")]
            pub fn eq(address: Pubkey, expected: u64) -> Assert {
                Self::cmp(address, Cmp::Eq, expected)
            }

            #[doc = concat!("Assert ", $doc, " is at most `expected`.")]
            pub fn le(address: Pubkey, expected: u64) -> Assert {
                Self::cmp(address, Cmp::Le, expected)
            }

            #[doc = concat!("Assert ", $doc, " is below `expected`.")]
            pub fn lt(address: Pubkey, expected: u64) -> Assert {
                Self::cmp(address, Cmp::Lt, expected)
            }

            #[doc = concat!("Assert ", $doc, " is at least `expected`.")]
            pub fn ge(address: Pubkey, expected: u64) -> Assert {
                Self::cmp(address, Cmp::Ge, expected)
            }

            #[doc = concat!("Assert ", $doc, " is above `expected`.")]
            pub fn gt(address: Pubkey, expected: u64) -> Assert {
                Self::cmp(address, Cmp::Gt, expected)
            }
        }

        impl $namespace {
            fn cmp(address: Pubkey, cmp: Cmp, expected: u64) -> Assert {
                Assert(Inner::Account(AccountValue::$value, address, cmp, expected))
            }
        }
    };
}

/// Compute-unit budget facts: `CuBudget::le(5_000)`.
pub struct CuBudget;

impl CuBudget {
    fn cmp(cmp: Cmp, expected: u64) -> Assert {
        Assert(Inner::Cu(cmp, expected))
    }
}

comparators!(CuBudget, "the transaction's consumed compute units");

/// Lamport-balance facts: `Lamports::eq(vault, 1_000_000_000)`.
pub struct Lamports;
comparators!(
    Lamports,
    Lamports,
    "the resulting lamport balance of `address`"
);

/// Token-balance facts for Token or Token-2022 accounts:
/// `Tokens::eq(ata, 500)`.
pub struct Tokens;
comparators!(Tokens, Tokens, "the resulting token balance of `address`");

/// Mint-supply facts for Token or Token-2022 mints:
/// `Supply::eq(mint, 1_000)`.
pub struct Supply;
comparators!(
    Supply,
    Supply,
    "the resulting supply of the mint at `address`"
);

/// Account-ownership facts: `Owner::eq(vault, program_id)`.
pub struct Owner;

impl Owner {
    /// Assert the resulting account at `address` is owned by `program`.
    pub fn eq(address: Pubkey, program: Pubkey) -> Assert {
        Assert(Inner::Owner(address, program))
    }
}

/// Raw account-data facts: `Data::eq(config, [1, 0, 0, 0])`. The raw sibling
/// of [`State`], for fixed byte images and accounts without a typed schema.
pub struct Data;

impl Data {
    /// Assert the resulting account's exact raw data bytes.
    pub fn eq(address: Pubkey, expected: impl Into<Vec<u8>>) -> Assert {
        Assert(Inner::Data(address, expected.into()))
    }
}

/// Transaction return-data facts: `ReturnData::eq([7])`.
pub struct ReturnData;

impl ReturnData {
    /// Assert the transaction's exact return data.
    pub fn eq(expected: impl Into<Vec<u8>>) -> Assert {
        Assert(Inner::ReturnData(expected.into()))
    }
}

/// Typed account-state facts, decoded through `T`'s wincode schema — the same
/// decode path as [`Test::read`](crate::Test::read). Ownership is
/// intentionally not checked here; pair with [`Owner::eq`] when it matters.
pub struct State;

impl State {
    /// Assert the account at `address` decodes to exactly `expected`:
    /// `State::eq(vault, Vault { authority, amount: 600 })`. `T` is inferred
    /// from the value; failures print the full decoded/expected pair.
    pub fn eq<T>(address: Pubkey, expected: T) -> Assert
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>
            + PartialEq
            + core::fmt::Debug
            + 'static,
    {
        Assert(Inner::Dyn(Box::new(move |outcome| {
            let actual = decode::<T>(outcome, address);
            assert_eq!(actual, expected, "unexpected state for {address}");
        })))
    }

    /// Assert on the decoded state with a closure, for partial or computed
    /// facts: `State::with::<Vault>(vault, |v| assert!(v.amount > 0))`.
    pub fn with<T>(address: Pubkey, check: impl Fn(&T) + 'static) -> Assert
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T> + 'static,
    {
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(&decode::<T>(outcome, address));
        })))
    }
}

fn decode<T>(outcome: &Outcome, address: Pubkey) -> T
where
    T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
{
    let account = required(outcome, address);
    crate::world::decode::<T>("State", address, &account.data, 0)
}

/// Changed-account facts, from the transaction's writable before/after set.
pub struct Changes;

impl Changes {
    /// Assert the exact set of accounts this transaction changed, in
    /// first-appearance order: `Changes::eq([signer, vault])`.
    pub fn eq(addresses: impl Into<Vec<Pubkey>>) -> Assert {
        Assert(Inner::ChangesEq(addresses.into()))
    }

    /// Assert the transaction created the account at `address` (absent
    /// before, present after).
    pub fn created(address: Pubkey) -> Assert {
        Assert(Inner::Created(address))
    }

    /// Assert the transaction removed the account at `address` (present
    /// before, absent after).
    pub fn removed(address: Pubkey) -> Assert {
        Assert(Inner::Removed(address))
    }

    /// Assert Solana's closed-account state at `address`: the runtime may
    /// remove the account entirely or retain its empty system-owned form.
    pub fn closed(address: Pubkey) -> Assert {
        Assert(Inner::Closed(address))
    }
}
