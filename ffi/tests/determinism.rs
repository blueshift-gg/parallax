//! Determinism proof: two fresh worlds running the identical scenario must
//! produce byte-identical results.
//!
//! This drives the core through the public wire API — exactly the bytes a
//! consumer observes — so any nondeterminism in the core, the LiteSVM backend,
//! or the serialization path would surface here as differing bytes: the raw
//! outcome bundles of a successful send and a failed send, and the post-state of
//! every account the scenario touched. Determinism is a product guarantee (see
//! the `parallax_svm` crate docs), not an accident of a single run.

use parallax_svm::{
    fixture::{Mint, TokenProgram, Wallet},
    system_program, AccountMeta, Instruction, Pubkey, Test,
};
use parallax_svm_ffi::wire;

fn system_transfer(from: Pubkey, to: Pubkey, lamports: u64) -> Instruction {
    let mut data = vec![2u8, 0, 0, 0];
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: system_program::ID,
        accounts: vec![AccountMeta::new(from, true), AccountMeta::new(to, false)],
        data,
    }
}

/// The raw bytes a consumer observes from one scenario run: the success-send
/// bundle, the failed-send bundle, and each touched account's post-state (an
/// `option<account>` blob).
type ScenarioBytes = (Box<[u8]>, Box<[u8]>, Vec<Box<[u8]>>);

/// Run one fixed, non-trivial scenario in a fresh world and return the raw wire
/// bytes a consumer would observe.
fn scenario() -> ScenarioBytes {
    let mut test = Test::builder(Pubkey::new_from_array([0; 32]))
        .no_program()
        .build()
        .expect("program-less world builds");

    // Fixtures: a funded payer, two holders, and a mint that funds each holder's
    // associated token account.
    let payer = test.add(Wallet::account());
    let alice = test.add(Wallet::account());
    let bob = test.add(Wallet::account());
    let mint = test.add(
        Mint::account()
            .with_authority(payer)
            .with_supply(1_000)
            .token_program(TokenProgram::Legacy)
            .with_holder([(alice, 400), (bob, 600)]),
    );

    // A successful send: move lamports from the funded payer to a new account.
    let recipient = Pubkey::new_from_array([9; 32]);
    let ok = wire::serialize_outcome(&test.send(system_transfer(payer, recipient, 1_000_000)));

    // A failed send: an uninstalled writable signer enters as an empty init
    // target with no lamports, so the transfer fails and commits nothing.
    let ghost = Pubkey::new_from_array([8; 32]);
    let err = wire::serialize_outcome(&test.send(system_transfer(ghost, recipient, 1_000_000)));

    // Post-state of every touched account, in a fixed address order.
    let alice_ata = test.derive_ata(alice, mint, TokenProgram::Legacy);
    let bob_ata = test.derive_ata(bob, mint, TokenProgram::Legacy);
    let accounts = [payer, alice, bob, mint, alice_ata, bob_ata, recipient]
        .into_iter()
        .map(|address| wire::serialize_optional_account(test.account(address).as_ref()))
        .collect();

    (ok, err, accounts)
}

#[test]
fn two_worlds_produce_byte_identical_results() {
    let (ok_a, err_a, accounts_a) = scenario();
    let (ok_b, err_b, accounts_b) = scenario();

    assert_eq!(ok_a, ok_b, "success bundle differs across identical worlds");
    assert_eq!(
        err_a, err_b,
        "failure bundle differs across identical worlds"
    );
    assert_eq!(
        accounts_a, accounts_b,
        "post-state accounts differ across identical worlds"
    );

    // The scenario is genuinely non-trivial — guard against a degenerate
    // all-empty "determinism" where both sends did nothing. The first bundle
    // byte is the `is_err` flag (see wire.rs).
    assert_eq!(ok_a[0], 0, "the success send should have succeeded");
    assert_eq!(err_a[0], 1, "the failed send should have failed");
}
