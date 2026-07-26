<h1 align="center">Parallax</h1>

<!-- Logo drops in here once one exists: <p align="center"><img width="380" alt="Parallax" src="..." /></p> -->

<p align="center">
  <b>A fixture-based testing harness for <a href="https://github.com/LiteSVM/litesvm">LiteSVM</a>.</b>
</p>

<p align="center">
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" /></a>
  <!-- Badges below activate at first publish — keep commented until the packages/CI exist:
  <a href="https://crates.io/crates/parallax-svm"><img src="https://img.shields.io/crates/v/parallax-svm?logo=rust" /></a>
  <a href="https://www.npmjs.com/package/parallax-svm"><img src="https://img.shields.io/npm/v/parallax-svm?logo=npm" /></a>
  <a href="https://github.com/blueshift-gg/parallax/actions"><img src="https://img.shields.io/github/actions/workflow/status/blueshift-gg/parallax/ci.yml?logo=github" /></a>
  -->
</p>

Parallax makes an on-chain program test read like an ordinary test: install
fixtures, send an instruction, assert on the outcome. The same test model is
available from Rust, [Kit](https://github.com/anza-xyz/kit), and Web3.js, backed
by a single Rust implementation, so a program tested in Rust behaves identically
when a TypeScript client drives it.

## Quickstart

```toml
# Cargo.toml
[dev-dependencies]
parallax-svm = "0.1"
```

```rust,ignore
use parallax_svm::prelude::*;

#[parallax_test]
fn deposits_into_the_vault(test: &mut Test) {
    let authority = test.add(Wallet::new());

    test.send(DepositInstruction { authority, amount: 1_000_000_000 })
        .succeeds()
        .cu_at_most(10_000);
}
```

`#[parallax_test]` expands to a plain `#[test]` that loads the current crate's
compiled program into an isolated `Test` world — the program id defaults to
`crate::ID`. That is the whole setup.

## Why Parallax

- **Real runtime.** LiteSVM runs the Agave runtime, so rent, ownership, and
  account rules behave like mainnet.
- **One implementation, three languages.** The Rust, Kit, and Web3.js harnesses
  share the same names, the same semantics, and deterministic worlds.
- **Fast.** ~5 µs per `send` in Rust, ~6.5 µs from TypeScript, ~180 ns for a
  typed read. ([numbers](#performance))
- **Spoofed signers, zero fees.** Any address can sign without a keypair, and
  balances only move when a program moves them.
- **Mainnet state in two lines.** Dump accounts or programs once; every later
  run is offline and deterministic.
- **No framework lock-in.** Typed state goes through
  [wincode](https://docs.rs/wincode) schemas and plain codecs, so any Solana
  program's accounts decode.

## The same test, both languages

One deposit test, in Rust and in TypeScript.

```rust,ignore
// Rust
#[parallax_test]
fn deposits(test: &mut Test) {
    let user = test.add(Wallet::new());

    test.send(DepositInstruction { user, amount: 1_000 })
        .succeeds()
        .has_tokens(vault_of(user), 1_000);
}
```

```ts
// TypeScript (Kit)
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/vault.so");
const user = await test.add(wallet());
const deposit = await new VaultClient().createDepositInstruction({ user, amount: 1_000n });

test.send(deposit).succeeds().hasTokens(vaultOf(user), 1_000n);
```

`parallax-svm/kit` and `parallax-svm/web3.js` are thin shells over the same Rust
core; the member names are identical, camel-cased. See [`typescript/`](typescript).

## Tour

### Fixtures are values

A fixture is any value implementing `Fixture`; `test.add` installs it and returns
the address(es) it placed — the only composition primitive.

```rust,ignore
let [alice, bob] = test.add([Wallet::new().fund(7); 2]);   // two fresh funded actors

let mint = test.add(
    Mint::new()
        .with_authority(alice)
        .with_holder([(alice, 400), (bob, 600)]),          // one funded ATA per holder
);
```

Built-ins are `Wallet`, `Mint`, `TokenAccount`, `AssociatedTokenAccount`,
`Account`, and `Program`; the full catalog — plurals, application fixtures — is in
the [reference](docs/rust_reference.md#fixtures-are-values).

### Dump real mainnet state

```rust,ignore
let [pool, oracle] = test.add(Dump::accounts([POOL, ORACLE]));
test.add(Dump::program(AMM_PROGRAM).sync_clock());   // adopt the dumped slot's clock
test.send(SwapInstruction { pool, oracle }).succeeds();
```

- **First run fetches once** — one batched `getMultipleAccounts` at one slot;
  every later run is fully offline and deterministic.
- **Bytes land in a committed `.parallax/` store** — one `<address>.dump` per
  account, itself a shareable artifact.
- **`Load::accounts("fixtures/pool.dump")`** installs a dump by path — no store,
  no network, ever.

### Outcomes

Every execution returns a `#[must_use]` `Outcome` of stable, backend-neutral
data. Assertions chain and panic with address-naming messages; reads return plain
values.

```rust,ignore
let out = test.send(withdraw);
out.succeeds()
   .has_lamports(recipient, 1_000_000)   // or: .fails_with(VaultError::Unauthorized)
   .has_tokens(vault, 600)
   .owned_by(vault, test.program_id());

for change in out.account_changes() {     // writable before/after, first-appearance order
    if change.was_created() { /* newly initialized this transaction */ }
}
```

`send` commits, `simulate` does not; each has an `_all` (chain) and a `_with` (raw
inputs) variant. Full surface in the [reference](docs/rust_reference.md#outcomes).

### Typed state

State is read and written with [wincode](https://docs.rs/wincode) — a standard,
not a framework — so any program's accounts decode.

```rust,ignore
test.write(vault, program_id, VaultState { authority, amount: 1_000 });

let state = test.read::<VaultState>(vault);   // Snapshot<T>, derefs to T
assert_eq!(state.amount, 1_000);
```

`has_state::<T>(addr, check)` asserts on decoded state inline. The trailing-bytes
rule and the Rust/TS owner asymmetry are in the
[reference](docs/rust_reference.md#typed-state-is-wincode-native).

### Adversarial testing

Signature checks are relaxed, so a test forges any signer without a keypair — what
is under test is the program's *own* authorization, not the runtime's.

```rust,ignore
// Prove the program rejects an attacker who merely *claims* to be the authority.
let attacker = test.add(Wallet::new());
let mut forged: Instruction = WithdrawInstruction { vault }.into();
forged.accounts.push(AccountMeta::new_readonly(attacker, true));  // spoofed signer

test.send(forged).fails_with(VaultError::Unauthorized);
```

For legitimate multisig members, `co_signers(&[..])` builds the read-only signer
metas; the harness backfills each as a funded account for free.

### Time control

```rust,ignore
test.warp_to_timestamp(1_800_000_000);   // set the clock's Unix timestamp
```

Nothing advances the clock implicitly; `sync_clock` (via a `Dump`) adopts a
dumped mainnet slot's time instead.

## Performance

Measured with `cargo test --release -- --ignored --nocapture bench_` and
`typescript/scripts/bench.mjs` on an Apple-silicon laptop — rerun to reproduce.

| Operation                                    | Measured  |
| -------------------------------------------- | --------- |
| `send` round-trip — Rust core                | ~5.2 µs   |
| `send` through the TypeScript kernel (Kit)   | ~6.5 µs   |
| typed-state `read` — Rust                    | ~180 ns   |

The TypeScript shell adds ~1.5 µs of FFI + wire tax over the core; a Web3.js send
is faster still, skipping the base58 round-trip a Kit `Address` pays.

## Documentation

- [`docs/rust_reference.md`](docs/rust_reference.md) — the Rust API.
- [`docs/typescript_reference.md`](docs/typescript_reference.md) — the Kit and
  Web3.js API.
- [`SKILL.md`](SKILL.md) — instructions for AI agents using Parallax.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
