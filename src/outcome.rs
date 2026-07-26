use {
    crate::{backend::ExecutionResult, Account, AccountChange, ProgramError, Pubkey},
    base64::{engine::general_purpose::STANDARD, Engine as _},
};

pub(crate) struct TrackedAccount {
    pub(crate) address: Pubkey,
    pub(crate) writable: bool,
    pub(crate) signer: bool,
    pub(crate) before: Option<Account>,
    pub(crate) after: Option<Account>,
}

/// The structured result of executing one transaction.
///
/// `Outcome` owns the stable data tests normally need: the program error,
/// logs, return data, compute units, resulting accounts, and writable account
/// changes. The runtime's internal result type is intentionally private.
#[must_use = "assert the outcome with succeeds, fails, or fails_with"]
pub struct Outcome {
    error: Option<ProgramError>,
    compute_units: u64,
    logs: Vec<String>,
    return_data: Vec<u8>,
    accounts: Vec<Account>,
    changes: Vec<AccountChange>,
    hint: Option<String>,
}

impl Outcome {
    pub(crate) fn from_backend(result: ExecutionResult, tracked: Vec<TrackedAccount>) -> Self {
        // Tracked addresses are unique, so the resulting set needs no dedup, and
        // its order is never observed (accounts are looked up by address). A
        // single pass moves each post-state into the resulting set, cloning only
        // the changed writable accounts that also feed an `AccountChange`.
        let mut accounts = Vec::with_capacity(tracked.len());
        let mut changes = Vec::new();
        for account in tracked {
            let changed = account.writable && account.before != account.after;
            match account.after {
                Some(after) if changed => {
                    accounts.push(after.clone());
                    changes.push(AccountChange::new(
                        account.address,
                        account.before,
                        Some(after),
                    ));
                }
                Some(after) => accounts.push(after),
                None if changed => {
                    changes.push(AccountChange::new(account.address, account.before, None));
                }
                None => {}
            }
        }

        Self {
            error: result.error,
            compute_units: result.compute_units_consumed,
            logs: result.logs,
            return_data: result.return_data,
            accounts,
            changes,
            hint: None,
        }
    }

    /// Attach a guided-error hint (e.g. a missing dumped account), surfaced by
    /// the failure assertions. See [`crate::fixture::Dump`].
    pub(crate) fn with_hint(mut self, hint: Option<String>) -> Self {
        self.hint = hint;
        self
    }

    pub(crate) fn simulated_account(result: &ExecutionResult, address: &Pubkey) -> Option<Account> {
        result.account(address).cloned()
    }

    /// The guided-error hint attached to this outcome, if any. Exposed for the
    /// FFI wire and the failure assertions; not part of the stable surface.
    #[doc(hidden)]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// Whether execution succeeded.
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// Whether execution failed.
    pub fn is_err(&self) -> bool {
        self.error.is_some()
    }

    /// The execution error, if any.
    pub fn error(&self) -> Option<&ProgramError> {
        self.error.as_ref()
    }

    /// Compute units consumed by the transaction.
    pub fn compute_units(&self) -> u64 {
        self.compute_units
    }

    /// Program logs in execution order.
    pub fn logs(&self) -> &[String] {
        &self.logs
    }

    /// Raw Solana return data.
    pub fn return_data(&self) -> &[u8] {
        &self.return_data
    }

    /// Decode return data with a generated or application-provided decoder.
    pub fn return_value<T>(&self, decode: impl FnOnce(&[u8]) -> Option<T>) -> Option<T> {
        decode(&self.return_data)
    }

    /// The resulting account state for an address involved in the transaction.
    pub fn account(&self, address: Pubkey) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|account| account.address == address)
    }

    /// Every tracked account's resulting post-state, in first-appearance
    /// instruction order.
    ///
    /// Accounts that no longer exist after execution (closed or never created)
    /// are omitted, as they have no post-state; a removed writable account
    /// still surfaces through [`Self::account_changes`] with a `None` after.
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// Decode a resulting account with a caller-supplied decoder.
    pub fn account_as<T>(
        &self,
        address: Pubkey,
        decode: impl FnOnce(&[u8]) -> Option<T>,
    ) -> Option<T> {
        self.account(address)
            .and_then(|account| decode(&account.data))
    }

    /// Writable account changes, in first-appearance instruction order.
    pub fn account_changes(&self) -> &[AccountChange] {
        &self.changes
    }

    /// Decode every matching `sol_log_data` payload with a generated client
    /// event decoder. Unrelated program-data logs are ignored.
    pub fn events<T>(&self, decode: impl Fn(&[u8]) -> Option<T>) -> Vec<T> {
        self.logs
            .iter()
            .filter_map(|log| log.strip_prefix("Program data: "))
            .filter_map(|encoded| STANDARD.decode(encoded).ok())
            .filter_map(|bytes| decode(&bytes))
            .collect()
    }

    /// Assert success and keep the outcome available for chained assertions.
    pub fn succeeds(&self) -> &Self {
        if let Some(error) = &self.error {
            panic!("expected success, got {error}{}", self.formatted_logs());
        }
        self
    }

    /// Assert a typed custom program error.
    pub fn fails_with<E>(&self, expected: E) -> &Self
    where
        E: Into<u32>,
    {
        self.fails(ProgramError::Custom(expected.into()))
    }

    /// Assert a runtime or non-custom program error.
    pub fn fails(&self, expected: ProgramError) -> &Self {
        assert_eq!(
            self.error.as_ref(),
            Some(&expected),
            "unexpected execution outcome{}",
            self.formatted_logs()
        );
        self
    }

    /// Run a reusable [`Check`](crate::Check) against this outcome. Chainable.
    ///
    /// Accepts a built-in fact (`check([Cu::spent().le(5_000), Account::lamports(vault).eq(n)])`),
    /// a struct implementing [`Check`](crate::Check), a closure, or an array
    /// or tuple of checks. For a check verified after *every* committed send,
    /// register it once with [`Test::invariant`](crate::Test::invariant)
    /// instead.
    pub fn check(&self, check: impl crate::Check) -> &Self {
        check.check(self);
        self
    }

    fn formatted_logs(&self) -> String {
        let mut formatted = if self.logs.is_empty() {
            String::new()
        } else {
            format!("\nprogram logs:\n  {}", self.logs.join("\n  "))
        };
        if let Some(hint) = &self.hint {
            formatted.push_str("\nhint: ");
            formatted.push_str(hint);
        }
        formatted
    }
}

pub(crate) fn token_amount(account: &Account) -> u64 {
    use spl_token::{solana_program::program_pack::Pack, state::Account as TokenAccount};

    TokenAccount::unpack(&account.data)
        .unwrap_or_else(|error| {
            panic!(
                "could not decode {} as a token account: {error}",
                account.address
            )
        })
        .amount
}

pub(crate) fn mint_supply(account: &Account) -> u64 {
    use spl_token::{solana_program::program_pack::Pack, state::Mint};

    Mint::unpack(&account.data)
        .unwrap_or_else(|error| {
            panic!(
                "could not decode {} as a token mint: {error}",
                account.address
            )
        })
        .supply
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{Changes, Cu, ReturnData},
    };

    fn outcome(logs: &[&str], compute_units: u64) -> Outcome {
        Outcome {
            error: None,
            compute_units,
            logs: logs.iter().map(ToString::to_string).collect(),
            return_data: vec![9, 8, 7],
            accounts: Vec::new(),
            changes: Vec::new(),
            hint: None,
        }
    }

    #[test]
    fn event_decoding_ignores_unrelated_and_malformed_logs() {
        let outcome = outcome(
            &[
                "Program log: before",
                "Program data: AQID",
                "Program data: not-base64",
                "Program log: after",
            ],
            10,
        );

        assert_eq!(
            outcome.events(|bytes| (bytes.first() == Some(&1)).then(|| bytes.to_vec())),
            [vec![1, 2, 3]]
        );
        assert_eq!(outcome.return_value(|bytes| Some(bytes[1])), Some(8));
    }

    // Every comparator holds and rejects with its own boundary.
    #[test]
    fn cu_comparators_cover_their_boundaries() {
        let ten = outcome(&[], 10);
        ten.check([
            Cu::spent().eq(10),
            Cu::spent().le(10),
            Cu::spent().lt(11),
            Cu::spent().ge(10),
            Cu::spent().gt(9),
        ]);
    }

    #[test]
    #[should_panic(expected = "compute units: expected <= 9, consumed 10")]
    fn cu_le_rejects_over_budget() {
        outcome(&[], 10).check(Cu::spent().le(9));
    }

    #[test]
    fn return_data_asserts_exact_bytes() {
        outcome(&[], 0).check(ReturnData::eq([9, 8, 7]));
    }

    #[test]
    #[should_panic(expected = "unexpected return data")]
    fn return_data_rejects_different_bytes() {
        outcome(&[], 0).check(ReturnData::eq([9, 8]));
    }

    // `Assert::from_fn` lifts a closure into the built-ins' concrete type, so
    // application facts group in the same arrays.
    #[test]
    fn from_fn_asserts_group_with_built_ins() {
        outcome(&[], 10).check([
            Cu::spent().le(10),
            crate::Assert::from_fn(|o| assert_eq!(o.compute_units(), 10)),
        ]);
    }

    // A check value runs through `check` in every accepted shape: built-in
    // facts, a closure, and an array grouping several.
    #[test]
    fn check_accepts_facts_closures_and_arrays() {
        let ran = core::cell::Cell::new(false);
        outcome(&[], 10)
            .check(Cu::spent().eq(10))
            .check(|o: &Outcome| assert_eq!(o.compute_units(), 10))
            .check([Cu::spent().le(10), Cu::spent().le(11)])
            .check(|_: &Outcome| ran.set(true));
        assert!(ran.get(), "closure checks must run");
    }

    // A minimal wincode account type: a discriminator-free struct whose schema
    // covers the whole account, enough to exercise the typed `State` and
    // `Owner` checks without a program.
    #[derive(wincode::SchemaRead, wincode::SchemaWrite, Clone, PartialEq, Debug)]
    struct Counter {
        count: u64,
        tag: u8,
    }

    fn state_outcome(account: Account) -> Outcome {
        Outcome {
            error: None,
            compute_units: 0,
            logs: Vec::new(),
            return_data: Vec::new(),
            accounts: vec![account],
            changes: Vec::new(),
            hint: None,
        }
    }

    fn counter_account(address: Pubkey, owner: Pubkey, counter: &Counter) -> Account {
        let data = wincode::serialize(counter).expect("Counter serializes");
        Account::new(address, owner, 42, data)
    }

    #[test]
    fn state_checks_decode_typed_post_state() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 7, tag: 3 },
        ));

        outcome.check([
            Account::state(address).eq(Counter { count: 7, tag: 3 }),
            Account::state(address).with::<Counter>(|value| assert_eq!(value.count, 7)),
        ]);
    }

    #[test]
    #[should_panic(expected = "did not decode")]
    fn state_checks_panic_when_bytes_do_not_decode() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        // Too few bytes for a Counter (needs nine), so the wincode decode fails.
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.check(
            Account::state(address)
                .with::<Counter>(|_| unreachable!("the decode fails before the check runs")),
        );
    }

    #[test]
    fn lamports_comparators_read_the_resulting_account() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(Account::new(address, owner, 42, Vec::new()));

        outcome.check([
            Account::lamports(address).eq(42),
            Account::lamports(address).ge(40),
        ]);
    }

    #[test]
    #[should_panic(expected = "outcome does not contain account")]
    fn account_facts_name_a_missing_account() {
        let missing = Pubkey::new_from_array([8; 32]);
        outcome(&[], 0).check(Account::lamports(missing).eq(1));
    }

    #[test]
    fn changes_facts_read_the_change_set() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let created = Account::new(address, owner, 42, Vec::new());
        let mut outcome = state_outcome(created.clone());
        outcome.changes = vec![crate::AccountChange::new(address, None, Some(created))];

        outcome.check([Changes::eq([address]), Changes::created(address)]);
    }

    #[test]
    #[should_panic(expected = "did not change account")]
    fn changes_facts_name_an_untouched_account() {
        outcome(&[], 0).check(Changes::created(Pubkey::new_from_array([8; 32])));
    }

    #[test]
    fn data_check_asserts_exact_raw_bytes() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.check(Account::data(address).eq([1, 2, 3]));
    }

    #[test]
    #[should_panic(expected = "unexpected account data")]
    fn data_check_rejects_different_bytes() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.check(Account::data(address).eq([1, 2]));
    }

    #[test]
    fn owner_check_asserts_account_ownership() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 1, tag: 0 },
        ));

        outcome.check((
            Account::owner(address).eq(owner),
            Account::state(address).with::<Counter>(|value| assert_eq!(value.count, 1)),
        ));
    }

    #[test]
    #[should_panic(expected = "owned by")]
    fn owner_check_rejects_the_wrong_owner() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let foreign = Pubkey::new_from_array([1; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 1, tag: 0 },
        ));

        outcome.check(Account::owner(address).eq(foreign));
    }
}
