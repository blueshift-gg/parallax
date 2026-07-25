# parallax-svm

The TypeScript half of [Parallax](https://github.com/blueshift-gg/parallax), a
fixture-based testing harness for Solana programs on LiteSVM. It is the Kit and
Web3.js sibling of the Rust crate, and a thin shell over the same Rust core:
every harness semantic — fixture placement, deterministic addresses, backfill,
the fee model, account-change tracking, and error mapping — lives in the
`parallax-svm-ffi` native kernel, reached through a small binary wire format.
The shell only converts Kit/Web3.js types to and from that wire, so a program is
exercised from three vantage points (Rust, Kit, Web3.js) that agree by
construction.

```bash
npm install --save-dev parallax-svm @solana/kit
```

```ts
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/vault.so");
const user = await test.add(wallet({ fund: 1_000_000n }));
const client = new VaultClient();
const deposit = await client.createDepositInstruction({ user, amount: 1_000n });
test
  .send(deposit)
  .succeeds()
  .hasLamports(deposit.vaultAddress, 1_000n)
  .cuAtMost(10_000n);
```

Actors are `wallet()` fixtures: `test.add(wallet({ fund }))` installs a funded
account and returns its address. A transaction may still name a signer the world
never installed — a read-only co-signer such as a multisig member is auto-funded
on `send` — but an account that pays or is created is world state, so a payer
needs a `wallet()` and an init target enters empty. This mirrors the Rust
harness exactly.

Use `parallax-svm/web3.js` for the same API with Web3.js address, account, and
instruction types. Fixture addresses are deterministic and match between the
Kit, Web3.js, and Rust harnesses.

Built-in fixtures are `wallet`, `mint`, `tokenAccount`,
`associatedTokenAccount`, and `program`. Application fixtures are ordinary
objects implementing `Fixture`; `test.add` is the only composition primitive.
Typed account state is read and written through a structural `AccountCodec`
(`decode`/`encode`, and optional `owner`, `discriminator`, and `size` framing),
so a generated client's codec plugs straight into `read`, `write`, `hasState`,
`accountAs`, `events`, and `returnValue`.

`send`, `sendAll`, and `simulate` return `Outcome`. Its stable assertions are
`succeeds`, `fails`, `failsWith`, `cuAtMost`, `hasLamports`, `hasTokens`,
`hasSupply`, `hasState`, and `isClosed`; `accountChanges` reports writable
before/after state in instruction order.

Pass `{ computeUnitLimit: 200_000n }` as the third `Test` constructor argument
or `Test.open` option to set the same per-transaction ceiling as Rust's
`Test::builder(...).compute_unit_limit(...)`.

`Test.open(PROGRAM_ADDRESS, programPath?)` resolves the program artifact through
a discovery chain, mirroring the Rust harness's `setup.rs`: an explicit
`programPath` wins; otherwise `PARALLAX_PROGRAM_PATH`, the same environment
variable the Rust harness uses to locate a freshly built program; failing both,
an actionable error. Passing the ELF path explicitly keeps direct test-runner
invocation straightforward.

## Native library

The shell loads the `parallax-svm-ffi` shared library, resolved in this order:

1. `PARALLAX_SVM_LIB`, if set, as an explicit path to the shared library.
2. The platform package for your OS and architecture
   (`parallax-svm-darwin-arm64`, `parallax-svm-linux-x64-gnu`, and so on),
   installed automatically as an optional dependency.
3. The repo-local build at `../target/release/libparallax_svm_ffi.*`, so
   `cargo build --release -p parallax-svm-ffi` from the repository root enables
   local development against an unpublished kernel.

Licensed under either of Apache-2.0 or MIT at your option.
