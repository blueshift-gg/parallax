use {
    crate::{
        accounts,
        backend::Backend,
        fixture::{Fixture, TokenProgram},
        outcome::{mint_supply, token_amount, TrackedAccount},
        Account, Instruction, Outcome, Pubkey, SetupError, TestBuilder,
    },
    std::{ops::Deref, path::Path},
    wincode::{config::DefaultConfig, SchemaRead, SchemaWrite},
};

/// Default balance assigned by [`crate::fixture::Wallet`]: ten SOL.
pub const DEFAULT_WALLET_LAMPORTS: u64 = 10_000_000_000;

/// An isolated Solana program test world.
///
/// Each `Test` owns its runtime and account state. The public API describes
/// test behavior rather than a particular SVM, so the same test can be hosted
/// by additional runtimes without becoming generic over a backend.
pub struct Test {
    pub(super) backend: Backend,
    pub(super) program_id: Pubkey,
    pub(super) program_path: std::path::PathBuf,
    pub(super) fresh_addresses: u64,
}

impl Test {
    /// Load a compiled program by discovering its artifact.
    ///
    /// # Panics
    ///
    /// Panics with an actionable message when no program artifact can be
    /// located or read. Use [`Self::try_new`] to handle setup errors.
    pub fn new(program_id: impl Into<Pubkey>) -> Self {
        Self::builder(program_id)
            .build()
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// Fallible variant of [`Self::new`].
    pub fn try_new(program_id: impl Into<Pubkey>) -> Result<Self, SetupError> {
        Self::builder(program_id).build()
    }

    /// Configure artifact discovery and runtime limits before loading a world.
    pub fn builder(program_id: impl Into<Pubkey>) -> TestBuilder {
        TestBuilder::new(program_id.into())
    }

    /// The primary program under test.
    pub fn program_id(&self) -> Pubkey {
        self.program_id
    }

    /// The ELF artifact loaded for the primary program.
    pub fn program_path(&self) -> &Path {
        &self.program_path
    }

    /// Install a built-in or application-defined fixture.
    pub fn add<F: Fixture>(&mut self, fixture: F) -> F::Output {
        fixture.install(self)
    }

    /// Insert or replace a raw account.
    pub fn set_account(&mut self, account: Account) {
        self.backend.set_account(account);
    }

    /// Read a raw account from the current world.
    pub fn account(&self, address: Pubkey) -> Option<Account> {
        self.backend.account(&address)
    }

    /// Decode an account with a caller-supplied decoder (e.g. a generated
    /// client's decode function).
    pub fn account_as<T>(
        &self,
        address: Pubkey,
        decode: impl FnOnce(&[u8]) -> Option<T>,
    ) -> Option<T> {
        self.account(address)
            .and_then(|account| decode(&account.data))
    }

    /// Preload a program for cross-program invocations.
    pub fn load_program(&mut self, program_id: Pubkey, elf: &[u8]) {
        self.backend.load_program(&program_id, elf);
    }

    /// Produce a deterministic address unused by earlier fixtures in this
    /// world. The sequence is independent for every test, and identical across
    /// the Rust and TypeScript harnesses so fixture addresses match. Internal:
    /// fixtures call this to place accounts the caller did not pin; tests name
    /// actors through [`crate::fixture::Wallet`] and read back the address.
    pub(crate) fn fresh_address(&mut self) -> Pubkey {
        self.fresh_addresses += 1;
        let mut bytes = *b"parallax/fresh-address\0\0\0\0\0\0\0\0\0\0";
        bytes[24..].copy_from_slice(&self.fresh_addresses.to_le_bytes());
        Pubkey::new_from_array(bytes)
    }

    /// Derive an associated-token address without installing the account.
    pub fn derive_ata(&self, owner: Pubkey, mint: Pubkey, token_program: TokenProgram) -> Pubkey {
        Pubkey::find_program_address(
            &[owner.as_ref(), token_program.id().as_ref(), mint.as_ref()],
            &crate::SPL_ASSOCIATED_TOKEN_PROGRAM_ID,
        )
        .0
    }

    /// Derive a program-derived address under the program under test from raw
    /// seed slices.
    pub fn derive_pda(&self, seeds: &[&[u8]]) -> Pubkey {
        self.derive_pda_with_bump(seeds).0
    }

    /// Derive a program-derived address and its canonical bump under the
    /// program under test from raw seed slices.
    pub fn derive_pda_with_bump(&self, seeds: &[&[u8]]) -> (Pubkey, u8) {
        Pubkey::find_program_address(seeds, &self.program_id)
    }

    /// Decode the account at `address` with wincode's [`DefaultConfig`].
    ///
    /// `T`'s wincode schema is applied to the account's full data. A generated
    /// client account type encodes its discriminator as the leading schema
    /// field, so an on-chain account round-trips directly — the same bytes a
    /// program wrote, decoded back into the client type. For a type whose
    /// schema covers only a suffix of the data, use [`Self::read_at`].
    ///
    /// # Panics
    ///
    /// Panics with the address and the wincode error when no account exists at
    /// `address` or its bytes do not decode as `T`.
    pub fn read<T>(&self, address: Pubkey) -> Snapshot<T>
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
    {
        self.read_at(address, 0)
    }

    /// Decode the account at `address` starting `offset` bytes in.
    ///
    /// Use when `T`'s schema describes only a suffix of the account — for
    /// example decoding the fields after a discriminator the caller frames
    /// separately.
    ///
    /// # Panics
    ///
    /// Panics like [`Self::read`], and additionally when the account holds
    /// fewer than `offset` bytes.
    pub fn read_at<T>(&self, address: Pubkey, offset: usize) -> Snapshot<T>
    where
        T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
    {
        let account = self.account(address).unwrap_or_else(|| {
            panic!(
                "read {}: no account at {address}",
                core::any::type_name::<T>()
            )
        });
        let state = decode::<T>("read", address, &account.data, offset);
        Snapshot {
            address,
            lamports: account.lamports,
            state,
        }
    }

    /// Serialize `value` with wincode and install it as a rent-exempt account
    /// owned by `owner`.
    ///
    /// The value's schema frames the account exactly as the program writes it:
    /// a generated account type emits its discriminator as the leading schema
    /// field, so no separate framing is needed. `owner` is explicit because the
    /// serialization substrate carries no ownership of its own. Returns
    /// `address`.
    pub fn write<T>(&mut self, address: Pubkey, owner: Pubkey, value: T) -> Pubkey
    where
        T: SchemaWrite<DefaultConfig, Src = T>,
    {
        let data = wincode::serialize(&value)
            .unwrap_or_else(|error| panic!("write {}: {error:?}", core::any::type_name::<T>()));
        self.set_account(accounts::program_account(address, owner, data));
        address
    }

    /// An account's current lamport balance.
    pub fn lamports(&self, address: Pubkey) -> u64 {
        self.required_account(address).lamports
    }

    /// A base Token or Token-2022 account's current amount.
    pub fn tokens(&self, address: Pubkey) -> u64 {
        token_amount(&self.required_account(address))
    }

    /// A base Token or Token-2022 mint's current supply.
    pub fn supply(&self, address: Pubkey) -> u64 {
        mint_supply(&self.required_account(address))
    }

    /// Set the runtime clock's Unix timestamp.
    pub fn warp_to_timestamp(&mut self, timestamp: i64) {
        self.backend.warp_to_timestamp(timestamp);
    }

    /// Execute and commit one instruction.
    pub fn send(&mut self, instruction: impl Into<Instruction>) -> Outcome {
        self.execute([instruction.into()], Vec::new(), true)
    }

    /// Execute and commit an atomic instruction sequence.
    pub fn send_all<I, T>(&mut self, instructions: I) -> Outcome
    where
        I: IntoIterator<Item = T>,
        T: Into<Instruction>,
    {
        self.execute(
            instructions.into_iter().map(Into::into).collect::<Vec<_>>(),
            Vec::new(),
            true,
        )
    }

    /// Execute and commit one instruction with raw transaction-input
    /// accounts. Fixtures installed in the world normally make this
    /// unnecessary; it remains useful when malformed input is the test case.
    pub fn send_with(
        &mut self,
        instruction: impl Into<Instruction>,
        accounts: impl IntoIterator<Item = Account>,
    ) -> Outcome {
        self.execute([instruction.into()], accounts.into_iter().collect(), true)
    }

    /// Execute an instruction without committing its changes.
    pub fn simulate(&mut self, instruction: impl Into<Instruction>) -> Outcome {
        self.execute([instruction.into()], Vec::new(), false)
    }

    fn execute(
        &mut self,
        instructions: impl AsRef<[Instruction]>,
        mut inputs: Vec<Account>,
        commit: bool,
    ) -> Outcome {
        let instructions = instructions.as_ref();
        assert!(
            !instructions.is_empty(),
            "a transaction needs an instruction"
        );
        assert_unique_accounts(&inputs);

        let mut tracked = Vec::<TrackedAccount>::new();
        for instruction in instructions {
            for meta in &instruction.accounts {
                if let Some(existing) = tracked
                    .iter_mut()
                    .find(|account| account.address == meta.pubkey)
                {
                    existing.writable |= meta.is_writable;
                    existing.signer |= meta.is_signer;
                    continue;
                }
                let before = inputs
                    .iter()
                    .find(|account| account.address == meta.pubkey)
                    .cloned()
                    .or_else(|| self.backend.account(&meta.pubkey));
                tracked.push(TrackedAccount {
                    address: meta.pubkey,
                    writable: meta.is_writable,
                    signer: meta.is_signer,
                    before,
                    after: None,
                });
            }
        }

        for input in &inputs {
            if tracked
                .iter()
                .all(|account| account.address != input.address)
            {
                tracked.push(TrackedAccount {
                    address: input.address,
                    writable: false,
                    signer: false,
                    before: Some(input.clone()),
                    after: None,
                });
            }
        }

        // Backfill accounts a transaction names but the world has not
        // installed. A missing writable account is an init target — including
        // keypair accounts that sign their own creation — and enters as
        // Solana's empty system account, exactly as a brand-new keypair
        // account arrives on chain; the SVM commits that input only when
        // execution succeeds, so init targets persist without polluting the
        // world after a failed transaction. A missing read-only signer (a
        // co-signer, e.g. a multisig member) enters as a funded system
        // account, matching the real wallets those signatures come from.
        // Actors that pay — payers, makers — are world state: install them
        // with [`crate::fixture::Wallet`].
        for account in &tracked {
            // A present pre-state means the account is installed or was supplied
            // as an explicit input, so it needs no backfill. Every remaining
            // tracked address is unique and absent from `inputs`.
            if account.before.is_some() {
                continue;
            }
            if account.writable {
                inputs.push(accounts::empty_account(account.address));
            } else if account.signer {
                inputs.push(accounts::system_account(
                    account.address,
                    DEFAULT_WALLET_LAMPORTS,
                ));
            }
        }

        let result = self.backend.execute(instructions, &inputs, commit);
        let succeeded = result.is_ok();
        for account in &mut tracked {
            account.after = if !succeeded {
                account.before.clone()
            } else if commit {
                // An installed read-only account cannot change, so its committed
                // post-state is its pre-state and needs no read back. Writable
                // accounts may have changed, and a backfilled read-only signer (a
                // co-signer with no pre-state) was seeded funded, so both are read
                // from the store.
                if account.writable || (account.signer && account.before.is_none()) {
                    self.backend.account(&account.address)
                } else {
                    account.before.clone()
                }
            } else {
                // Simulation reports only writable accounts; a read-only
                // account cannot change, so its pre-state is its post-state.
                Outcome::simulated_account(&result, &account.address)
                    .or_else(|| account.before.clone())
            };
        }
        Outcome::from_backend(result, tracked)
    }

    fn required_account(&self, address: Pubkey) -> Account {
        self.account(address)
            .unwrap_or_else(|| panic!("no account at {address}"))
    }
}

fn assert_unique_accounts(accounts: &[Account]) {
    for (index, account) in accounts.iter().enumerate() {
        assert!(
            accounts[..index]
                .iter()
                .all(|earlier| earlier.address != account.address),
            "transaction input contains account {} more than once",
            account.address
        );
    }
}

/// Decode `data` (from `offset`) as `T` via wincode's [`DefaultConfig`]. Shared
/// by [`Test::read`], [`Test::read_at`], and [`crate::Outcome::has_state`] so
/// every typed read takes the same decode path. `context` names the calling
/// operation so panics stay actionable.
pub(crate) fn decode<T>(context: &str, address: Pubkey, data: &[u8], offset: usize) -> T
where
    T: for<'de> SchemaRead<'de, DefaultConfig, Dst = T>,
{
    let name = core::any::type_name::<T>();
    let bytes = data.get(offset..).unwrap_or_else(|| {
        panic!(
            "{context} {name}: account {address} holds {} bytes, need at least {offset}",
            data.len()
        )
    });
    wincode::deserialize::<T>(bytes).unwrap_or_else(|error| {
        panic!("{context} {name}: account {address} did not decode as {name}: {error:?}")
    })
}

/// Typed account state captured at one point in a test.
///
/// Derefs to the decoded value, and also reports the address and lamport
/// balance it was read with.
pub struct Snapshot<T> {
    address: Pubkey,
    lamports: u64,
    state: T,
}

impl<T> Snapshot<T> {
    /// Address from which the state was read.
    pub fn address(&self) -> Pubkey {
        self.address
    }

    /// Lamport balance captured with the state.
    pub fn lamports(&self) -> u64 {
        self.lamports
    }
}

impl<T> Deref for Snapshot<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
