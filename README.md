# Parallax

**A fixture-based testing harness for [LiteSVM](https://github.com/LiteSVM/litesvm).**

The name is the pitch: the same program, observed from multiple vantage points —
Rust, [Kit](https://github.com/anza-xyz/kit), and Web3.js — must agree. Parallax
gives all three the same test model over the same engine, so a contract you
verify in Rust behaves identically when a TypeScript client drives it.

Parallax makes an on-chain program test read like an ordinary test: name your
actors, install some fixtures, send an instruction, and assert on a structured
outcome. It never exposes the SVM underneath — tests depend on Solana's public
`Instruction`/`Pubkey` types and Parallax's own `Test`, `Fixture`, `Outcome`,
and `ProgramError` contracts, nothing more.

## Rust

```toml
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
compiled program into an isolated `Test` world. Filters, `#[ignore]`,
`#[should_panic]`, and `Result<(), E>` returns all work normally. The program
artifact is discovered under an ancestor `target/deploy`, or taken from
`PARALLAX_PROGRAM_PATH` when a test runner sets it.

## TypeScript

```bash
npm install --save-dev parallax-svm @solana/kit
```

```ts
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/vault.so");
const authority = await test.add(wallet());
const deposit = await new VaultClient().createDepositInstruction({
  user: authority,
  amount: 1_000_000_000n,
});

test.send(deposit).succeeds().cuAtMost(10_000n);
```

`parallax-svm/kit` and `parallax-svm/web3.js` are the Kit and Web3.js adapters.
Fixture addresses are deterministic and **identical across all three harnesses**,
so a value the Rust test computes matches the one the TypeScript test does. See
[`typescript/`](typescript) for the package.

## Fixtures are values

Setup is data, not a DSL. `Wallet`, `Mint`, `TokenAccount`,
`AssociatedTokenAccount`, `Account`, and `Program` cover common ground, and
`test.add` is the only composition primitive:

```rust,ignore
let [alice, bob, carol] = test.add([Wallet::new(); 3]);

let mint = test.add(
    Mint::new().with_authority(alice).supply(1_000).with_holder([(bob, 400)]),
);
```

An application composes those built-ins behind its own `Fixture` for protocol
state, and threads back the addresses each fixture returns rather than pinning
them up front. A signer a transaction names but never installs is auto-funded on
`send`, so co-signers cost nothing extra; a writable account it never installs
enters as an empty init target.

## Typed state is wincode-native

Parallax reads and writes account state with
[wincode](https://docs.rs/wincode) — a serialization standard, not a framework —
so the harness stays program-agnostic:

```rust,ignore
// `write` serializes with wincode and installs a rent-exempt account owned by
// an explicit program. A generated account type emits its discriminator as the
// leading schema field, so the account frames exactly as the program writes it.
test.write(vault_address, program_id, VaultState { authority, amount: 1_000 });

// `read` decodes the full account data back through the same schema.
let vault = test.read::<VaultState>(vault_address);
assert_eq!(vault.amount, 1_000);

// `read_at` decodes a suffix, for a type that covers only part of the data.
let amount = test.read_at::<u64>(vault_address, DISCRIMINATOR_LEN + 32);

// Assertions chain off an outcome; ownership is orthogonal to decoding.
test.send(withdraw)
    .succeeds()
    .has_state::<VaultState>(vault_address, |v| assert_eq!(v.amount, 600))
    .owned_by(vault_address, program_id);
```

Because a generated client's account schema encodes its discriminator as the
first field, decoding a full on-chain account with `read::<T>` Just Works — the
same bytes the program wrote, decoded straight back into the client type. The
TypeScript half offers the same shape through a structural `AccountCodec`.

`derive_pda(&[&[u8]])` (and `derive_pda_with_bump`) derive program addresses
from raw seed slices under the program under test.

## Relationship to LiteSVM and Mollusk

Parallax is a **harness**, not an engine. [LiteSVM](https://github.com/LiteSVM/litesvm)
executes the transactions; Parallax is the fixture, assertion, and
cross-language layer on top — the part that makes a test read like a test.
[Mollusk](https://github.com/anza-xyz/mollusk) is an alternative engine in the
same space as LiteSVM; Parallax sits a level above either, and today drives
LiteSVM. The backend is deliberately private so the public test API can outlast
the engine beneath it.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
