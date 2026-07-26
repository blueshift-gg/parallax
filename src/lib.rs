//! Parallax: a fixture-based testing harness for Solana programs on LiteSVM.
//!
//! [`parallax_test`] turns an ordinary Rust test into an isolated [`Test`]
//! world loaded with the current program. [`fixture`] provides composable
//! account setup, while [`Outcome`] keeps execution assertions structured and
//! independent of the SVM that ran the transaction. Typed account state is
//! read and written with [wincode](https://docs.rs/wincode) — a serialization
//! standard, not a framework — so the harness stays program-agnostic.
//!
//! ```rust,ignore
//! use parallax_svm::prelude::*;
//!
//! #[parallax_test]
//! fn initializes(test: &mut Test) {
//!     let authority = test.add(Wallet::account());
//!     test.send(InitializeInstruction { authority }).succeeds();
//! }
//! ```
//!
//! [`fixture::Wallet::account`] funds an actor with the default balance;
//! [`fixture::Wallet::fund`] sets an exact one. Any signer a transaction names
//! but never installs is auto-funded on send, so co-signers cost nothing extra.
//!
//! The name is the pitch: the same program observed from multiple vantage
//! points — this Rust crate and the `parallax-svm` TypeScript package (Kit and
//! Web3.js) — must agree. Fixture addresses are deterministic and identical
//! across all three.
//!
//! # Determinism
//!
//! Execution is deterministic and is treated as a guarantee, not an accident:
//! two fresh [`Test`] worlds running the identical scenario produce
//! byte-identical results — the same [`Outcome`] (error, compute units, logs,
//! return data, account changes) and the same post-state account bytes. The
//! backend seeds a fixed genesis blockhash and a zero-timestamp clock (no
//! wall-clock reads, no RNG), fixture placement follows a per-world
//! deterministic address sequence, and every observable ordering — the tracked
//! account set, the outcome's accounts and changes — is first-appearance, never
//! hash-map iteration. A run therefore reproduces exactly, in this crate and
//! across the TypeScript harnesses. (Both harness test suites assert this
//! directly against two worlds.)

#![warn(missing_docs)]

mod accounts;
mod backend;
mod check;
mod dump;
pub mod fixture;
mod outcome;
mod setup;
mod types;
mod world;

/// FFI support: the two-phase dump plan result. Not part of the stable surface.
#[doc(hidden)]
pub use dump::DumpPlan;

pub use {
    check::{bundle, CheckFn, Cu, DataExpected, Expected, ExpectedBytes, Raw, ReturnData, Typed},
    outcome::{FailedTransaction, Outcome, SucceededTransaction},
    parallax_svm_derive::parallax_test,
    setup::{SetupError, TestBuilder, PROGRAM_PATH_ENV},
    solana_instruction::{AccountMeta, Instruction},
    solana_pubkey::Pubkey,
    solana_sdk_ids::system_program,
    types::{Account, AccountChange, ProgramError},
    world::{Snapshot, Test, DEFAULT_WALLET_LAMPORTS},
};

/// The SPL Token program.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// The Token-2022 program.
pub const SPL_TOKEN_2022_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

/// The SPL Associated Token Account program.
pub const SPL_ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// Build read-only signer metas for co-signers, such as multisig members.
///
/// Each address becomes an [`AccountMeta`] that is read-only and a signer, the
/// shape a program expects for an authority it only needs to have signed.
/// [`Test::send`] auto-registers any co-signer the world has not installed as a
/// funded system account — as it does for every signer it backfills — so tests
/// pass the addresses alone without hand-rolling metas or wallets.
pub fn co_signers(addresses: &[Pubkey]) -> Vec<AccountMeta> {
    addresses
        .iter()
        .map(|&address| AccountMeta::new_readonly(address, true))
        .collect()
}

/// Imports used by most program tests.
pub mod prelude {
    pub use crate::{
        bundle, co_signers,
        fixture::{
            AssociatedTokenAccount, Dump, Fixture, Load, Mint, Program, TokenAccount, TokenProgram,
            Wallet,
        },
        parallax_test, system_program, Account, AccountChange, AccountMeta, CheckFn, Cu,
        FailedTransaction, Instruction, Outcome, ProgramError, Pubkey, ReturnData, Snapshot,
        SucceededTransaction, Test, DEFAULT_WALLET_LAMPORTS, SPL_ASSOCIATED_TOKEN_PROGRAM_ID,
        SPL_TOKEN_2022_PROGRAM_ID, SPL_TOKEN_PROGRAM_ID,
    };
}

#[cfg(test)]
mod tests {
    use {
        super::{
            backend::Backend,
            co_signers,
            fixture::{AssociatedTokenAccount, Fixture, Mint, TokenAccount, TokenProgram, Wallet},
            setup::resolve_program_path_from_named,
            system_program, Account, AccountMeta, CheckFn, Instruction, ProgramError, Pubkey,
            SetupError, Test, DEFAULT_WALLET_LAMPORTS, SPL_TOKEN_2022_PROGRAM_ID,
        },
        spl_token::solana_program::program_option::COption,
        std::{fs, path::PathBuf},
    };

    /// Encode a System program `Transfer` (`SystemInstruction` variant 2).
    fn system_transfer(from: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
        let mut data = vec![2, 0, 0, 0];
        data.extend_from_slice(&lamports.to_le_bytes());
        Instruction {
            program_id: system_program::ID,
            accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
            data,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "parallax-svm-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    fn empty_test() -> Test {
        Test::from_parts(
            Backend::new(),
            Pubkey::new_from_array([42; 32]),
            PathBuf::new(),
            crate::dump::DEFAULT_RPC_URL.to_string(),
            None,
            crate::dump::default_transport(),
        )
    }

    #[test]
    fn resolves_the_only_deployed_program() {
        let root = temp_dir("resolve");
        let nested = root.join("program/tests");
        let deploy = root.join("target/deploy");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&deploy).unwrap();
        fs::write(deploy.join("example.so"), b"elf").unwrap();

        assert_eq!(
            resolve_program_path_from_named(&nested, None).unwrap(),
            deploy.join("example.so")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_the_named_crate_among_many_programs() {
        let root = temp_dir("named");
        let deploy = root.join("target/deploy");
        fs::create_dir_all(&deploy).unwrap();
        fs::write(deploy.join("my_program.so"), b"elf").unwrap();
        fs::write(deploy.join("other_program.so"), b"elf").unwrap();

        assert_eq!(
            resolve_program_path_from_named(&root, Some("my-program")).unwrap(),
            deploy.join("my_program.so")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ambiguous_deploy_directories() {
        let root = temp_dir("ambiguous");
        let deploy = root.join("target/deploy");
        fs::create_dir_all(&deploy).unwrap();
        fs::write(deploy.join("one.so"), b"elf").unwrap();
        fs::write(deploy.join("two.so"), b"elf").unwrap();

        assert!(matches!(
            resolve_program_path_from_named(&root, None),
            Err(SetupError::AmbiguousPrograms { .. })
        ));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fixtures_are_deterministic_and_compose() {
        let mut test = empty_test();
        let wallet = test.add(Wallet::account().fund(42));
        let mint = test.add(
            Mint::account()
                .with_authority(wallet)
                .with_supply(1_000)
                .decimals(9)
                .token_program(TokenProgram::Token2022),
        );
        let tokens = test.add(
            TokenAccount::account(mint, wallet)
                .with_amount(600)
                .token_program(TokenProgram::Token2022),
        );
        let associated = test.add(
            AssociatedTokenAccount::account(mint, wallet)
                .with_amount(400)
                .token_program(TokenProgram::Token2022),
        );

        assert_eq!(test.lamports(wallet), 42);
        assert_eq!(test.supply(mint), 1_000);
        assert_eq!(test.tokens(tokens), 600);
        assert_eq!(test.tokens(associated), 400);
        assert_eq!(test.account(mint).unwrap().owner, SPL_TOKEN_2022_PROGRAM_ID);
        assert_eq!(
            associated,
            test.derive_ata(wallet, mint, TokenProgram::Token2022)
        );
    }

    #[test]
    fn arrays_install_repeated_fixtures_without_helper_methods() {
        let mut test = empty_test();
        let [alice, bob, carol] = test.add([Wallet::account().fund(7); 3]);

        assert_eq!(test.lamports(alice), 7);
        assert_eq!(test.lamports(bob), 7);
        assert_eq!(test.lamports(carol), 7);
        assert_ne!(alice, bob);
        assert_ne!(bob, carol);
    }

    // The generated-address plural (`Mint::accounts()`) installs N distinct
    // mints with one shared config — the plural for a non-`Copy` fixture, which
    // the `[expr; N]` array-repeat syntax cannot express. N is inferred from
    // the destructuring pattern; no count is written anywhere.
    #[test]
    fn accounts_const_generic_installs_n_distinct_fresh_fixtures() {
        let mut test = empty_test();

        let [first, second] = test.add(Mint::accounts().with_supply(1_000));
        assert_ne!(first, second);
        assert_eq!(test.supply(first), 1_000);
        assert_eq!(test.supply(second), 1_000);
    }

    // The pinned-address plural (`Wallet::accounts([..])` / `TokenAccount::accounts`)
    // applies one config at each named address, mirroring `Dump::accounts`.
    #[test]
    fn accounts_array_pins_a_fixture_at_every_address() {
        let mut test = empty_test();
        let a = Pubkey::new_from_array([10; 32]);
        let b = Pubkey::new_from_array([11; 32]);

        let [alice, bob] = test.add(Wallet::accounts([a, b]).fund(5_000));
        assert_eq!(alice, a);
        assert_eq!(bob, b);
        assert_eq!(test.lamports(a), 5_000);
        assert_eq!(test.lamports(b), 5_000);

        // TokenAccount::accounts carries mint/owner/amount to each pinned address.
        let mint = test.add(Mint::account().with_supply(1_000));
        let owner = test.add(Wallet::account());
        let ta_a = Pubkey::new_from_array([20; 32]);
        let ta_b = Pubkey::new_from_array([21; 32]);
        let [first, second] =
            test.add(TokenAccount::accounts([ta_a, ta_b], mint, owner).with_amount(42));
        assert_eq!([first, second], [ta_a, ta_b]);
        assert_eq!(test.tokens(ta_a), 42);
        assert_eq!(test.tokens(ta_b), 42);
    }

    #[test]
    fn applications_can_define_protocol_fixtures() {
        struct ProtocolFixture;

        impl Fixture for ProtocolFixture {
            type Output = (Pubkey, Pubkey);

            fn install(self, test: &mut Test) -> Self::Output {
                let authority = test.add(Wallet::account());
                let state = Pubkey::new_from_array([200; 32]);
                test.add(Account::new(state, test.program_id(), 1, vec![1, 2, 3]));
                (authority, state)
            }
        }

        let mut test = empty_test();
        let (authority, state) = test.add(ProtocolFixture);
        assert_ne!(authority, state);
        assert_eq!(test.account(state).unwrap().data, [1, 2, 3]);
    }

    #[test]
    fn stable_program_errors_do_not_expose_the_backend_type() {
        let error =
            ProgramError::from(solana_instruction_error::InstructionError::InvalidInstructionData);
        assert_eq!(error, ProgramError::InvalidInstructionData);
    }

    #[test]
    fn with_holder_installs_one_associated_account_per_holder() {
        let mut test = empty_test();
        let authority = test.add(Wallet::account());
        let alice = test.add(Wallet::account());
        let bob = test.add(Wallet::account());

        let mint = test.add(
            Mint::account()
                .with_authority(authority)
                .with_supply(1_000)
                .token_program(TokenProgram::Token2022)
                .with_holder([(alice, 400), (bob, 600)]),
        );

        let alice_ata = test.derive_ata(alice, mint, TokenProgram::Token2022);
        let bob_ata = test.derive_ata(bob, mint, TokenProgram::Token2022);
        assert_eq!(test.supply(mint), 1_000);
        assert_eq!(test.tokens(alice_ata), 400);
        assert_eq!(test.tokens(bob_ata), 600);
        assert_eq!(
            test.account(alice_ata).unwrap().owner,
            SPL_TOKEN_2022_PROGRAM_ID
        );
    }

    #[test]
    fn co_signers_are_read_only_signer_metas() {
        let first = Pubkey::new_from_array([1; 32]);
        let second = Pubkey::new_from_array([2; 32]);

        let metas = co_signers(&[first, second]);

        assert_eq!(metas.len(), 2);
        for (meta, expected) in metas.iter().zip([first, second]) {
            assert_eq!(meta.pubkey, expected);
            assert!(meta.is_signer);
            assert!(!meta.is_writable);
        }
        assert!(co_signers(&[]).is_empty());
    }

    #[test]
    fn mint_new_is_fixed_supply_without_authorities() {
        use spl_token::{solana_program::program_pack::Pack, state::Mint as SplMint};

        let mut test = empty_test();
        let mint = test.add(Mint::account().with_supply(1_000));
        let decoded = SplMint::unpack(&test.account(mint).unwrap().data).unwrap();

        assert_eq!(decoded.mint_authority, COption::None);
        assert_eq!(decoded.freeze_authority, COption::None);
        assert_eq!(decoded.supply, 1_000);
    }

    #[test]
    fn mint_authorities_come_from_the_builders() {
        use spl_token::{solana_program::program_pack::Pack, state::Mint as SplMint};

        let mut test = empty_test();
        let authority = Pubkey::new_from_array([1; 32]);
        let freeze = Pubkey::new_from_array([2; 32]);
        let mint = test.add(
            Mint::account()
                .with_authority(authority)
                .with_freeze_authority(freeze),
        );
        let decoded = SplMint::unpack(&test.account(mint).unwrap().data).unwrap();

        assert_eq!(decoded.mint_authority, COption::Some(authority));
        assert_eq!(decoded.freeze_authority, COption::Some(freeze));
    }

    // The typed-state API is wincode-native: `write` serializes a value and
    // installs it as a rent-exempt account owned by an explicit program, and
    // `read` decodes the account's data back into the value through the same
    // schema. A discriminator-free struct's schema covers the whole account.
    #[derive(wincode::SchemaRead, wincode::SchemaWrite, PartialEq, Debug)]
    struct Vault {
        owner: [u8; 32],
        amount: u64,
    }

    #[test]
    fn writes_and_reads_a_wincode_typed_account() {
        let mut test = empty_test();
        let program = test.program_id();
        let address = Pubkey::new_from_array([3; 32]);

        let returned = test.write(
            address,
            program,
            Vault {
                owner: [7; 32],
                amount: 1_000,
            },
        );
        assert_eq!(returned, address);

        let snapshot = test.read::<Vault>(address);
        assert_eq!(snapshot.owner, [7; 32]);
        assert_eq!(snapshot.amount, 1_000);
        assert_eq!(snapshot.address(), address);
        // 40 bytes of data, installed rent-exempt.
        assert!(snapshot.lamports() > 0);
        assert_eq!(test.account(address).unwrap().owner, program);
    }

    #[test]
    fn read_at_decodes_a_suffix_after_a_prefix() {
        let mut test = empty_test();
        let program = test.program_id();
        let address = Pubkey::new_from_array([4; 32]);

        // A one-byte discriminator followed by a u64 the caller frames apart.
        let mut data = vec![0xAB];
        data.extend_from_slice(&99_u64.to_le_bytes());
        test.set_account(Account::new(address, program, 1, data));

        let snapshot = test.read_at::<u64>(address, 1);
        assert_eq!(*snapshot, 99);
    }

    // Trailing-bytes contract for `read`/`read_at`. wincode decodes exactly `T`'s
    // bytes and stops; the harness then requires the unconsumed tail to be all
    // zero. A stray non-zero byte past a well-formed value — the fingerprint of
    // the wrong (or a stale) type read against an account — is a precise panic,
    // not a silent short read.
    #[test]
    #[should_panic(expected = "trailing")]
    fn read_rejects_nonzero_trailing_bytes() {
        let mut test = empty_test();
        let program = test.program_id();
        let address = Pubkey::new_from_array([7; 32]);

        // A well-formed 40-byte Vault, then one stray non-zero byte.
        let mut data = wincode::serialize(&Vault {
            owner: [1; 32],
            amount: 5,
        })
        .unwrap();
        data.push(0xAB);
        test.set_account(Account::new(address, program, 1, data));

        let _ = test.read::<Vault>(address);
    }

    // Zeroed trailing bytes are reserved padding — a growable or migration-target
    // account carries space for future fields, zero-initialized on chain — so a
    // typed read of the current fields still succeeds.
    #[test]
    fn read_allows_zeroed_reserved_padding() {
        let mut test = empty_test();
        let program = test.program_id();
        let address = Pubkey::new_from_array([8; 32]);

        let mut data = wincode::serialize(&Vault {
            owner: [2; 32],
            amount: 9,
        })
        .unwrap();
        data.extend_from_slice(&[0; 16]); // reserved padding for future fields
        test.set_account(Account::new(address, program, 1, data));

        let snapshot = test.read::<Vault>(address);
        assert_eq!(snapshot.owner, [2; 32]);
        assert_eq!(snapshot.amount, 9);
    }

    // The tail check applies to `read_at`'s suffix too: after the framed offset,
    // a non-zero unconsumed byte is rejected.
    #[test]
    #[should_panic(expected = "trailing")]
    fn read_at_rejects_nonzero_trailing_bytes() {
        let mut test = empty_test();
        let program = test.program_id();
        let address = Pubkey::new_from_array([9; 32]);

        // A one-byte discriminator, a u64, then a stray non-zero byte.
        let mut data = vec![0xAB];
        data.extend_from_slice(&7_u64.to_le_bytes());
        data.push(0x01);
        test.set_account(Account::new(address, program, 1, data));

        let _ = test.read_at::<u64>(address, 1);
    }

    #[test]
    fn derive_pda_matches_find_program_address() {
        let test = empty_test();
        let program = test.program_id();
        let (expected, bump) = Pubkey::find_program_address(&[b"vault", &[7]], &program);

        assert_eq!(test.derive_pda(&[b"vault", &[7]]), expected);
        assert_eq!(
            test.derive_pda_with_bump(&[b"vault", &[7]]),
            (expected, bump)
        );
    }

    // A missing writable account is an init target and enters empty — even
    // when it signs, as a keypair account being created does. Payers are
    // world state: a payer the test never installs has nothing to move.
    #[test]
    fn a_missing_writable_signer_is_an_empty_init_target_not_a_funded_payer() {
        let mut test = empty_test();
        // Raw addresses the world never installs, so send must backfill them.
        let payer = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        let amount = 1_000_000_000;

        // The uninstalled payer enters empty, so the transfer must fail —
        // proof that writable-signer init targets are not silently funded.
        assert!(test
            .simulate(system_transfer(payer, recipient, amount))
            .is_err());

        // Installed as a wallet, the same transfer goes through.
        test.add(Wallet::account().at(payer));
        test.send(system_transfer(payer, recipient, amount))
            .succeeds()
            .checks([
                Account::lamports(recipient, amount),
                Account::lamports(payer, |lamports| lamports > 0),
                Account::created(recipient),
            ]);
    }

    // `simulate_with` seeds explicit transaction inputs like `send_with`, but
    // commits nothing: a payer supplied only as an input lets the simulated
    // transfer succeed, yet never enters the world afterwards.
    #[test]
    fn simulate_with_seeds_inputs_without_committing() {
        let mut test = empty_test();
        let payer = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        let amount = 1_000_000;

        // Uninstalled, the payer is an empty init target and the transfer fails.
        assert!(test
            .simulate(system_transfer(payer, recipient, amount))
            .is_err());

        // Seeded as an explicit input, the same transfer simulates successfully.
        let funded = Account::new(
            payer,
            system_program::ID,
            DEFAULT_WALLET_LAMPORTS,
            Vec::new(),
        );
        test.simulate_with(system_transfer(payer, recipient, amount), [funded])
            .succeeds();

        // Simulation commits nothing, so the seeded payer never enters the world.
        assert!(test.account(payer).is_none());
    }

    // A read-only signer (a co-signer, e.g. a multisig member) is an actor
    // and enters funded, even though the world never installed it.
    #[test]
    fn a_read_only_signer_is_backfilled_funded() {
        let mut test = empty_test();
        let payer = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        let cosigner = Pubkey::new_from_array([3; 32]);
        let amount = 1_000_000_000;
        test.add(Wallet::account().at(payer));

        let mut transfer = system_transfer(payer, recipient, amount);
        transfer
            .accounts
            .push(AccountMeta::new_readonly(cosigner, true));

        test.send(transfer)
            .succeeds()
            .check(Account::lamports(cosigner, DEFAULT_WALLET_LAMPORTS));
    }

    /// A named protocol invariant: `account` never holds more than `max`
    /// lamports. Guards on presence, the realistic shape — an invariant only
    /// judges accounts the checked transaction touched.
    fn capped(account: Pubkey, max: u64) -> CheckFn {
        CheckFn::new(move |tx| {
            if let Some(account_state) = tx.account(account) {
                assert!(
                    account_state.lamports <= max,
                    "cap exceeded for {account}: {} lamports",
                    account_state.lamports
                );
            }
        })
    }

    // A registered invariant is verified after every committed send — the
    // second transfer pushes the recipient past the cap and the *send itself*
    // fails, with no assertion written at the call site.
    #[test]
    #[should_panic(expected = "cap exceeded")]
    fn invariants_run_after_every_send() {
        let mut test = empty_test();
        let payer = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        test.add(Wallet::account().at(payer));
        test.invariant(capped(recipient, 1_500_000_000));

        test.send(system_transfer(payer, recipient, 1_000_000_000))
            .succeeds();
        // The invariant panics inside this send, before its outcome exists.
        let _ = test.send(system_transfer(payer, recipient, 1_000_000_000));
    }

    // Simulations commit nothing, so invariants do not judge them: the same
    // over-cap transfer that fails as a send passes as a simulation.
    #[test]
    fn invariants_ignore_simulations() {
        let mut test = empty_test();
        let payer = Pubkey::new_from_array([1; 32]);
        let recipient = Pubkey::new_from_array([2; 32]);
        test.add(Wallet::account().at(payer));
        test.invariant(capped(recipient, 1));

        test.simulate(system_transfer(payer, recipient, 1_000_000_000))
            .succeeds();
    }

    // Micro-benchmarks for the harness hot paths. These are `#[ignore]` so they
    // never run in the ordinary suite; measure with
    // `cargo test --release -- --ignored --nocapture bench_`.
    mod bench {
        use {super::*, std::time::Instant};

        /// Time `iters` iterations of `body` and print nanoseconds per iteration.
        fn report(name: &str, iters: u32, mut body: impl FnMut()) {
            // Warm up caches and one-time allocations before timing.
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

        #[test]
        #[ignore = "benchmark"]
        fn bench_world_setup_with_fixtures() {
            report("world_setup_16_fixtures", 2_000, || {
                let mut test = empty_test();
                let authority = test.add(Wallet::account());
                let mint = test.add(Mint::account().with_authority(authority).with_supply(1_000));
                for _ in 0..14 {
                    let holder = test.add(Wallet::account());
                    test.add(TokenAccount::account(mint, holder).with_amount(10));
                }
                std::hint::black_box(&test);
            });
        }

        #[test]
        #[ignore = "benchmark"]
        fn bench_send_round_trip() {
            let mut test = empty_test();
            let payer = test.add(Wallet::account().fund(u64::MAX));
            let recipient = Pubkey::new_from_array([9; 32]);
            report("send_round_trip", 5_000, || {
                std::hint::black_box(test.send(system_transfer(payer, recipient, 1)).is_ok());
            });
        }

        #[test]
        #[ignore = "benchmark"]
        fn bench_outcome_construction() {
            use crate::{backend::ExecutionResult, outcome::TrackedAccount};

            // A representative tracked set: two writable accounts that changed,
            // plus six read-only accounts (program, token program, sysvars,
            // mint, authority) that could not, each carrying token-account-sized
            // data.
            let tracked_account = |seed: u8, writable: bool, changed: bool| {
                let owner = Pubkey::new_from_array([1; 32]);
                let before = Account::new(
                    Pubkey::new_from_array([seed; 32]),
                    owner,
                    1_000,
                    vec![seed; 165],
                );
                let after = changed.then(|| {
                    let mut after = before.clone();
                    after.lamports += 1;
                    after
                });
                TrackedAccount {
                    address: before.address,
                    writable,
                    signer: false,
                    after: Some(after.unwrap_or_else(|| before.clone())),
                    before: Some(before),
                }
            };

            report("outcome_construction_8_accounts", 50_000, || {
                let tracked = vec![
                    tracked_account(1, true, true),
                    tracked_account(2, true, true),
                    tracked_account(3, false, false),
                    tracked_account(4, false, false),
                    tracked_account(5, false, false),
                    tracked_account(6, false, false),
                    tracked_account(7, false, false),
                    tracked_account(8, false, false),
                ];
                let outcome =
                    crate::Outcome::from_backend(ExecutionResult::for_test(1_400), tracked);
                std::hint::black_box(outcome.account_changes().len());
            });
        }

        #[test]
        #[ignore = "benchmark"]
        fn bench_typed_read() {
            let mut test = empty_test();
            let program = test.program_id();
            let address = Pubkey::new_from_array([3; 32]);
            test.write(
                address,
                program,
                Vault {
                    owner: [7; 32],
                    amount: 1_000,
                },
            );
            report("typed_read", 20_000, || {
                let snapshot = test.read::<Vault>(address);
                std::hint::black_box(snapshot.amount);
            });
        }
    }
}
