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
//!     Cu::spent().le(5_000),
//!     Account::lamports(vault).eq(amount),
//!     Account::lamports(signer).with(|l| assert!(l > 0)),
//!     Account::created(vault),
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
/// Built-in checks follow one shape — name the fact, bind its subject, then
/// compare: [`Cu::spent`], [`Account::lamports`](crate::Account::lamports) and
/// its siblings (`owner`/`data`/`state`/`created`/`removed`/`closed`),
/// [`TokenAccount::amount`](crate::fixture::TokenAccount::amount)/[`Mint::supply`](crate::fixture::Mint::supply),
/// plus the transaction-scoped [`ReturnData`]. Every bound fact compares
/// (`eq`, and `le`/`lt`/`ge`/`gt` where numeric) or pipes its measured value
/// into a closure (`with`). Closures over the whole outcome,
/// arrays, and tuples of checks all qualify, and applications implement the
/// trait to name their own invariants:
///
/// ```rust,ignore
/// struct Solvent { pool: Pubkey }
///
/// impl Check for Solvent {
///     fn check(&self, outcome: &Outcome) {
///         Account::state(self.pool)
///             .with::<Pool>(|p| assert!(p.reserves >= p.obligations))
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
/// `check([Cu::spent().le(5_000), Account::lamports(vault).eq(n)])`.
pub struct Assert(Inner);

impl Assert {
    /// Wrap a closure as an `Assert`, so application-defined facts group in
    /// the same arrays as the built-ins. This is how a crate builds its own
    /// fact namespace on top of parallax.
    pub fn from_fn(check: impl Fn(&Outcome) + 'static) -> Self {
        Self(Inner::Dyn(Box::new(check)))
    }
}

enum Inner {
    Cu(Cmp, u64),
    Account(AccountValue, Pubkey, Cmp, u64),
    Owner(Pubkey, Pubkey),
    Data(Pubkey, Vec<u8>),
    ReturnData(Vec<u8>),
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

/// A bound numeric fact awaiting its comparator: `Account::lamports(vault)`
/// yields a `Measure`, and `.eq(n)`/`.le(n)`/`.lt(n)`/`.ge(n)`/`.gt(n)`
/// finish it into an [`Assert`].
pub struct Measure(MeasureSource);

#[derive(Clone, Copy)]
enum MeasureSource {
    Cu,
    Account(AccountValue, Pubkey),
}

impl MeasureSource {
    fn read(self, outcome: &Outcome) -> u64 {
        match self {
            Self::Cu => outcome.compute_units(),
            Self::Account(value, address) => value.read(required(outcome, address)),
        }
    }
}

impl Measure {
    fn finish(self, cmp: Cmp, expected: u64) -> Assert {
        Assert(match self.0 {
            MeasureSource::Cu => Inner::Cu(cmp, expected),
            MeasureSource::Account(value, address) => Inner::Account(value, address, cmp, expected),
        })
    }

    /// Assert the measured value equals `expected`.
    pub fn eq(self, expected: u64) -> Assert {
        self.finish(Cmp::Eq, expected)
    }

    /// Assert the measured value is at most `expected`.
    pub fn le(self, expected: u64) -> Assert {
        self.finish(Cmp::Le, expected)
    }

    /// Assert the measured value is below `expected`.
    pub fn lt(self, expected: u64) -> Assert {
        self.finish(Cmp::Lt, expected)
    }

    /// Assert the measured value is at least `expected`.
    pub fn ge(self, expected: u64) -> Assert {
        self.finish(Cmp::Ge, expected)
    }

    /// Assert the measured value is above `expected`.
    pub fn gt(self, expected: u64) -> Assert {
        self.finish(Cmp::Gt, expected)
    }

    /// Pipe the measured value into a closure, for facts the comparators
    /// cannot spell: `Account::lamports(user).with(|l| assert!(l > rent))`.
    pub fn with(self, check: impl Fn(u64) + 'static) -> Assert {
        let source = self.0;
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(source.read(outcome))
        })))
    }
}

/// Compute-unit facts: `Cu::spent().le(5_000)`.
pub struct Cu;

impl Cu {
    /// The compute units the transaction consumed.
    pub fn spent() -> Measure {
        Measure(MeasureSource::Cu)
    }
}

/// The token-account fact, hung off the [`TokenAccount`](crate::fixture::TokenAccount)
/// fixture type itself: one noun installs token accounts and measures them.
/// Reads Token or Token-2022 accounts.
impl crate::fixture::TokenAccount {
    /// The token balance of the token account at `address`:
    /// `TokenAccount::amount(ata).ge(500)`.
    pub fn amount(address: Pubkey) -> Measure {
        Measure(MeasureSource::Account(AccountValue::Tokens, address))
    }
}

/// The mint fact, hung off the [`Mint`](crate::fixture::Mint) fixture type
/// itself. Reads Token or Token-2022 mints.
impl crate::fixture::Mint {
    /// The supply of the mint at `address`: `Mint::supply(mint).eq(1_000)`.
    pub fn supply(address: Pubkey) -> Measure {
        Measure(MeasureSource::Account(AccountValue::Supply, address))
    }
}

/// The account-scoped facts, hung off the [`Account`](crate::Account) type
/// itself: one noun installs raw accounts and measures them.
impl crate::Account {
    /// The lamport balance of the account at `address`.
    pub fn lamports(address: Pubkey) -> Measure {
        Measure(MeasureSource::Account(AccountValue::Lamports, address))
    }

    /// The owner of the account at `address`: `Account::owner(vault).eq(program_id)`.
    pub fn owner(address: Pubkey) -> OwnerMeasure {
        OwnerMeasure(address)
    }

    /// The raw data bytes of the account at `address` — the raw sibling of
    /// [`Self::state`].
    pub fn data(address: Pubkey) -> DataMeasure {
        DataMeasure(address)
    }

    /// The typed state of the account at `address`, decoded through the
    /// type's wincode schema — the same decode path as
    /// [`Test::read`](crate::Test::read). Ownership is intentionally not
    /// checked; pair with [`Self::owner`] when it matters.
    pub fn state(address: Pubkey) -> StateMeasure {
        StateMeasure(address)
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

/// A bound account-owner fact awaiting its expected program.
pub struct OwnerMeasure(Pubkey);

impl OwnerMeasure {
    /// Assert the account is owned by `program`.
    pub fn eq(self, program: Pubkey) -> Assert {
        Assert(Inner::Owner(self.0, program))
    }

    /// Pipe the owner into a closure.
    pub fn with(self, check: impl Fn(Pubkey) + 'static) -> Assert {
        let address = self.0;
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(required(outcome, address).owner)
        })))
    }
}

/// A bound raw-data fact awaiting its expected bytes.
pub struct DataMeasure(Pubkey);

impl DataMeasure {
    /// Assert the account's exact raw data bytes.
    pub fn eq(self, expected: impl Into<Vec<u8>>) -> Assert {
        Assert(Inner::Data(self.0, expected.into()))
    }

    /// Pipe the raw data bytes into a closure.
    pub fn with(self, check: impl Fn(&[u8]) + 'static) -> Assert {
        let address = self.0;
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(&required(outcome, address).data)
        })))
    }
}

/// A bound typed-state fact awaiting its expected value or closure.
pub struct StateMeasure(Pubkey);

impl StateMeasure {
    /// Assert the account decodes to exactly `expected`:
    /// `Account::state(vault).eq(Vault { authority, amount: 600 })`. `T` is
    /// inferred from the value; failures print the full decoded/expected pair.
    pub fn eq<T>(self, expected: T) -> Assert
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>
            + PartialEq
            + core::fmt::Debug
            + 'static,
    {
        let address = self.0;
        Assert(Inner::Dyn(Box::new(move |outcome| {
            let actual = decode::<T>(outcome, address);
            assert_eq!(actual, expected, "unexpected state for {address}");
        })))
    }

    /// Assert on the decoded state with a closure, for partial or computed
    /// facts: `Account::state(vault).with::<Vault>(|v| assert!(v.amount > 0))`.
    pub fn with<T>(self, check: impl Fn(&T) + 'static) -> Assert
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T> + 'static,
    {
        let address = self.0;
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(&decode::<T>(outcome, address));
        })))
    }
}

/// Transaction return-data facts: `ReturnData::eq([7])`.
pub struct ReturnData;

impl ReturnData {
    /// Assert the transaction's exact return data.
    pub fn eq(expected: impl Into<Vec<u8>>) -> Assert {
        Assert(Inner::ReturnData(expected.into()))
    }

    /// Pipe the return data into a closure.
    pub fn with(check: impl Fn(&[u8]) + 'static) -> Assert {
        Assert(Inner::Dyn(Box::new(move |outcome| {
            check(outcome.return_data())
        })))
    }
}

fn decode<T>(outcome: &Outcome, address: Pubkey) -> T
where
    T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
{
    let account = required(outcome, address);
    crate::world::decode::<T>("state", address, &account.data, 0)
}
