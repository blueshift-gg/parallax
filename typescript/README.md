# parallax-svm

The TypeScript half of [Parallax](https://github.com/blueshift-gg/parallax), a
fixture-based testing harness for Solana programs on LiteSVM. It is the Kit and
Web3.js sibling of the Rust crate: both expose the same test model — an isolated
`Test` world, composable fixtures, and structured outcomes — over the same
LiteSVM engine, so a program can be exercised from three vantage points (Rust,
Kit, Web3.js) that must agree.

```bash
npm install --save-dev parallax-svm @solana/kit
```

```ts
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.load(PROGRAM_ADDRESS, "target/deploy/vault.so");
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
or `Test.load` option to set the same per-transaction ceiling as Rust's
`Test::builder(...).compute_unit_limit(...)`.

`Test.load(PROGRAM_ADDRESS)` reads `PARALLAX_PROGRAM_PATH`, the same environment
variable the Rust harness uses to locate a freshly built program. Passing the
ELF path explicitly keeps direct test-runner invocation straightforward.

Licensed under either of Apache-2.0 or MIT at your option.
