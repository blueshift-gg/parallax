<h1 align="center">Parallax</h1>

<!-- Logo drops in here once one exists: <p align="center"><img width="380" alt="Parallax" src="..." /></p> -->

<p align="center">
  <b>Fixture-based testing for Solana programs — one test model, three languages, the real runtime.</b>
</p>

<p align="center">
  <a href="#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" /></a>
  <!-- Badges below activate at first publish — keep commented until the packages/CI exist:
  <a href="https://crates.io/crates/parallax-svm"><img src="https://img.shields.io/crates/v/parallax-svm?logo=rust" /></a>
  <a href="https://www.npmjs.com/package/parallax-svm"><img src="https://img.shields.io/npm/v/parallax-svm?logo=npm" /></a>
  <a href="https://github.com/blueshift-gg/parallax/actions"><img src="https://img.shields.io/github/actions/workflow/status/blueshift-gg/parallax/ci.yml?logo=github" /></a>
  -->
</p>

Parallax makes an on-chain program test read like an ordinary test: name your
actors, install fixtures, send an instruction, assert on a structured outcome.
The same model runs from Rust, [Kit](https://github.com/anza-xyz/kit), and
Web3.js — over [LiteSVM](https://github.com/LiteSVM/litesvm) and the real Agave
runtime — so a contract verified in Rust behaves identically when a TypeScript
client drives it. That forced agreement is the name: one program, three vantages.

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

- **The real runtime, not a mock.** Rent, ownership, account rules, and signature
  logic are enforced exactly as on mainnet — the Agave runtime under LiteSVM.
- **One model, three languages.** Rust, Kit, and Web3.js share member names and
  **byte-identical, deterministic worlds** — a fixture address one computes
  matches the others', so tests port by construction.
- **Fast.** ~5 µs per `send` through the Rust core, ~6.5 µs through the
  TypeScript kernel, ~180 ns for a typed-state read. ([numbers](#performance))
- **Spoofed signers, zero fees.** Name any address as a signer without a keypair;
  no fees are charged, so a balance only ever moves because a program moved it.
- **Mainnet state in two lines.** Dump real accounts or programs from a live
  cluster — the first run fetches, every later run is offline and deterministic.
- **Framework-agnostic.** Typed state is [wincode](https://docs.rs/wincode)- and
  codec-based, so any Solana program's accounts decode — no framework lock-in.

## The same test, both languages

The parity is the point — one deposit test, in Rust and in TypeScript.

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
the [reference](docs/reference.md#fixtures-are-values).

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
inputs) variant. Full surface in the [reference](docs/reference.md#outcomes).

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
[reference](docs/reference.md#typed-state-is-wincode-native).

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

## Relationship to LiteSVM and Mollusk

Parallax is a **harness**, not an engine.
[LiteSVM](https://github.com/LiteSVM/litesvm) executes the transactions; Parallax
is the fixture, assertion, and cross-language layer that makes a test read like a
test. [Mollusk](https://github.com/anza-xyz/mollusk) is an alternative engine in
the same space; Parallax sits above either, drives LiteSVM today, and keeps the
backend private so the public test API can outlast the engine beneath it.

## Documentation

- **[docs/reference.md](docs/reference.md)** — the full Rust API: fixtures,
  dump/load, outcomes, typed state, program loading.
- **[docs/design.md](docs/design.md)** — the harness contracts (determinism, zero
  fees, backfill) and the design rules that double as the contribution bar.
- **[typescript/README.md](typescript/README.md)** — the Kit and Web3.js harness,
  with its own [reference](typescript/docs/reference.md). API docs land on docs.rs
  and npm once published.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at
your option.
