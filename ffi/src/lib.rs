//! C-ABI transport for the `parallax-svm` test harness.
//!
//! This crate is a thin, panic-safe boundary over [`parallax_svm::Ctx`]. The
//! core owns every harness semantic; this crate only marshals option-bags in
//! and result bundles out. The binary wire format — the contract consumers
//! (the `parallax-svm` npm package via koffi, or any C caller) implement
//! against — is documented in [`wire`]. The extern entry points live in
//! [`ffi`]; status codes and the last-error channel in [`error`].
//!
//! # Panic strategy
//!
//! Every `catch_unwind` in [`ffi`] rests on one assumption: a Rust panic
//! *unwinds*. Under `panic = "abort"` a panic terminates the process before
//! `catch_unwind` can turn it into a status code, so a panic in the harness
//! would crash the C host rather than being reported across the boundary. The
//! workspace pins `panic = "unwind"` in `[profile.release]` and `[profile.dev]`
//! (see the root `Cargo.toml`) to keep that assumption true wherever this
//! cdylib is built; the boundary's soundness proofs are only valid under it.
//!
//! Every extern entry point dereferences the raw pointers a C caller hands it —
//! that unsafety is the boundary's contract, not an oversight — so the crate
//! opts out of `clippy::not_unsafe_ptr_arg_deref` rather than marking every
//! `#[no_mangle]` function `unsafe` (which C callers cannot observe anyway).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod error;
pub mod ffi;
pub mod wire;

#[cfg(test)]
mod tests {
    use crate::error::*;
    use crate::ffi::*;
    use parallax_svm::{system_program, Pubkey};

    /// Locate a program artifact to load. Loading a primary program through
    /// [`parallax_new`] requires a valid SBF ELF (program-less worlds pass
    /// `elf_len == 0` instead); the transfer itself only touches the built-in
    /// system program, so any valid program serves. Gated on
    /// `PARALLAX_PROGRAM_PATH` so the suite stays green without an artifact.
    fn program_elf() -> Option<Vec<u8>> {
        let path = std::env::var_os("PARALLAX_PROGRAM_PATH")?;
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                eprintln!("PARALLAX_PROGRAM_PATH set but unreadable: {error}");
                None
            }
        }
    }

    // ---- tiny wire cursor for decoding the outcome bundle in-test ----
    struct Cursor<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self { data, pos: 0 }
        }
        fn take(&mut self, n: usize) -> &'a [u8] {
            let slice = &self.data[self.pos..self.pos + n];
            self.pos += n;
            slice
        }
        fn u8(&mut self) -> u8 {
            self.take(1)[0]
        }
        fn bool(&mut self) -> bool {
            self.u8() != 0
        }
        fn u32(&mut self) -> usize {
            u32::from_le_bytes(self.take(4).try_into().unwrap()) as usize
        }
        fn u64(&mut self) -> u64 {
            u64::from_le_bytes(self.take(8).try_into().unwrap())
        }
        fn pubkey(&mut self) -> Pubkey {
            Pubkey::new_from_array(self.take(32).try_into().unwrap())
        }
        fn bytes(&mut self) -> Vec<u8> {
            let len = self.u32();
            self.take(len).to_vec()
        }
    }

    #[derive(Debug, Clone)]
    struct WireAccount {
        address: Pubkey,
        owner: Pubkey,
        lamports: u64,
        data: Vec<u8>,
        executable: bool,
    }

    fn read_account(c: &mut Cursor) -> WireAccount {
        let address = c.pubkey();
        let owner = c.pubkey();
        let lamports = c.u64();
        let data = c.bytes();
        let executable = c.bool();
        WireAccount {
            address,
            owner,
            lamports,
            data,
            executable,
        }
    }

    fn read_option_account(c: &mut Cursor) -> Option<WireAccount> {
        c.bool().then(|| read_account(c))
    }

    struct Bundle {
        is_err: bool,
        compute_units: u64,
        changes: Vec<(Pubkey, Option<WireAccount>, Option<WireAccount>)>,
        accounts: Vec<WireAccount>,
    }

    fn decode_bundle(bytes: &[u8]) -> Bundle {
        let mut c = Cursor::new(bytes);
        let is_err = c.bool();
        if is_err {
            let _tag = c.u8();
            let _custom = c.u32(); // custom_code (u32) read as usize is fine here
            let _msg = c.bytes();
        }
        let compute_units = c.u64();
        let _return_data = c.bytes();
        let num_logs = c.u32();
        for _ in 0..num_logs {
            let _ = c.bytes();
        }
        // Post-state accounts precede changes; a change's `after` is a presence
        // bit resolved back to the account listed here at the same address.
        let num_accounts = c.u32();
        let mut accounts = Vec::with_capacity(num_accounts);
        for _ in 0..num_accounts {
            accounts.push(read_account(&mut c));
        }
        let num_changes = c.u32();
        let mut changes = Vec::with_capacity(num_changes);
        for _ in 0..num_changes {
            let address = c.pubkey();
            let before = read_option_account(&mut c);
            let after = c.bool().then(|| {
                accounts
                    .iter()
                    .find(|account| account.address == address)
                    .expect("after_present resolves to a post-state account")
                    .clone()
            });
            changes.push((address, before, after));
        }
        let _hint = c.bytes(); // trailing guided-error hint (empty when none)
        assert_eq!(c.pos, bytes.len(), "outcome bundle had trailing bytes");
        Bundle {
            is_err,
            compute_units,
            changes,
            accounts,
        }
    }

    /// Encode a System `Transfer` instruction chain in the wire format.
    fn system_transfer_wire(from: Pubkey, to: Pubkey, lamports: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // num_instructions
        buf.extend_from_slice(system_program::ID.as_ref()); // program_id
        let mut data = vec![2u8, 0, 0, 0];
        data.extend_from_slice(&lamports.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&data);
        buf.extend_from_slice(&2u32.to_le_bytes()); // num_metas
        buf.extend_from_slice(from.as_ref());
        buf.extend_from_slice(&[1, 1]); // signer, writable
        buf.extend_from_slice(to.as_ref());
        buf.extend_from_slice(&[0, 1]); // not signer, writable
        buf
    }

    fn wallet_input(address: Pubkey, fund: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(1);
        buf.extend_from_slice(address.as_ref());
        buf.push(1);
        buf.extend_from_slice(&fund.to_le_bytes());
        buf
    }

    #[test]
    fn end_to_end_system_transfer_through_ffi() {
        let Some(elf) = program_elf() else {
            eprintln!("skipping FFI e2e: set PARALLAX_PROGRAM_PATH to a program artifact");
            return;
        };

        let program_id = [7u8; 32];
        let handle = parallax_new(
            &program_id as *const [u8; 32],
            elf.as_ptr(),
            elf.len() as u64,
            false,
            0,
        );
        assert!(!handle.is_null(), "parallax_new returned null");

        // Install a funded payer at a pinned address.
        let payer = Pubkey::new_from_array([11u8; 32]);
        let recipient = Pubkey::new_from_array([22u8; 32]);
        let start = 10_000_000_000u64;
        let amount = 1_000_000_000u64;

        let input = wallet_input(payer, start);
        let mut payer_out = [0u8; 32];
        let code = parallax_install_wallet(
            handle,
            input.as_ptr(),
            input.len() as u64,
            payer_out.as_mut_ptr(),
        );
        assert_eq!(code, PARALLAX_OK);
        assert_eq!(Pubkey::new_from_array(payer_out), payer);

        // get_account round-trips the installed wallet.
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_get_account(
            handle,
            payer.to_bytes().as_ptr() as *const [u8; 32],
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_OK);
        let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
        let mut c = Cursor::new(blob);
        let present = c.bool();
        assert!(present, "installed wallet should be readable");
        let account = read_account(&mut c);
        assert_eq!(account.lamports, start);
        assert_eq!(account.owner, system_program::ID);
        assert!(!account.executable);
        parallax_free_bytes(result, result_len);

        // Send the transfer and decode the outcome bundle.
        let ix = system_transfer_wire(payer, recipient, amount);
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_send(
            handle,
            ix.as_ptr(),
            ix.len() as u64,
            std::ptr::null(),
            0,
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_OK, "parallax_send failed");
        let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
        let bundle = decode_bundle(blob);
        parallax_free_bytes(result, result_len);

        assert!(!bundle.is_err, "transfer should succeed");
        assert!(bundle.compute_units > 0, "transfer should consume compute");

        // First-appearance order: payer, then recipient.
        assert_eq!(bundle.changes.len(), 2);
        let (addr0, before0, after0) = &bundle.changes[0];
        assert_eq!(*addr0, payer);
        assert_eq!(before0.as_ref().unwrap().lamports, start);
        assert_eq!(after0.as_ref().unwrap().lamports, start - amount);

        let (addr1, before1, after1) = &bundle.changes[1];
        assert_eq!(*addr1, recipient);
        assert!(before1.is_none(), "recipient is created, no before");
        assert_eq!(after1.as_ref().unwrap().lamports, amount);

        // Post-state carries both writable accounts.
        let recipient_post = bundle
            .accounts
            .iter()
            .find(|a| a.address == recipient)
            .expect("recipient in post-state");
        assert_eq!(recipient_post.lamports, amount);
        let payer_post = bundle
            .accounts
            .iter()
            .find(|a| a.address == payer)
            .expect("payer in post-state");
        assert_eq!(payer_post.lamports, start - amount);

        // A committed transfer persists: reading the payer now shows the debit.
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        parallax_get_account(
            handle,
            payer.to_bytes().as_ptr() as *const [u8; 32],
            &mut result,
            &mut result_len,
        );
        let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
        let mut c = Cursor::new(blob);
        assert!(c.bool());
        assert_eq!(read_account(&mut c).lamports, start - amount);
        parallax_free_bytes(result, result_len);

        parallax_free(handle);
    }

    /// A world with no primary program (`elf_len == 0`) still executes
    /// built-in programs: the fixture-only test worlds the TypeScript shell
    /// constructs with `new Ctx()` depend on this path.
    #[test]
    fn program_less_world_executes_builtin_programs() {
        let program_id = [0u8; 32];
        let handle = parallax_new(
            &program_id as *const [u8; 32],
            std::ptr::null(),
            0,
            false,
            0,
        );
        assert!(!handle.is_null(), "program-less parallax_new returned null");

        let payer = Pubkey::new_from_array([31u8; 32]);
        let recipient = Pubkey::new_from_array([32u8; 32]);
        let start = 5_000_000_000u64;
        let amount = 2_000_000_000u64;

        let input = wallet_input(payer, start);
        let mut out = [0u8; 32];
        let code =
            parallax_install_wallet(handle, input.as_ptr(), input.len() as u64, out.as_mut_ptr());
        assert_eq!(code, PARALLAX_OK);

        let ix = system_transfer_wire(payer, recipient, amount);
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_send(
            handle,
            ix.as_ptr(),
            ix.len() as u64,
            std::ptr::null(),
            0,
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_OK, "send in a program-less world failed");
        let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
        let bundle = decode_bundle(blob);
        parallax_free_bytes(result, result_len);

        assert!(!bundle.is_err, "builtin transfer should succeed");
        assert_eq!(bundle.changes.len(), 2);
        assert_eq!(bundle.changes[1].2.as_ref().unwrap().lamports, amount);

        parallax_free(handle);
    }

    #[test]
    fn install_mint_returns_mint_and_holder_atas() {
        let Some(elf) = program_elf() else {
            eprintln!("skipping FFI mint test: set PARALLAX_PROGRAM_PATH");
            return;
        };
        let program_id = [7u8; 32];
        let handle = parallax_new(
            &program_id as *const [u8; 32],
            elf.as_ptr(),
            elf.len() as u64,
            false,
            0,
        );
        assert!(!handle.is_null());

        let alice = Pubkey::new_from_array([31u8; 32]);
        let bob = Pubkey::new_from_array([32u8; 32]);

        // MintInput: no authority, no freeze, supply 1000, decimals 6, legacy,
        // two holders.
        let mut input = Vec::new();
        input.push(0); // authority absent
        input.push(0); // freeze absent
        input.extend_from_slice(&1_000u64.to_le_bytes());
        input.push(6); // decimals
        input.push(0); // legacy
        input.extend_from_slice(&2u32.to_le_bytes());
        input.extend_from_slice(alice.as_ref());
        input.extend_from_slice(&400u64.to_le_bytes());
        input.extend_from_slice(bob.as_ref());
        input.extend_from_slice(&600u64.to_le_bytes());

        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_install_mint(
            handle,
            input.as_ptr(),
            input.len() as u64,
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_OK);
        let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
        let mut c = Cursor::new(blob);
        let count = c.u32();
        assert_eq!(count, 3, "mint + two holder ATAs");
        let mint = c.pubkey();
        let alice_ata = c.pubkey();
        let bob_ata = c.pubkey();
        parallax_free_bytes(result, result_len);

        assert_ne!(mint, alice_ata);
        assert_ne!(alice_ata, bob_ata);

        // The holder ATAs hold the funded balances.
        for (ata, expected) in [(alice_ata, 400u64), (bob_ata, 600u64)] {
            let mut result: *mut u8 = std::ptr::null_mut();
            let mut result_len: u64 = 0;
            parallax_get_account(
                handle,
                ata.to_bytes().as_ptr() as *const [u8; 32],
                &mut result,
                &mut result_len,
            );
            let blob = unsafe { std::slice::from_raw_parts(result, result_len as usize) };
            let mut c = Cursor::new(blob);
            assert!(c.bool(), "holder ATA {ata} should exist");
            let account = read_account(&mut c);
            // SPL token account layout: amount is a u64 LE at offset 64.
            let amount = u64::from_le_bytes(account.data[64..72].try_into().unwrap());
            assert_eq!(amount, expected);
            parallax_free_bytes(result, result_len);
        }

        parallax_free(handle);
    }

    #[test]
    fn null_handle_is_a_status_code_not_a_crash() {
        let mut out = [0u8; 32];
        let input = wallet_input(Pubkey::new_from_array([1u8; 32]), 1);
        let code = parallax_install_wallet(
            std::ptr::null_mut(),
            input.as_ptr(),
            input.len() as u64,
            out.as_mut_ptr(),
        );
        assert_eq!(code, PARALLAX_ERR_NULL_POINTER);
        assert!(!parallax_last_error().is_null());
    }

    // A world is pinned to its creating thread: a call from another thread is a
    // PARALLAX_ERR_WRONG_THREAD status code, not a data race into the world. The
    // same call on the creating thread still succeeds, and the rejection's
    // message lands on the last-error channel of the thread that made the call.
    #[test]
    fn cross_thread_use_is_a_status_code_not_a_data_race() {
        let handle = program_less_handle();

        // Raw pointers are !Send; carry the address across the thread boundary
        // exactly as a C caller sharing the handle would, provenance preserved.
        struct SendHandle(*mut parallax_svm::Ctx);
        unsafe impl Send for SendHandle {}
        let moved = SendHandle(handle);

        let code = std::thread::spawn(move || {
            // Capture the whole wrapper, not its !Send field (2021 disjoint capture).
            let moved = moved;
            let code = parallax_warp_to_timestamp(moved.0, 1);
            assert!(
                !parallax_last_error().is_null(),
                "the wrong-thread error is set on the calling thread"
            );
            code
        })
        .join()
        .unwrap();
        assert_eq!(code, PARALLAX_ERR_WRONG_THREAD);

        // The creating thread is unaffected by the rejected cross-thread call.
        assert_eq!(parallax_warp_to_timestamp(handle, 1), PARALLAX_OK);
        parallax_free(handle);
    }

    /// Build a program-less world for the boundary-robustness tests below.
    fn program_less_handle() -> *mut parallax_svm::Ctx {
        let program_id = [0u8; 32];
        let handle = parallax_new(
            &program_id as *const [u8; 32],
            std::ptr::null(),
            0,
            false,
            0,
        );
        assert!(!handle.is_null());
        handle
    }

    // Loading bytes that are not a valid SBF ELF makes the core loader panic;
    // the boundary must catch that unwind and report it as a program-load
    // failure — a status code, never a crash across the C ABI.
    #[test]
    fn bad_elf_load_is_a_status_code_not_a_crash() {
        let handle = program_less_handle();
        let other_id = [5u8; 32];
        let bad_elf = [0u8; 16];
        let code = parallax_load_program(
            handle,
            &other_id as *const [u8; 32],
            bad_elf.as_ptr(),
            bad_elf.len() as u64,
        );
        assert_eq!(code, PARALLAX_ERR_PROGRAM_LOAD);
        assert!(!parallax_last_error().is_null());
        parallax_free(handle);
    }

    // A truncated instruction chain (claims one instruction, supplies no bytes)
    // is a decode error, reported as an invalid-wire status.
    #[test]
    fn malformed_wire_send_is_a_status_code_not_a_crash() {
        let handle = program_less_handle();
        let truncated = 1u32.to_le_bytes();
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_send(
            handle,
            truncated.as_ptr(),
            truncated.len() as u64,
            std::ptr::null(),
            0,
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_ERR_INVALID_WIRE);
        assert!(!parallax_last_error().is_null());
        parallax_free(handle);
    }

    // An absurd instruction count must be rejected as invalid wire, never turned
    // into a huge allocation that aborts the process.
    #[test]
    fn absurd_instruction_count_through_ffi_is_rejected() {
        let handle = program_less_handle();
        let huge = u32::MAX.to_le_bytes();
        let mut result: *mut u8 = std::ptr::null_mut();
        let mut result_len: u64 = 0;
        let code = parallax_send(
            handle,
            huge.as_ptr(),
            huge.len() as u64,
            std::ptr::null(),
            0,
            &mut result,
            &mut result_len,
        );
        assert_eq!(code, PARALLAX_ERR_INVALID_WIRE);
        parallax_free(handle);
    }
}

// Micro-benchmarks for the wire codec — the new hot path between the core send
// path and the TypeScript shell. `#[ignore]` so they never run in the ordinary
// suite; measure with `cargo test --release -- --ignored --nocapture bench_`.
#[cfg(test)]
mod bench {
    use {
        crate::wire,
        parallax_svm::{
            fixture::Wallet, system_program, Account, AccountMeta, Ctx, Instruction, Outcome,
            Pubkey,
        },
        std::time::Instant,
    };

    /// Time `iters` iterations of `body` and print nanoseconds per iteration.
    fn report(name: &str, iters: u32, mut body: impl FnMut()) {
        for _ in 0..(iters / 8).max(1) {
            body();
        }
        let start = Instant::now();
        for _ in 0..iters {
            body();
        }
        let per = start.elapsed().as_nanos() as f64 / f64::from(iters);
        println!("{name}: {per:.0} ns/iter ({iters} iters)");
    }

    /// A System `Transfer` instruction chain encoded as the shell would send it.
    fn transfer_chain_wire(from: Pubkey, to: Pubkey, lamports: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // num_instructions
        buf.extend_from_slice(system_program::ID.as_ref());
        let mut data = vec![2u8, 0, 0, 0];
        data.extend_from_slice(&lamports.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&data);
        buf.extend_from_slice(&2u32.to_le_bytes()); // num_metas
        buf.extend_from_slice(from.as_ref());
        buf.extend_from_slice(&[1, 1]);
        buf.extend_from_slice(to.as_ref());
        buf.extend_from_slice(&[0, 1]);
        buf
    }

    /// Build a real committed `Outcome` whose recipient carries `data_len` bytes,
    /// so the bundle exercises a large writable account crossing the wire.
    fn transfer_outcome(data_len: usize) -> Outcome {
        let mut test = Ctx::builder(Pubkey::new_from_array([0; 32]))
            .no_program()
            .build()
            .expect("program-less world builds");
        let payer = test.add(Wallet::account().fund(u64::MAX));
        let recipient = Pubkey::new_from_array([9; 32]);
        if data_len > 0 {
            // A system-owned account the transfer credits; its data rides the
            // bundle in both the post-state accounts and the account change.
            test.set_account(Account::new(
                recipient,
                system_program::ID,
                1_000_000,
                vec![7; data_len],
            ));
        }
        let mut ix_data = vec![2u8, 0, 0, 0];
        ix_data.extend_from_slice(&1u64.to_le_bytes());
        let ix = Instruction {
            program_id: system_program::ID,
            accounts: vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(recipient, false),
            ],
            data: ix_data,
        };
        test.execute(ix)
    }

    #[test]
    #[ignore = "benchmark"]
    fn bench_wire_decode_instructions() {
        let chain = transfer_chain_wire(
            Pubkey::new_from_array([1; 32]),
            Pubkey::new_from_array([2; 32]),
            1,
        );
        report("wire_decode_instructions_transfer", 200_000, || {
            let ixs = wire::deserialize_instructions(&chain).unwrap();
            std::hint::black_box(ixs.len());
        });
    }

    #[test]
    #[ignore = "benchmark"]
    fn bench_wire_encode_outcome() {
        // Small (empty recipient), medium (token-account-sized), and large (10 KiB)
        // writable accounts, so the encode cost's scaling with account data — the
        // bundle's dominant term — is visible.
        for (label, data_len) in [("small_0b", 0), ("medium_165b", 165), ("large_10k", 10_240)] {
            let outcome = transfer_outcome(data_len);
            report(&format!("wire_encode_outcome_{label}"), 100_000, || {
                std::hint::black_box(wire::serialize_outcome(&outcome).len());
            });
        }
    }
}
