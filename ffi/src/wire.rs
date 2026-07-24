//! Binary wire format between the `parallax-svm` core and its FFI consumers.
//!
//! This module doc is the contract. The TypeScript shell (and any other
//! consumer) serializes inputs and deserializes outputs exactly as specified
//! here. The Rust core owns every harness semantic (fixture placement, fresh
//! addresses, backfill, fee model, account-change tracking, error mapping); the
//! wire only moves option-bags in and result bundles out.
//!
//! # Conventions
//!
//! * All integers are **little-endian**.
//! * A **count**/**length** prefix is `u32` unless noted; **lamports**,
//!   **amount**, and **supply** are `u64`; **timestamp** is `i64`.
//! * `pubkey` = 32 raw bytes.
//! * `bool` = 1 byte, `0` = false, non-zero = true.
//! * `option<T>` = `[1] present` (`0`/`1`) followed by `T` only when present.
//! * `bytes` (length-prefixed) = `[4] len` + `[len] raw`.
//! * `string` (length-prefixed) = `[4] len` + `[len]` UTF-8.
//!
//! # `account`
//!
//! The single account encoding, used everywhere an account crosses the wire:
//!
//! ```text
//! [32] address
//! [32] owner
//! [8]  lamports (u64)
//! [4]  data_len
//! [N]  data
//! [1]  executable (bool)
//! ```
//!
//! # `token_program` enum (1 byte)
//!
//! ```text
//! 0 = Legacy     (SPL Token,  Tokenkeg...)
//! 1 = Token2022  (Token-2022, TokenzQd...)
//! ```
//!
//! # Inputs
//!
//! ## Instruction chain — `parallax_send` / `parallax_simulate`
//!
//! ```text
//! [4] num_instructions
//! per instruction:
//!   [32] program_id
//!   [4]  data_len
//!   [N]  data
//!   [4]  num_metas
//!   per meta:
//!     [32] pubkey
//!     [1]  is_signer  (bool)
//!     [1]  is_writable (bool)
//! ```
//!
//! ## Explicit input accounts — `parallax_send` / `parallax_simulate`
//!
//! A count-prefixed list of `account` (see above). For the common case where
//! the world's installed fixtures supply every input, pass `[4] 0` — or, as a
//! shortcut, a null pointer with length 0 (an empty buffer is read as zero
//! accounts).
//!
//! ```text
//! [4] num_accounts
//! per account: account
//! ```
//!
//! ## `install_wallet` input
//!
//! ```text
//! option<pubkey> address   // absent => next deterministic fresh address
//! option<u64>    fund       // absent => DEFAULT_WALLET_LAMPORTS (10_000_000_000)
//! ```
//!
//! ## `install_mint` input
//!
//! The mint address is always a fresh address (never caller-pinned), matching
//! the core `Mint` fixture.
//!
//! ```text
//! option<pubkey> authority         // absent => fixed-supply (no mint authority)
//! option<pubkey> freeze_authority  // absent => not freezable
//! [8] supply (u64)
//! [1] decimals (u8)
//! [1] token_program (enum)
//! [4] num_holders
//! per holder:
//!   [32] owner
//!   [8]  amount (u64)
//! ```
//!
//! ## `install_token_account` input
//!
//! ```text
//! option<pubkey> address   // absent => next deterministic fresh address
//! [32] mint
//! [32] owner
//! [8]  amount (u64)
//! [1]  token_program (enum)
//! ```
//!
//! ## `install_ata` input
//!
//! The address is always the derived associated-token address.
//!
//! ```text
//! [32] mint
//! [32] owner
//! [8]  amount (u64)
//! [1]  token_program (enum)
//! ```
//!
//! ## `install_raw_account` input
//!
//! Installs a non-executable account. Executable accounts are programs; load
//! them with `parallax_load_program`.
//!
//! ```text
//! [32] address
//! [32] owner
//! [8]  lamports (u64)
//! [4]  data_len
//! [N]  data
//! ```
//!
//! # Outputs
//!
//! ## Install result (returned by `install_mint`)
//!
//! A count-prefixed list of pubkeys. Index `0` is the primary address (the
//! mint); indices `1..` are the holders' associated-token addresses in the
//! order the holders were supplied.
//!
//! ```text
//! [4] num_addresses
//! per address: [32] pubkey
//! ```
//!
//! Single-address installs (`install_wallet`, `install_token_account`,
//! `install_ata`, `install_raw_account`) instead write the one resulting
//! address into a caller-provided 32-byte buffer, so no allocation is freed.
//!
//! ## `parallax_get_account` result
//!
//! ```text
//! option<account>
//! ```
//!
//! ## Outcome bundle — `parallax_send` / `parallax_simulate`
//!
//! Everything the TypeScript `Outcome`/`AccountChange` surface exposes is
//! derivable from this bundle.
//!
//! ```text
//! --- status ---
//! [1] is_err (bool)            // 0 => success, 1 => failure
//! if is_err:
//!   [1] error_tag (u8)         // ProgramError tag, table below
//!   [4] custom_code (u32)      // Custom's code; 0 otherwise
//!   [4] runtime_msg_len
//!   [N] runtime_msg (UTF-8)    // Runtime's message; empty otherwise
//! --- metrics ---
//! [8] compute_units (u64)
//! --- return data ---
//! [4] return_data_len
//! [N] return_data
//! --- logs (execution order) ---
//! [4] num_logs
//! per log: string
//! --- account changes (first-appearance instruction order) ---
//! [4] num_changes
//! per change:
//!   [32] address
//!   option<account> before     // absent => created by this transaction
//!   option<account> after      // absent => removed by this transaction
//! --- post-state accounts (first-appearance order; existing accounts only) ---
//! [4] num_accounts
//! per account: account
//! ```
//!
//! ### `ProgramError` tag table (u8)
//!
//! Mirrors [`parallax_svm::ProgramError`] exactly. `custom_code` carries the
//! code for `Custom`; `runtime_msg` carries the message for `Runtime`.
//!
//! ```text
//!  0 = InvalidArgument            10 = InvalidSeeds
//!  1 = InvalidInstructionData     11 = ArithmeticOverflow
//!  2 = InvalidAccountData         12 = AccountNotRentExempt
//!  3 = AccountDataTooSmall        13 = InvalidAccountOwner
//!  4 = InsufficientFunds          14 = IncorrectAuthority
//!  5 = IncorrectProgramId         15 = Immutable
//!  6 = MissingRequiredSignature   16 = BorshIoError
//!  7 = AccountAlreadyInitialized  17 = ComputeBudgetExceeded
//!  8 = UninitializedAccount       18 = Custom       (custom_code)
//!  9 = MissingAccount             19 = Runtime      (runtime_msg)
//! ```

use parallax_svm::{
    fixture::TokenProgram, Account, AccountMeta, Instruction, Outcome, ProgramError, Pubkey,
};

// ---------------------------------------------------------------------------
// ProgramError tags
// ---------------------------------------------------------------------------

/// Wire tag for [`ProgramError::Custom`]; its code rides in `custom_code`.
pub const ERROR_TAG_CUSTOM: u8 = 18;
/// Wire tag for [`ProgramError::Runtime`]; its message rides in `runtime_msg`.
pub const ERROR_TAG_RUNTIME: u8 = 19;

/// The stable wire tag for a program error. `Custom`/`Runtime` payloads are
/// serialized separately (see the module doc).
fn error_tag(error: &ProgramError) -> u8 {
    match error {
        ProgramError::InvalidArgument => 0,
        ProgramError::InvalidInstructionData => 1,
        ProgramError::InvalidAccountData => 2,
        ProgramError::AccountDataTooSmall => 3,
        ProgramError::InsufficientFunds => 4,
        ProgramError::IncorrectProgramId => 5,
        ProgramError::MissingRequiredSignature => 6,
        ProgramError::AccountAlreadyInitialized => 7,
        ProgramError::UninitializedAccount => 8,
        ProgramError::MissingAccount => 9,
        ProgramError::InvalidSeeds => 10,
        ProgramError::ArithmeticOverflow => 11,
        ProgramError::AccountNotRentExempt => 12,
        ProgramError::InvalidAccountOwner => 13,
        ProgramError::IncorrectAuthority => 14,
        ProgramError::Immutable => 15,
        ProgramError::BorshIoError => 16,
        ProgramError::ComputeBudgetExceeded => 17,
        ProgramError::Custom(_) => ERROR_TAG_CUSTOM,
        ProgramError::Runtime(_) => ERROR_TAG_RUNTIME,
        // `ProgramError` is `#[non_exhaustive]`: a future stable variant reaches
        // the consumer as `Runtime` with its Display text until this table and
        // the tag doc are extended.
        _ => ERROR_TAG_RUNTIME,
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], &'static str> {
        let end = self.pos.checked_add(n).ok_or("length overflow")?;
        if end > self.data.len() {
            return Err("unexpected end of input");
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, &'static str> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_bool(&mut self) -> Result<bool, &'static str> {
        Ok(self.read_u8()? != 0)
    }

    fn read_u32(&mut self) -> Result<u32, &'static str> {
        let bytes: [u8; 4] = self.read_bytes(4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(bytes))
    }

    fn read_len(&mut self) -> Result<usize, &'static str> {
        Ok(self.read_u32()? as usize)
    }

    fn read_u64(&mut self) -> Result<u64, &'static str> {
        let bytes: [u8; 8] = self.read_bytes(8)?.try_into().unwrap();
        Ok(u64::from_le_bytes(bytes))
    }

    fn read_pubkey(&mut self) -> Result<Pubkey, &'static str> {
        let bytes: [u8; 32] = self.read_bytes(32)?.try_into().unwrap();
        Ok(Pubkey::new_from_array(bytes))
    }

    fn read_option_pubkey(&mut self) -> Result<Option<Pubkey>, &'static str> {
        if self.read_bool()? {
            Ok(Some(self.read_pubkey()?))
        } else {
            Ok(None)
        }
    }

    fn read_option_u64(&mut self) -> Result<Option<u64>, &'static str> {
        if self.read_bool()? {
            Ok(Some(self.read_u64()?))
        } else {
            Ok(None)
        }
    }

    fn read_length_prefixed(&mut self) -> Result<Vec<u8>, &'static str> {
        let len = self.read_len()?;
        Ok(self.read_bytes(len)?.to_vec())
    }

    fn read_token_program(&mut self) -> Result<TokenProgram, &'static str> {
        match self.read_u8()? {
            0 => Ok(TokenProgram::Legacy),
            1 => Ok(TokenProgram::Token2022),
            _ => Err("invalid token_program tag"),
        }
    }

    fn read_account(&mut self) -> Result<Account, &'static str> {
        let address = self.read_pubkey()?;
        let owner = self.read_pubkey()?;
        let lamports = self.read_u64()?;
        let data = self.read_length_prefixed()?;
        let executable = self.read_bool()?;
        Ok(Account {
            address,
            lamports,
            data,
            owner,
            executable,
        })
    }

    fn finish(&self, context: &'static str) -> Result<(), &'static str> {
        if self.remaining() > 0 {
            return Err(context);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(1024),
        }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_bool(&mut self, v: bool) {
        self.buf.push(u8::from(v));
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_len(&mut self, v: usize) {
        self.write_u32(v as u32);
    }

    fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    fn write_length_prefixed(&mut self, data: &[u8]) {
        self.write_len(data.len());
        self.write_bytes(data);
    }

    fn write_pubkey(&mut self, pubkey: &Pubkey) {
        self.write_bytes(pubkey.as_ref());
    }

    fn write_account(&mut self, account: &Account) {
        self.write_pubkey(&account.address);
        self.write_pubkey(&account.owner);
        self.write_u64(account.lamports);
        self.write_length_prefixed(&account.data);
        self.write_bool(account.executable);
    }

    fn write_option_account(&mut self, account: Option<&Account>) {
        match account {
            Some(account) => {
                self.write_bool(true);
                self.write_account(account);
            }
            None => self.write_bool(false),
        }
    }

    fn into_boxed_slice(self) -> Box<[u8]> {
        self.buf.into_boxed_slice()
    }
}

// ---------------------------------------------------------------------------
// Input decoding
// ---------------------------------------------------------------------------

fn read_one_instruction(r: &mut Reader) -> Result<Instruction, &'static str> {
    let program_id = r.read_pubkey()?;
    let data = r.read_length_prefixed()?;
    let num_metas = r.read_len()?;
    let mut accounts = Vec::with_capacity(num_metas);
    for _ in 0..num_metas {
        let pubkey = r.read_pubkey()?;
        let is_signer = r.read_bool()?;
        let is_writable = r.read_bool()?;
        accounts.push(AccountMeta {
            pubkey,
            is_signer,
            is_writable,
        });
    }
    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

/// Decode a count-prefixed instruction chain.
pub fn deserialize_instructions(data: &[u8]) -> Result<Vec<Instruction>, &'static str> {
    let mut r = Reader::new(data);
    let count = r.read_len()?;
    let mut instructions = Vec::with_capacity(count);
    for _ in 0..count {
        instructions.push(read_one_instruction(&mut r)?);
    }
    r.finish("trailing data after instructions")?;
    Ok(instructions)
}

/// Decode a count-prefixed list of explicit input accounts.
pub fn deserialize_accounts(data: &[u8]) -> Result<Vec<Account>, &'static str> {
    let mut r = Reader::new(data);
    let count = r.read_len()?;
    let mut accounts = Vec::with_capacity(count);
    for _ in 0..count {
        accounts.push(r.read_account()?);
    }
    r.finish("trailing data after accounts")?;
    Ok(accounts)
}

/// Decoded `install_wallet` option-bag.
pub struct WalletInput {
    /// Explicit address, or `None` for the next fresh address.
    pub address: Option<Pubkey>,
    /// Explicit funding, or `None` for `DEFAULT_WALLET_LAMPORTS`.
    pub fund: Option<u64>,
}

/// Decode an `install_wallet` option-bag.
pub fn deserialize_wallet_input(data: &[u8]) -> Result<WalletInput, &'static str> {
    let mut r = Reader::new(data);
    let address = r.read_option_pubkey()?;
    let fund = r.read_option_u64()?;
    r.finish("trailing data after wallet input")?;
    Ok(WalletInput { address, fund })
}

/// Decoded `install_mint` option-bag.
pub struct MintInput {
    /// Mint authority, or `None` for a fixed-supply mint.
    pub authority: Option<Pubkey>,
    /// Freeze authority, or `None` when the mint cannot freeze.
    pub freeze_authority: Option<Pubkey>,
    /// Initial supply.
    pub supply: u64,
    /// Mint precision.
    pub decimals: u8,
    /// Owning token program.
    pub token_program: TokenProgram,
    /// `(owner, amount)` holders, each funded through its associated account.
    pub holders: Vec<(Pubkey, u64)>,
}

/// Decode an `install_mint` option-bag.
pub fn deserialize_mint_input(data: &[u8]) -> Result<MintInput, &'static str> {
    let mut r = Reader::new(data);
    let authority = r.read_option_pubkey()?;
    let freeze_authority = r.read_option_pubkey()?;
    let supply = r.read_u64()?;
    let decimals = r.read_u8()?;
    let token_program = r.read_token_program()?;
    let num_holders = r.read_len()?;
    let mut holders = Vec::with_capacity(num_holders);
    for _ in 0..num_holders {
        let owner = r.read_pubkey()?;
        let amount = r.read_u64()?;
        holders.push((owner, amount));
    }
    r.finish("trailing data after mint input")?;
    Ok(MintInput {
        authority,
        freeze_authority,
        supply,
        decimals,
        token_program,
        holders,
    })
}

/// Decoded `install_token_account` option-bag.
pub struct TokenAccountInput {
    /// Explicit address, or `None` for the next fresh address.
    pub address: Option<Pubkey>,
    /// Mint the account holds.
    pub mint: Pubkey,
    /// Account owner (authority).
    pub owner: Pubkey,
    /// Initial token balance.
    pub amount: u64,
    /// Owning token program.
    pub token_program: TokenProgram,
}

/// Decode an `install_token_account` option-bag.
pub fn deserialize_token_account_input(data: &[u8]) -> Result<TokenAccountInput, &'static str> {
    let mut r = Reader::new(data);
    let address = r.read_option_pubkey()?;
    let mint = r.read_pubkey()?;
    let owner = r.read_pubkey()?;
    let amount = r.read_u64()?;
    let token_program = r.read_token_program()?;
    r.finish("trailing data after token account input")?;
    Ok(TokenAccountInput {
        address,
        mint,
        owner,
        amount,
        token_program,
    })
}

/// Decoded `install_ata` option-bag.
pub struct AtaInput {
    /// Mint the associated account holds.
    pub mint: Pubkey,
    /// Wallet the associated account belongs to.
    pub owner: Pubkey,
    /// Initial token balance.
    pub amount: u64,
    /// Owning token program (also seeds the address derivation).
    pub token_program: TokenProgram,
}

/// Decode an `install_ata` option-bag.
pub fn deserialize_ata_input(data: &[u8]) -> Result<AtaInput, &'static str> {
    let mut r = Reader::new(data);
    let mint = r.read_pubkey()?;
    let owner = r.read_pubkey()?;
    let amount = r.read_u64()?;
    let token_program = r.read_token_program()?;
    r.finish("trailing data after ata input")?;
    Ok(AtaInput {
        mint,
        owner,
        amount,
        token_program,
    })
}

/// Decoded `install_raw_account` option-bag.
pub struct RawAccountInput {
    /// Address to install at.
    pub address: Pubkey,
    /// Program that owns the account.
    pub owner: Pubkey,
    /// Lamport balance.
    pub lamports: u64,
    /// Raw account data.
    pub data: Vec<u8>,
}

/// Decode an `install_raw_account` option-bag.
pub fn deserialize_raw_account_input(data: &[u8]) -> Result<RawAccountInput, &'static str> {
    let mut r = Reader::new(data);
    let address = r.read_pubkey()?;
    let owner = r.read_pubkey()?;
    let lamports = r.read_u64()?;
    let bytes = r.read_length_prefixed()?;
    r.finish("trailing data after raw account input")?;
    Ok(RawAccountInput {
        address,
        owner,
        lamports,
        data: bytes,
    })
}

// ---------------------------------------------------------------------------
// Output encoding
// ---------------------------------------------------------------------------

/// Encode a count-prefixed pubkey list (the `install_mint` result).
pub fn serialize_pubkeys(pubkeys: &[Pubkey]) -> Box<[u8]> {
    let mut w = Writer::new();
    w.write_len(pubkeys.len());
    for pubkey in pubkeys {
        w.write_pubkey(pubkey);
    }
    w.into_boxed_slice()
}

/// Encode an `option<account>` (the `parallax_get_account` result).
pub fn serialize_optional_account(account: Option<&Account>) -> Box<[u8]> {
    let mut w = Writer::new();
    w.write_option_account(account);
    w.into_boxed_slice()
}

/// Encode the outcome bundle for a committed or simulated transaction.
pub fn serialize_outcome(outcome: &Outcome) -> Box<[u8]> {
    let mut w = Writer::new();

    // Status.
    match outcome.error() {
        None => w.write_bool(false),
        Some(error) => {
            w.write_bool(true);
            w.write_u8(error_tag(error));
            let custom_code = match error {
                ProgramError::Custom(code) => *code,
                _ => 0,
            };
            w.write_u32(custom_code);
            match error {
                ProgramError::Runtime(message) => w.write_length_prefixed(message.as_bytes()),
                _ => w.write_len(0),
            }
        }
    }

    // Metrics.
    w.write_u64(outcome.compute_units());

    // Return data.
    w.write_length_prefixed(outcome.return_data());

    // Logs.
    let logs = outcome.logs();
    w.write_len(logs.len());
    for log in logs {
        w.write_length_prefixed(log.as_bytes());
    }

    // Account changes (first-appearance order).
    let changes = outcome.account_changes();
    w.write_len(changes.len());
    for change in changes {
        w.write_pubkey(&change.address());
        w.write_option_account(change.before());
        w.write_option_account(change.after());
    }

    // Post-state accounts (first-appearance order; existing accounts only).
    let accounts = outcome.accounts();
    w.write_len(accounts.len());
    for account in accounts {
        w.write_account(account);
    }

    w.into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(seed: u8) -> Pubkey {
        Pubkey::new_from_array([seed; 32])
    }

    fn account(seed: u8) -> Account {
        Account {
            address: pk(seed),
            owner: pk(seed.wrapping_add(1)),
            lamports: u64::from(seed) * 1000,
            data: vec![seed; seed as usize],
            executable: seed.is_multiple_of(2),
        }
    }

    #[test]
    fn instruction_chain_round_trips() {
        let instructions = vec![
            Instruction {
                program_id: pk(1),
                accounts: vec![
                    AccountMeta {
                        pubkey: pk(2),
                        is_signer: true,
                        is_writable: true,
                    },
                    AccountMeta {
                        pubkey: pk(3),
                        is_signer: false,
                        is_writable: false,
                    },
                ],
                data: vec![9, 8, 7, 6],
            },
            Instruction {
                program_id: pk(4),
                accounts: vec![],
                data: vec![],
            },
        ];

        // Re-encode with the wire writer so the test exercises both directions.
        let mut w = Writer::new();
        w.write_len(instructions.len());
        for ix in &instructions {
            w.write_pubkey(&ix.program_id);
            w.write_length_prefixed(&ix.data);
            w.write_len(ix.accounts.len());
            for meta in &ix.accounts {
                w.write_pubkey(&meta.pubkey);
                w.write_bool(meta.is_signer);
                w.write_bool(meta.is_writable);
            }
        }
        let bytes = w.into_boxed_slice();

        let decoded = deserialize_instructions(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].program_id, pk(1));
        assert_eq!(decoded[0].data, vec![9, 8, 7, 6]);
        assert_eq!(decoded[0].accounts.len(), 2);
        assert!(decoded[0].accounts[0].is_signer && decoded[0].accounts[0].is_writable);
        assert!(!decoded[0].accounts[1].is_signer && !decoded[0].accounts[1].is_writable);
        assert_eq!(decoded[1].program_id, pk(4));
        assert!(decoded[1].accounts.is_empty());
    }

    #[test]
    fn accounts_round_trip() {
        let accounts = vec![account(2), account(5), account(200)];
        let mut w = Writer::new();
        w.write_len(accounts.len());
        for a in &accounts {
            w.write_account(a);
        }
        let bytes = w.into_boxed_slice();

        let decoded = deserialize_accounts(&bytes).unwrap();
        assert_eq!(decoded, accounts);
    }

    #[test]
    fn wallet_input_round_trips_both_option_states() {
        // Both present.
        let mut w = Writer::new();
        w.write_bool(true);
        w.write_pubkey(&pk(7));
        w.write_bool(true);
        w.write_u64(42);
        let decoded = deserialize_wallet_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(decoded.address, Some(pk(7)));
        assert_eq!(decoded.fund, Some(42));

        // Both absent.
        let mut w = Writer::new();
        w.write_bool(false);
        w.write_bool(false);
        let decoded = deserialize_wallet_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(decoded.address, None);
        assert_eq!(decoded.fund, None);
    }

    #[test]
    fn mint_input_round_trips() {
        let mut w = Writer::new();
        w.write_bool(true);
        w.write_pubkey(&pk(1));
        w.write_bool(false);
        w.write_u64(1_000);
        w.write_u8(9);
        w.write_u8(1); // Token2022
        w.write_len(2);
        w.write_pubkey(&pk(2));
        w.write_u64(400);
        w.write_pubkey(&pk(3));
        w.write_u64(600);

        let decoded = deserialize_mint_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(decoded.authority, Some(pk(1)));
        assert_eq!(decoded.freeze_authority, None);
        assert_eq!(decoded.supply, 1_000);
        assert_eq!(decoded.decimals, 9);
        assert_eq!(decoded.token_program, TokenProgram::Token2022);
        assert_eq!(decoded.holders, vec![(pk(2), 400), (pk(3), 600)]);
    }

    #[test]
    fn token_account_and_ata_and_raw_inputs_round_trip() {
        let mut w = Writer::new();
        w.write_bool(false);
        w.write_pubkey(&pk(10));
        w.write_pubkey(&pk(11));
        w.write_u64(123);
        w.write_u8(0);
        let ta = deserialize_token_account_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(ta.address, None);
        assert_eq!(ta.mint, pk(10));
        assert_eq!(ta.owner, pk(11));
        assert_eq!(ta.amount, 123);
        assert_eq!(ta.token_program, TokenProgram::Legacy);

        let mut w = Writer::new();
        w.write_pubkey(&pk(10));
        w.write_pubkey(&pk(11));
        w.write_u64(7);
        w.write_u8(1);
        let ata = deserialize_ata_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(ata.mint, pk(10));
        assert_eq!(ata.owner, pk(11));
        assert_eq!(ata.amount, 7);
        assert_eq!(ata.token_program, TokenProgram::Token2022);

        let mut w = Writer::new();
        w.write_pubkey(&pk(20));
        w.write_pubkey(&pk(21));
        w.write_u64(9_999);
        w.write_length_prefixed(&[1, 2, 3, 4, 5]);
        let raw = deserialize_raw_account_input(&w.into_boxed_slice()).unwrap();
        assert_eq!(raw.address, pk(20));
        assert_eq!(raw.owner, pk(21));
        assert_eq!(raw.lamports, 9_999);
        assert_eq!(raw.data, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pubkeys_and_optional_account_encode() {
        let bytes = serialize_pubkeys(&[pk(1), pk(2)]);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_len().unwrap(), 2);
        assert_eq!(r.read_pubkey().unwrap(), pk(1));
        assert_eq!(r.read_pubkey().unwrap(), pk(2));
        assert_eq!(r.remaining(), 0);

        let some = serialize_optional_account(Some(&account(3)));
        let mut r = Reader::new(&some);
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_account().unwrap(), account(3));
        assert_eq!(r.remaining(), 0);

        let none = serialize_optional_account(None);
        assert_eq!(&*none, &[0]);
    }

    #[test]
    fn truncated_input_is_rejected() {
        assert!(deserialize_instructions(&[1, 0, 0, 0]).is_err());
        assert!(deserialize_accounts(&[1, 0, 0, 0]).is_err());
        assert!(deserialize_wallet_input(&[1]).is_err());
    }

    #[test]
    fn trailing_data_is_rejected() {
        let mut w = Writer::new();
        w.write_bool(false);
        w.write_bool(false);
        w.write_u8(0xFF); // one byte too many
        assert!(deserialize_wallet_input(&w.into_boxed_slice()).is_err());
    }
}
