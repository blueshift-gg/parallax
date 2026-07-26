---
name: parallax
description: Write, run, or fix Solana program tests with Parallax (parallax-svm), the fixture-based LiteSVM harness — world setup, sending instructions, asserting outcomes, checks and invariants, mainnet dumps, in Rust or TypeScript. Also covers building and smoke-testing the harness itself.
---

# Testing Solana programs with Parallax

One mental model: **install pre-state as fixtures, send instructions, assert
the outcome.** `test.add` is the only setup primitive; every execution returns
an `Outcome` you chain assertions on. The Rust and TypeScript APIs mirror each
other — anything shown in one exists in the other.

## Golden path (Rust)

`cargo build-sbf` the program first; the harness finds the artifact under an
ancestor `target/deploy` (or `PARALLAX_PROGRAM_PATH` when a runner sets it).

```rust
use parallax_svm::prelude::*;

#[parallax_test]                        // program id defaults to crate::ID
fn deposits(test: &mut Test) {
    let user = test.add(Wallet::account());

    test.send(DepositInstruction { user, amount: 1_000_000_000 })
        .succeeds()
        .check([CuBudget::le(10_000), Lamports::eq(vault, 1_000_000_000)]);
}
```

`#[parallax_test(program_id = EXPR)]` targets another program. `Test::builder(id)`
configures by hand: `.rpc(url)`, `.compute_unit_limit(n)`, `.program_bytes(elf)`,
`.no_program()`, then `.build()`.

## Golden path (TypeScript)

```ts
import { Test, wallet } from "parallax-svm/kit"; // or parallax-svm/web3.js

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/my_program.so");
const user = await test.add(wallet());
test.send(await client.createDepositInstruction({ user, amount: 1_000_000_000n }))
    .succeeds()
    .check(CuBudget.le(10_000n));
```

The native kernel resolves from the installed platform package, or set
`PARALLAX_SVM_LIB` to a built `libparallax_svm_ffi` dylib.

## Fixtures: install pre-state, never init targets

Accounts the instruction will CREATE must not be installed — a missing writable
account enters empty on purpose (see semantics below). Pick by what you need:

| You need | Rust | TypeScript |
| --- | --- | --- |
| A funded actor | `Wallet::account()` (10 SOL) · `.fund(n)` exact · `.at(addr)` pinned | `wallet()` / `wallet({ fund, address })` |
| Actors at known addresses | `Wallet::accounts([A, B]).fund(n)` | `wallet({ accounts: [A, B], fund })` |
| A mint | `Mint::account().supply(n).decimals(d).with_authority(a)` | `mint({ supply, decimals, authority })` |
| A funded holder set | `Mint::account().supply(1_000).with_holder([(user, 400)])` — one funded ATA per holder | `mint({ supply, holders: [[user, 400n]] })` |
| N fresh mints | `let [a, b] = test.add(Mint::accounts().supply(9));` — N inferred from the pattern | `mint({ count: 2 })` |
| A token account | `TokenAccount::account(mint, owner).amount(n)` · pinned plural `TokenAccount::accounts([A, B], mint, owner)` | `tokenAccount(mint, owner, { amount })` |
| An ATA | `AssociatedTokenAccount::account(mint, owner).amount(n)` | `associatedTokenAccount(mint, owner, { amount })` |
| Raw bytes | `Account::new(addr, owner_program, lamports, data)` | `account({ address, owner, data })` |
| A CPI callee | `Program::new(id, elf)` | `program(id, elf)` |

Every fixture returns its address(es) from `test.add` — thread the handles,
never hardcode. Protocol fixtures are plain types implementing `Fixture`
(`install(test)`), composing the built-ins.

## Mainnet state

```rust
let [pool, oracle] = test.add(Dump::accounts([POOL, ORACLE]));  // fetch once,
test.add(Dump::program(AMM_PROGRAM).sync_clock());              // then offline
test.add(Load::accounts("fixtures/pool.dump"));                 // from a file
```

First run fetches (one batched call, one slot) into a committed `.parallax/`
store; warm runs never touch the network. `sync_clock()` adopts the dumped
slot's clock. RPC comes from `Test::builder(id).rpc(url)`, default mainnet.
TS: `dump({ accounts })`, `dump.program(id)`, `load({ path })`.

## Asserting

Chain off `send`/`simulate` — an unasserted `Outcome` is a compile error in
Rust. `send` commits, `simulate` never does; `…_all` variants run an atomic
instruction chain, `…_with` variants take raw transaction-input accounts.

**The verdict is a method; every fact is a check value.** One grammar:

```rust
test.send(withdraw)
    .succeeds()                          // .fails(ProgramError::…) / .fails_with(MyError::Code)
    .check([
        CuBudget::le(20_000),            // eq/le/lt/ge/gt on all numeric facts
        Lamports::eq(addr, n), Tokens::eq(addr, n), Supply::eq(mint, n),
        Owner::eq(addr, program),
        Data::eq(addr, bytes), ReturnData::eq(bytes),
        Changes::eq([user, vault]),      // exact changed set, in order
        Changes::created(vault),         // and removed(a) / closed(a)
    ])
    .check(State::eq(vault, Vault { authority, amount: 600 }));  // typed, T inferred
```

TS mirrors every namespace (`CuBudget.le`, `Lamports.eq`, `State.eq(codec, addr, value)`,
`Changes.eq([..])`). `State::with::<T>(addr, |s| ..)` asserts partial facts.

Reads (not asserts): `account(addr)`, `accounts()`, `logs()`, `return_value()`,
`compute_units()`, `events(decode)`, `account_changes()`.

**Invariants.** Register any check — built-in, closure, or your own `Check`
struct — to run after every committed send (never on simulations):

```rust
test.invariant(Solvent { pool });        // enforced on every send from here on
```

## Typed state

```rust
test.write(addr, program_id, MyState { .. });   // wincode-serialize + install
let s = test.read::<MyState>(addr);             // decode full account data
```

Rust types derive wincode schemas; generated client account types frame their
own discriminator. TS uses generated `{Name}Account` codec bundles:
`test.read(VaultAccount, addr)` / `State.with(VaultAccount, addr, cb)`.

## Semantics you must not fight

- **Zero fees.** Exact-balance assertions work; nothing is deducted but what
  programs move.
- **Backfill is writable-first.** A missing WRITABLE account enters EMPTY (it's
  an init target — even when it signs); a missing READ-ONLY signer enters
  funded. Payers are world state: install them with `Wallet`.
- **Spoofed signers.** Any address can be a signer meta — no keypairs, ever.
  `co_signers(&[a, b])` builds read-only signer metas.
- **Rent is real.** A transfer leaving a 0-data account below ~890_880 lamports
  fails (`InsufficientFundsForRent`). Deposit above the minimum.
- **Ownership is real.** A program debiting lamports from an account it does
  not own fails (`ExternalAccountLamportSpend`) — system-owned PDAs spend via a
  CPI signed with their seeds.
- **Determinism.** Identical runs are byte-identical; fixture addresses match
  across Rust/Kit/Web3.js. Never sleep or poll; set time with
  `test.warp_to_timestamp(ts)`.

## When a test fails

| Error | Fix |
| --- | --- |
| `missing account …: add it to your dump accounts fixture` | Add the address to `Dump::accounts([...])` |
| `InsufficientFundsForRent` | Deposit/transfer above the rent-exempt minimum |
| account not found after send | You installed an init target — remove the fixture, let the program create it |
| wrong-type `read` panic (non-zero trailing bytes) | You read the wrong type for that account |
| invariant panic on an unrelated send | The invariant asserts an account absent from that transaction — guard on `outcome.account(..)` presence |
| `outcome does not contain account …` | The checked account is not in the transaction — add it to the instruction or read world state via `test` |
| TS: cannot find native library | Set `PARALLAX_SVM_LIB` or install the platform package |

Failure output includes program logs and, in dump worlds, a hint naming the
first likely-missing account.

Full surface: [docs/rust_reference.md](../../../docs/rust_reference.md) ·
[docs/typescript_reference.md](../../../docs/typescript_reference.md)

## Hacking on the harness itself

```bash
bash .claude/skills/parallax/smoke.sh
```

Runs the Rust suites, builds the FFI kernel dylib, and runs the TypeScript
fixture harness against it (program-less worlds, no artifact needed). Set
`PARALLAX_PROGRAM_PATH=<a compiled .so>` to extend it to the program-parity
suites. Ends with `SMOKE OK` when the harness is healthy on this machine.
