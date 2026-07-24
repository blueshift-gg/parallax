//! The C-ABI surface: opaque-handle lifecycle, fixture installs, account reads,
//! clock control, and transaction execution.
//!
//! Every function clears the thread-local last error on entry, validates its
//! pointer arguments, and runs the core work inside `catch_unwind` so a panic in
//! the harness becomes a status code and an error string instead of unwinding
//! across the boundary. Result blobs allocated here are freed with
//! [`parallax_free_bytes`]; handles are freed with [`parallax_free`].

use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

use crate::error::*;
use crate::wire;
use parallax_svm::{
    fixture::{AssociatedTokenAccount, Mint, TokenAccount, Wallet},
    Account, Pubkey, Test,
};

// ---------------------------------------------------------------------------
// Error query
// ---------------------------------------------------------------------------

/// Borrowed pointer to the calling thread's last-error C string, or null when
/// the last call succeeded. Valid until the next FFI call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_last_error() -> *const c_char {
    last_error_ptr()
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Build a world for `program_id` from in-memory ELF `elf`, optionally pinning a
/// transaction compute-unit limit. Pass `elf_len == 0` (with a null or empty
/// `elf`) to build a world with no primary program — only the runtime's
/// built-ins (e.g. the SPL programs). Returns an opaque handle, or null on
/// failure (`parallax_last_error` describes it). Free with [`parallax_free`].
#[unsafe(no_mangle)]
pub extern "C" fn parallax_new(
    program_id: *const [u8; 32],
    elf: *const u8,
    elf_len: u64,
    has_compute_unit_limit: bool,
    compute_unit_limit: u64,
) -> *mut Test {
    clear_last_error();
    // A null `elf` is only valid for the no-program case (`elf_len == 0`).
    if program_id.is_null() || (elf.is_null() && elf_len != 0) {
        set_last_error("null pointer argument");
        return ptr::null_mut();
    }
    let built = catch_unwind(AssertUnwindSafe(|| {
        let id = Pubkey::new_from_array(unsafe { *program_id });
        let mut builder = if elf_len == 0 {
            Test::builder(id).no_program()
        } else {
            let elf = unsafe { slice::from_raw_parts(elf, elf_len as usize) };
            Test::builder(id).program_bytes(elf.to_vec())
        };
        if has_compute_unit_limit {
            builder = builder.compute_unit_limit(compute_unit_limit);
        }
        builder.build()
    }));
    match built {
        Ok(Ok(test)) => Box::into_raw(Box::new(test)),
        Ok(Err(error)) => {
            set_last_error(format!("could not build world: {error}"));
            ptr::null_mut()
        }
        Err(_) => {
            set_last_error("panic while building world (is the program a valid SBF ELF?)");
            ptr::null_mut()
        }
    }
}

/// Free a handle returned by [`parallax_new`]. Safe on null.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_free(handle: *mut Test) {
    if !handle.is_null() {
        // Dropping the world runs the backend's destructor; guard the unwind so a
        // panic there can never cross the C boundary (which would be undefined
        // behavior). Every other entry point wraps its body the same way.
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
            drop(Box::from_raw(handle));
        }));
    }
}

/// Free a result buffer previously returned through a `result_out`/`result_len`
/// pair. Pass the exact pointer and length the call produced. Safe on null.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_free_bytes(ptr: *mut u8, len: u64) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(core::ptr::slice_from_raw_parts_mut(
                ptr,
                len as usize,
            )));
        }
    }
}

/// Reconfigure the transaction compute-unit limit on an existing world. Backs
/// the TypeScript runtime's `setComputeBudget`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_set_compute_unit_limit(handle: *mut Test, limit: u64) -> i32 {
    with_test(handle, |test| {
        test.set_compute_unit_limit(limit);
        PARALLAX_OK
    })
}

/// Preload a program (by id and ELF) for cross-program invocations. Also serves
/// as the program fixture install: the resulting address is the caller's `id`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_load_program(
    handle: *mut Test,
    program_id: *const [u8; 32],
    elf: *const u8,
    elf_len: u64,
) -> i32 {
    clear_last_error();
    if handle.is_null() || program_id.is_null() || elf.is_null() {
        set_last_error("null pointer argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    // The core loader panics on bytes that are not a valid SBF program. The only
    // operation inside this guard that can unwind is that load, so a caught panic
    // is a program-load failure — reported as such — rather than an internal
    // fault.
    match catch_unwind(AssertUnwindSafe(|| {
        let test = unsafe { &mut *handle };
        let id = Pubkey::new_from_array(unsafe { *program_id });
        let elf = unsafe { slice::from_raw_parts(elf, elf_len as usize) };
        test.load_program(id, elf);
    })) {
        Ok(()) => PARALLAX_OK,
        Err(_) => {
            set_last_error("could not load program (is the ELF a valid SBF program?)");
            PARALLAX_ERR_PROGRAM_LOAD
        }
    }
}

/// Set the runtime clock's Unix timestamp.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_warp_to_timestamp(handle: *mut Test, timestamp: i64) -> i32 {
    with_test(handle, |test| {
        test.warp_to_timestamp(timestamp);
        PARALLAX_OK
    })
}

// ---------------------------------------------------------------------------
// Fixture installs
// ---------------------------------------------------------------------------

/// Install a funded system wallet from a `WalletInput` option-bag. Writes the
/// resulting 32-byte address into `address_out`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_install_wallet(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    address_out: *mut u8,
) -> i32 {
    with_install(handle, input, input_len, address_out, |test, bytes| {
        let input = wire::deserialize_wallet_input(bytes)
            .map_err(|e| format!("invalid wallet input: {e}"))?;
        let mut wallet = Wallet::new();
        if let Some(address) = input.address {
            wallet = wallet.at(address);
        }
        if let Some(fund) = input.fund {
            wallet = wallet.fund(fund);
        }
        Ok(test.add(wallet))
    })
}

/// Install a token account from a `TokenAccountInput` option-bag. Writes the
/// resulting 32-byte address into `address_out`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_install_token_account(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    address_out: *mut u8,
) -> i32 {
    with_install(handle, input, input_len, address_out, |test, bytes| {
        let input = wire::deserialize_token_account_input(bytes)
            .map_err(|e| format!("invalid token account input: {e}"))?;
        let mut account = TokenAccount::new(input.mint, input.owner)
            .amount(input.amount)
            .token_program(input.token_program);
        if let Some(address) = input.address {
            account = account.at(address);
        }
        Ok(test.add(account))
    })
}

/// Install an associated-token account from an `AtaInput` option-bag. Writes the
/// derived 32-byte address into `address_out`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_install_ata(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    address_out: *mut u8,
) -> i32 {
    with_install(handle, input, input_len, address_out, |test, bytes| {
        let input =
            wire::deserialize_ata_input(bytes).map_err(|e| format!("invalid ata input: {e}"))?;
        let account = AssociatedTokenAccount::new(input.mint, input.owner)
            .amount(input.amount)
            .token_program(input.token_program);
        Ok(test.add(account))
    })
}

/// Install a raw account from a `RawAccountInput` option-bag. Writes the
/// resulting 32-byte address into `address_out`.
#[unsafe(no_mangle)]
pub extern "C" fn parallax_install_raw_account(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    address_out: *mut u8,
) -> i32 {
    with_install(handle, input, input_len, address_out, |test, bytes| {
        let input = wire::deserialize_raw_account_input(bytes)
            .map_err(|e| format!("invalid raw account input: {e}"))?;
        Ok(test.add(Account::new(
            input.address,
            input.owner,
            input.lamports,
            input.data,
        )))
    })
}

/// Install a mint (and one associated-token account per holder) from a
/// `MintInput` option-bag. Returns a count-prefixed pubkey list through
/// `result_out`/`result_len_out`: index 0 is the mint, indices 1.. are holder
/// ATAs in holder order. Free the buffer with [`parallax_free_bytes`].
#[unsafe(no_mangle)]
pub extern "C" fn parallax_install_mint(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
) -> i32 {
    if result_out.is_null() || result_len_out.is_null() {
        clear_last_error();
        set_last_error("null result pointer argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    with_result(
        handle,
        input,
        input_len,
        result_out,
        result_len_out,
        |test, bytes| {
            let input = wire::deserialize_mint_input(bytes)
                .map_err(|e| format!("invalid mint input: {e}"))?;
            let token_program = input.token_program;
            let mut mint = Mint::new()
                .supply(input.supply)
                .decimals(input.decimals)
                .token_program(token_program)
                .with_holder(input.holders.iter().copied());
            if let Some(authority) = input.authority {
                mint = mint.with_authority(authority);
            }
            if let Some(freeze_authority) = input.freeze_authority {
                mint = mint.with_freeze_authority(freeze_authority);
            }
            let mint_address = test.add(mint);
            let mut addresses = Vec::with_capacity(1 + input.holders.len());
            addresses.push(mint_address);
            for (owner, _amount) in &input.holders {
                addresses.push(test.derive_ata(*owner, mint_address, token_program));
            }
            Ok(wire::serialize_pubkeys(&addresses))
        },
    )
}

// ---------------------------------------------------------------------------
// Account reads
// ---------------------------------------------------------------------------

/// Read a raw account. Returns an `option<account>` blob through
/// `result_out`/`result_len_out` (a leading `0` byte means no account). Free the
/// buffer with [`parallax_free_bytes`].
#[unsafe(no_mangle)]
pub extern "C" fn parallax_get_account(
    handle: *mut Test,
    address: *const [u8; 32],
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
) -> i32 {
    clear_last_error();
    if handle.is_null() || address.is_null() || result_out.is_null() || result_len_out.is_null() {
        set_last_error("null pointer argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let test = unsafe { &*handle };
        let address = Pubkey::new_from_array(unsafe { *address });
        let account = test.account(address);
        wire::serialize_optional_account(account.as_ref())
    })) {
        Ok(bytes) => {
            write_result(result_out, result_len_out, bytes);
            PARALLAX_OK
        }
        Err(_) => {
            set_last_error("panic while reading account");
            PARALLAX_ERR_INTERNAL
        }
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute and commit an instruction chain, returning an outcome bundle through
/// `result_out`/`result_len_out`. `accounts` is a count-prefixed list of
/// explicit input accounts (pass a null pointer with length 0 for none). Free
/// the bundle with [`parallax_free_bytes`].
#[unsafe(no_mangle)]
pub extern "C" fn parallax_send(
    handle: *mut Test,
    instructions: *const u8,
    instructions_len: u64,
    accounts: *const u8,
    accounts_len: u64,
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
) -> i32 {
    execute(
        handle,
        instructions,
        instructions_len,
        accounts,
        accounts_len,
        result_out,
        result_len_out,
        true,
    )
}

/// Execute an instruction chain without committing, returning an outcome bundle.
/// Arguments match [`parallax_send`].
#[unsafe(no_mangle)]
pub extern "C" fn parallax_simulate(
    handle: *mut Test,
    instructions: *const u8,
    instructions_len: u64,
    accounts: *const u8,
    accounts_len: u64,
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
) -> i32 {
    execute(
        handle,
        instructions,
        instructions_len,
        accounts,
        accounts_len,
        result_out,
        result_len_out,
        false,
    )
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Null-check the handle and run `body` with a mutable core reference inside a
/// panic guard.
fn with_test<F>(handle: *mut Test, body: F) -> i32
where
    F: FnOnce(&mut Test) -> i32,
{
    clear_last_error();
    if handle.is_null() {
        set_last_error("null handle argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    match catch_unwind(AssertUnwindSafe(|| {
        let test = unsafe { &mut *handle };
        body(test)
    })) {
        Ok(code) => code,
        Err(_) => {
            set_last_error("panic at FFI boundary");
            PARALLAX_ERR_INTERNAL
        }
    }
}

/// Shared body for single-address installs: decode the input, install, and write
/// the resulting address into `address_out`.
fn with_install<F>(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    address_out: *mut u8,
    install: F,
) -> i32
where
    F: FnOnce(&mut Test, &[u8]) -> Result<Pubkey, String>,
{
    clear_last_error();
    if handle.is_null() || address_out.is_null() {
        set_last_error("null pointer argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    let bytes = match input_bytes(input, input_len) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    match catch_unwind(AssertUnwindSafe(|| {
        let test = unsafe { &mut *handle };
        install(test, bytes)
    })) {
        Ok(Ok(address)) => {
            unsafe {
                ptr::copy_nonoverlapping(address.to_bytes().as_ptr(), address_out, 32);
            }
            PARALLAX_OK
        }
        Ok(Err(message)) => {
            set_last_error(message);
            PARALLAX_ERR_INVALID_WIRE
        }
        Err(_) => {
            set_last_error("panic during install");
            PARALLAX_ERR_INTERNAL
        }
    }
}

/// Shared body for installs that return a variable-length blob.
fn with_result<F>(
    handle: *mut Test,
    input: *const u8,
    input_len: u64,
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
    install: F,
) -> i32
where
    F: FnOnce(&mut Test, &[u8]) -> Result<Box<[u8]>, String>,
{
    clear_last_error();
    if handle.is_null() {
        set_last_error("null handle argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    let bytes = match input_bytes(input, input_len) {
        Ok(bytes) => bytes,
        Err(code) => return code,
    };
    match catch_unwind(AssertUnwindSafe(|| {
        let test = unsafe { &mut *handle };
        install(test, bytes)
    })) {
        Ok(Ok(blob)) => {
            write_result(result_out, result_len_out, blob);
            PARALLAX_OK
        }
        Ok(Err(message)) => {
            set_last_error(message);
            PARALLAX_ERR_INVALID_WIRE
        }
        Err(_) => {
            set_last_error("panic during install");
            PARALLAX_ERR_INTERNAL
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute(
    handle: *mut Test,
    instructions: *const u8,
    instructions_len: u64,
    accounts: *const u8,
    accounts_len: u64,
    result_out: *mut *mut u8,
    result_len_out: *mut u64,
    commit: bool,
) -> i32 {
    clear_last_error();
    if handle.is_null()
        || instructions.is_null()
        || result_out.is_null()
        || result_len_out.is_null()
    {
        set_last_error("null pointer argument");
        return PARALLAX_ERR_NULL_POINTER;
    }
    let ix_bytes = unsafe { slice::from_raw_parts(instructions, instructions_len as usize) };
    let acct_bytes = match (accounts.is_null(), accounts_len) {
        (true, 0) => &[][..],
        (true, _) => {
            set_last_error("null accounts pointer with non-zero length");
            return PARALLAX_ERR_NULL_POINTER;
        }
        (false, len) => unsafe { slice::from_raw_parts(accounts, len as usize) },
    };
    match catch_unwind(AssertUnwindSafe(|| {
        let ixs = match wire::deserialize_instructions(ix_bytes) {
            Ok(ixs) => ixs,
            Err(error) => {
                set_last_error(format!("invalid instructions: {error}"));
                return Err(PARALLAX_ERR_INVALID_WIRE);
            }
        };
        if ixs.is_empty() {
            set_last_error("a transaction needs at least one instruction");
            return Err(PARALLAX_ERR_EXECUTION);
        }
        // An empty buffer is the "no explicit inputs" shortcut (null pointer,
        // zero length); a present buffer must carry the u32 count prefix.
        let inputs = if acct_bytes.is_empty() {
            Vec::new()
        } else {
            match wire::deserialize_accounts(acct_bytes) {
                Ok(inputs) => inputs,
                Err(error) => {
                    set_last_error(format!("invalid input accounts: {error}"));
                    return Err(PARALLAX_ERR_INVALID_WIRE);
                }
            }
        };
        let test = unsafe { &mut *handle };
        let outcome = if commit {
            test.send_all_with(ixs, inputs)
        } else {
            test.simulate_all_with(ixs, inputs)
        };
        Ok(wire::serialize_outcome(&outcome))
    })) {
        Ok(Ok(bytes)) => {
            write_result(result_out, result_len_out, bytes);
            PARALLAX_OK
        }
        Ok(Err(code)) => code,
        Err(_) => {
            set_last_error("panic during transaction execution");
            PARALLAX_ERR_INTERNAL
        }
    }
}

/// Validate an input pointer/length pair, returning a borrowed slice (empty when
/// the length is zero) or an error status.
fn input_bytes<'a>(input: *const u8, input_len: u64) -> Result<&'a [u8], i32> {
    match (input.is_null(), input_len) {
        (true, 0) => Ok(&[][..]),
        (true, _) => {
            set_last_error("null input pointer with non-zero length");
            Err(PARALLAX_ERR_NULL_POINTER)
        }
        (false, len) => Ok(unsafe { slice::from_raw_parts(input, len as usize) }),
    }
}

/// Hand a boxed buffer across the boundary through an out pointer/length pair.
fn write_result(result_out: *mut *mut u8, result_len_out: *mut u64, bytes: Box<[u8]>) {
    let len = bytes.len();
    let ptr = Box::into_raw(bytes) as *mut u8;
    unsafe {
        *result_out = ptr;
        *result_len_out = len as u64;
    }
}
