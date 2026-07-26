# parallax-svm

<p>
  <a href="../README.md#license"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" /></a>
  <!-- Activates at first publish:
  <a href="https://www.npmjs.com/package/parallax-svm"><img alt="npm" src="https://img.shields.io/npm/v/parallax-svm?logo=npm" /></a>
  -->
</p>

The TypeScript half of [Parallax](https://github.com/blueshift-gg/parallax), a
fixture-based testing harness for
[LiteSVM](https://github.com/LiteSVM/litesvm). The Kit and Web3.js adapters are
thin shells over the same Rust core that backs the Rust crate, so all three
harnesses share one implementation of every semantic.

```bash
npm install --save-dev parallax-svm @solana/kit
```

```ts
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/vault.so");
const user = await test.add(wallet({ fund: 1_000_000n }));
const deposit = await new VaultClient().createDepositInstruction({ user, amount: 1_000n });

test
  .send(deposit)
  .succeeds()
  .hasLamports(deposit.vaultAddress, 1_000n)
  .cuAtMost(10_000n);
```

Import from `parallax-svm/kit` for Kit's `Address`/`Account` types, or
`parallax-svm/web3.js` for the Web3.js equivalents; the API is otherwise
identical, and the member names match the Rust harness camel-cased. Fixture
addresses are deterministic and identical across all three harnesses — a value
one computes matches the others'. A send runs in ~6.5 µs through the native
kernel.

## Documentation

- **[reference](docs/reference.md)** — the full TypeScript surface: opening a
  world, fixtures, dump/load, outcomes, the `AccountCodec`, native-library
  resolution.
- **[root README](../README.md)** — the shared model and the pitch.
- **[design & guarantees](../docs/design.md)** — determinism, the zero-fee model,
  spoofed signers, backfill.

Licensed under either of Apache-2.0 or MIT at your option.
