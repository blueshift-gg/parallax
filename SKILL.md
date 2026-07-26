---
name: parallax
description: Test Solana programs with Parallax, the fixture-based LiteSVM harness — world setup, sending instructions, asserting outcomes, dumping mainnet state, from Rust or TypeScript.
---

# Testing Solana programs with Parallax

Use this skill when writing or fixing tests for a Solana program with the
`parallax-svm` crate (Rust) or `parallax-svm` npm package (Kit/Web3.js). The
APIs mirror each other; anything shown in one language exists in the other.

## Rust: the golden path

```toml
[dev-dependencies]
parallax-svm = "0.1"
```

Build the program first (`cargo build-sbf`); the harness discovers the artifact
under an ancestor `target/deploy` (or `PARALLAX_PROGRAM_PATH` when a runner
sets it).

```rust
use parallax_svm::prelude::*;

#[parallax_test]                       // program id defaults to crate::ID
fn deposits(test: &mut Test) {
    let user = test.add(Wallet::new());          // funded actor

    test.send(DepositInstruction { user, amount: 1_000_000_000 })
        .succeeds()
        .cu_at_most(10_000)
        .has_lamports(vault, 1_000_000_000);
}
```

`#[parallax_test(program_id = EXPR)]` targets another program.
`Test::builder(id)` configures by hand: `.rpc(url)`, `.compute_unit_limit(n)`,
`.program_bytes(elf)`, `.no_program()`, then `.build()`.

## TypeScript: the golden path

```ts
import { Test, wallet } from "parallax-svm/kit"; // or parallax-svm/web3.js

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/my_program.so");
const user = await test.add(wallet());
test.send(await client.createDepositInstruction({ user, amount: 1_000_000_000n }))
    .succeeds()
    .cuAtMost(10_000n);
```

The native kernel resolves from the installed platform package, or set
`PARALLAX_SVM_LIB` to a built `libparallax_svm_ffi` dylib.

## Fixtures (world setup)

`test.add(fixture)` installs and returns the address(es). Only install
PRE-STATE — accounts the instruction will CREATE must not be installed.

```rust
let user  = test.add(Wallet::new());                          // 10 SOL default
let payer = test.add(Wallet::new().fund(5_000_000_000));      // exact balance
let [a,b] = test.add(Wallet::accounts([ALICE, BOB]).fund(1_000_000_000)); // pinned
let mint  = test.add(Mint::new().with_authority(user).supply(1_000)
                       .with_holder([(user, 400)]));          // + funded ATA per holder
let [m1,m2] = test.add(Mint::new().supply(9).accounts::<2>());// N fresh mints
let ta    = test.add(TokenAccount::new(mint, user).amount(50));
let ata   = test.add(AssociatedTokenAccount::new(mint, user).amount(50));
test.add(Account::new(addr, owner_program, lamports, data));  // raw bytes
test.add(Program::new(id, elf));                              // extra program
```

## Mainnet state

```rust
let [pool, oracle] = test.add(Dump::accounts([POOL, ORACLE]));   // fetch once,
test.add(Dump::program(AMM_PROGRAM).sync_clock());               // then offline
test.add(Load::accounts("fixtures/pool.dump"));                  // from a file
```

First run fetches (one batched call, one slot) into a committed `.parallax/`
store; warm runs never touch the network. `sync_clock()` adopts the dumped
slot's clock. RPC comes from `Test::builder(id).rpc(url)`, default mainnet.

## Assertions

Chain off `send`/`simulate` (an unused `Outcome` is a compile error):

`succeeds()` · `fails(ProgramError::…)` · `fails_with(MyError::Code)` ·
`cu_at_most(n)` · `has_lamports(addr, n)` · `has_tokens(addr, n)` ·
`has_supply(mint, n)` · `has_state::<T>(addr, |s| …)` · `owned_by(addr, prog)` ·
`is_closed(addr)`. Reads: `account(addr)` (Option), `accounts()`, `logs()`,
`return_value()`, `compute_units()`, `account_changes()` (+ `was_created()` /
`was_removed()`). `simulate` never commits; `send_all` is atomic multi-ix.

## Typed state

```rust
test.write(addr, program_id, MyState { .. });   // wincode-serialize + install
let s = test.read::<MyState>(addr);             // decode full account data
```

Rust types derive wincode schemas; generated client account types already frame
their discriminator. TS uses generated `{Name}Account` codec bundles:
`test.read(VaultAccount, addr)` / `outcome.hasState(VaultAccount, addr, cb)`.

## Semantics you must not fight

- **Zero fees.** Exact-balance assertions work; nothing is deducted but what
  programs move.
- **Backfill is writable-first.** A missing WRITABLE account enters EMPTY (it's
  an init target — even when it signs); a missing READ-ONLY signer enters
  funded. Payers are world state: install them with `Wallet`.
- **Spoofed signers.** Any address can be a signer meta — no keypairs, ever.
  `co_signers(&[a, b])` builds read-only signer metas for remaining accounts.
- **Rent is real.** A transfer that leaves a 0-data account below ~890_880
  lamports fails (`InsufficientFundsForRent`). Deposit above the minimum.
- **Ownership is real.** A program debiting lamports from an account it does
  not own fails (`ExternalAccountLamportSpend`) — system-owned PDAs spend via
  a CPI signed with their seeds.
- **Determinism.** Identical runs are byte-identical; fixture addresses match
  across Rust/Kit/Web3.js. Never sleep/poll; set time with
  `test.warp_to_timestamp(ts)`.

## Common failures

| Error | Fix |
| --- | --- |
| `missing account …: add it to your dump accounts fixture` | Add the address to `Dump::accounts([...])` |
| `InsufficientFundsForRent` | Deposit/transfer amounts above the rent-exempt minimum |
| account not found after send | You installed an init target — remove the fixture, let the program create it |
| wrong-type `read` panic (non-zero trailing bytes) | You read the wrong type for that account |
| TS: cannot find native library | Set `PARALLAX_SVM_LIB` or install the platform package |

Full surface: [docs/rust_reference.md](docs/rust_reference.md) ·
[docs/typescript_reference.md](docs/typescript_reference.md)
