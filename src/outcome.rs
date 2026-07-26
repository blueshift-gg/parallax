use {
    crate::{backend::ExecutionResult, Account, AccountChange, ProgramError, Pubkey},
    base64::{engine::general_purpose::STANDARD, Engine as _},
    wincode::{config::DefaultConfig, SchemaRead},
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
    /// Accepts a check struct, a closure, or an array or tuple of checks:
    /// `outcome.check([CuBudget::at_most(10_000)])`. For a check verified after
    /// *every* committed send, register it once with
    /// [`Test::invariant`](crate::Test::invariant) instead.
    pub fn check(&self, check: impl crate::Check) -> &Self {
        check.check(self);
        self
    }

    /// Assert the transaction's exact return data.
    pub fn returns(&self, expected: &[u8]) -> &Self {
        assert_eq!(
            self.return_data,
            expected,
            "unexpected return data{}",
            self.formatted_logs()
        );
        self
    }

    /// Assert a resulting account's exact raw data bytes.
    ///
    /// The raw sibling of [`Self::has_state`], for fixed byte images and
    /// accounts without a typed schema.
    pub fn has_data(&self, address: Pubkey, expected: &[u8]) -> &Self {
        assert_eq!(
            self.required_account(address).data,
            expected,
            "unexpected account data for {address}"
        );
        self
    }

    /// Assert an inclusive compute-unit ceiling. The check-value equivalent is
    /// [`CuBudget::at_most`](crate::CuBudget::at_most), which this delegates to.
    pub fn cu_at_most(&self, limit: u64) -> &Self {
        self.check(crate::CuBudget::at_most(limit))
    }

    /// Assert a resulting lamport balance.
    pub fn has_lamports(&self, address: Pubkey, expected: u64) -> &Self {
        assert_eq!(
            self.required_account(address).lamports,
            expected,
            "unexpected lamport balance for {address}"
        );
        self
    }

    /// Assert a resulting Token or Token-2022 account balance.
    pub fn has_tokens(&self, address: Pubkey, expected: u64) -> &Self {
        assert_eq!(
            token_amount(self.required_account(address)),
            expected,
            "unexpected token balance for {address}"
        );
        self
    }

    /// Assert a resulting Token or Token-2022 mint supply.
    pub fn has_supply(&self, address: Pubkey, expected: u64) -> &Self {
        assert_eq!(
            mint_supply(self.required_account(address)),
            expected,
            "unexpected mint supply for {address}"
        );
        self
    }

    /// Assert typed post-state at `address`, passing the decoded value to
    /// `check` for user assertions.
    ///
    /// The account's data is decoded through `T`'s wincode schema, the same
    /// decode path as [`Test::read`](crate::Test::read). Panics with the
    /// address and the wincode error when the account is absent or its bytes do
    /// not decode. Ownership is intentionally not checked here — pair with
    /// [`Self::owned_by`] when that matters. This differs from the TypeScript
    /// harness by design: TS codecs carry and validate `owner` because generated
    /// bundles are self-framing; in Rust owner stays an orthogonal `owned_by`
    /// assertion. Chainable, so several accounts can be asserted in one
    /// expression.
    pub fn has_state<T>(&self, address: Pubkey, check: impl FnOnce(&T)) -> &Self
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
    {
        let name = core::any::type_name::<T>();
        let account = self.account(address).unwrap_or_else(|| {
            panic!("has_state {name}: outcome does not contain account {address}")
        });
        let state = crate::world::decode::<T>("has_state", address, &account.data, 0);
        check(&state);
        self
    }

    /// Assert a resulting account is owned by `program`.
    ///
    /// Orthogonal to [`Self::has_state`], which decodes without checking
    /// ownership. Chainable.
    pub fn owned_by(&self, address: Pubkey, program: Pubkey) -> &Self {
        let account = self.required_account(address);
        assert_eq!(
            account.owner, program,
            "account {address} is owned by {}, expected {program}",
            account.owner
        );
        self
    }

    /// Assert Solana's closed-account state. A runtime may remove the account
    /// entirely or retain its empty system-owned representation.
    pub fn is_closed(&self, address: Pubkey) -> &Self {
        if let Some(account) = self.account(address) {
            assert_eq!(account.lamports, 0, "closed account still holds lamports");
            assert!(account.data.is_empty(), "closed account still holds data");
            assert_eq!(
                account.owner,
                crate::system_program::ID,
                "closed account is not system-owned"
            );
        }
        self
    }

    fn required_account(&self, address: Pubkey) -> &Account {
        self.account(address)
            .unwrap_or_else(|| panic!("outcome does not contain account {address}"))
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
    use {super::*, crate::CuBudget};

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

    #[test]
    fn compute_unit_ceiling_is_inclusive() {
        outcome(&[], 10).cu_at_most(10);
    }

    #[test]
    fn returns_asserts_exact_return_data() {
        outcome(&[], 0).returns(&[9, 8, 7]);
    }

    #[test]
    #[should_panic(expected = "unexpected return data")]
    fn returns_rejects_different_bytes() {
        outcome(&[], 0).returns(&[9, 8]);
    }

    // A check value runs through `check` in every accepted shape: a struct
    // (`CuBudget`), a closure, and an array grouping several.
    #[test]
    fn check_accepts_structs_closures_and_arrays() {
        let ran = core::cell::Cell::new(false);
        outcome(&[], 10)
            .check(CuBudget::at_most(10))
            .check(|o: &Outcome| assert_eq!(o.compute_units(), 10))
            .check([CuBudget::at_most(10), CuBudget::at_most(11)])
            .check(|_: &Outcome| ran.set(true));
        assert!(ran.get(), "closure checks must run");
    }

    #[test]
    #[should_panic(expected = "CU budget exceeded")]
    fn check_surfaces_a_failing_budget() {
        outcome(&[], 10).check(CuBudget::at_most(9));
    }

    // A minimal wincode account type: a discriminator-free struct whose schema
    // covers the whole account, enough to exercise the typed `has_state` and
    // `owned_by` paths without a program.
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
    fn has_state_decodes_and_checks_typed_post_state() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 7, tag: 3 },
        ));

        let mut checked = false;
        outcome
            .has_state::<Counter>(address, |value| {
                assert_eq!(value.count, 7);
                assert_eq!(value.tag, 3);
                checked = true;
            })
            .cu_at_most(0);
        assert!(checked, "the check closure should run against the state");
    }

    #[test]
    #[should_panic(expected = "has_state")]
    fn has_state_panics_when_bytes_do_not_decode() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        // Too few bytes for a Counter (needs nine), so the wincode decode fails.
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.has_state::<Counter>(address, |_| {
            unreachable!("the decode fails before the check runs")
        });
    }

    #[test]
    fn has_data_asserts_exact_raw_bytes() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.has_data(address, &[1, 2, 3]);
    }

    #[test]
    #[should_panic(expected = "unexpected account data")]
    fn has_data_rejects_different_bytes() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(Account::new(address, owner, 42, vec![1, 2, 3]));

        outcome.has_data(address, &[1, 2]);
    }

    #[test]
    fn owned_by_asserts_account_ownership() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 1, tag: 0 },
        ));

        outcome
            .owned_by(address, owner)
            .has_state::<Counter>(address, |value| assert_eq!(value.count, 1));
    }

    #[test]
    #[should_panic(expected = "owned by")]
    fn owned_by_rejects_the_wrong_owner() {
        let address = Pubkey::new_from_array([5; 32]);
        let owner = Pubkey::new_from_array([9; 32]);
        let foreign = Pubkey::new_from_array([1; 32]);
        let outcome = state_outcome(counter_account(
            address,
            owner,
            &Counter { count: 1, tag: 0 },
        ));

        outcome.owned_by(address, foreign);
    }
}
