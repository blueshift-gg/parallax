# Parallax reference (Rust)

The full API surface of the Rust harness. The [README](../README.md) is the
tour; this is the manual. The TypeScript surface is documented in
[`docs/typescript_reference.md`](typescript_reference.md), and the
harness contracts — determinism, the zero-fee model, backfill — in
the [Guarantees](#guarantees) section below.

Everything here is reached through the prelude:

```rust,ignore
use parallax_svm::prelude::*;
```

## Opening a world

`#[parallax_test]` expands to a plain `#[test]` that loads the current crate's
compiled program into an isolated `Test` world. Filters, `#[ignore]`,
`#[should_panic]`, and `Result<(), E>` returns all work normally. The program id
defaults to `crate::ID`; a test for another program uses
`#[parallax_test(program_id = EXPR)]`. The artifact is discovered under an
ancestor `target/deploy` (preferring `{crate}.so` in a multi-program workspace),
or taken from `PARALLAX_PROGRAM_PATH` when a test runner sets it.

Drop to `Test::builder(id)` when a test needs to configure the world by hand:

```rust,ignore
let mut test = Test::builder(MY_PROGRAM_ID)
    .compute_unit_limit(200_000)   // per-transaction CU ceiling
    .rpc("https://my-rpc.example") // endpoint Dump fixtures fetch from
    .build()                       // -> Result<Test, SetupError>
    .unwrap();
```

`builder` also takes `.program_path(p)` / `.crate_name(n)` to steer discovery,
`.program_bytes(elf)` to load an in-memory artifact, and `.no_program()` for a
world with only the runtime built-ins (System, Token, Token-2022, ATA).
`Test::new(id)` is `builder(id).build()` unwrapped; `Test::try_new(id)` returns
the `Result`. `set_compute_unit_limit(limit)` reconfigures the ceiling on an
already-built world.

## Fixtures are values

Setup is data, not a DSL. A fixture is any value implementing `Fixture`, and
`test.add` is the only composition primitive — it installs the fixture and
returns the address(es) it placed, so tests thread handles back instead of
pinning addresses up front.

```rust,ignore
let authority = test.add(Wallet::account());             // funded actor, fresh address
let poor = test.add(Wallet::account().fund(0));           // exact balance
let pinned = test.add(Wallet::account().at(SOME_KEY));    // specific address

let mint = test.add(
    Mint::account()                                   // fixed-supply, 6-decimal legacy mint
        .with_authority(authority)                    // ...now mintable
        .with_freeze_authority(authority)
        .with_supply(1_000)
        .decimals(9)
        .token_program(TokenProgram::Token2022)
        .with_holder([(alice, 400), (bob, 600)]),     // one ATA per holder, funded
);

let vault = test.add(TokenAccount::account(mint, authority).with_amount(600));
let ata = test.add(AssociatedTokenAccount::account(mint, authority).with_amount(400));
```

The built-ins are `Wallet`, `Mint`, `TokenAccount`, `AssociatedTokenAccount`,
`Account` (a raw account — its fields are public, and it is itself a fixture),
and `Program` (preload compiled ELF for CPI). Every builder fixture enters
through an arity verb — `account(..)` yields one address, `accounts(..)` several
— the same grammar `Dump` and `Load` speak. Entry constructors take only what is
conceptually required (`TokenAccount::account(mint, owner)`); everything
optional is a builder method.

### The `accounts` plural vocabulary

Installing several of one fixture reads as a plural, and `test.add` destructures
the fixed-arity result:

```rust,ignore
// Fresh Copy fixtures: array-repeat already works.
let [alice, bob, carol] = test.add([Wallet::account().fund(7); 3]);

// Pinned plural — the addresses lead, one config applied at each (mirrors Dump::accounts):
let [a, b] = test.add(Wallet::accounts([ADDR_A, ADDR_B]).fund(5_000));
let [ta, tb] = test.add(TokenAccount::accounts([TA, TB], mint, owner).with_amount(42));

// Fresh plural — N distinct mints sharing one config. N is inferred from the
// destructuring pattern, so no count is ever written:
let [m1, m2] = test.add(Mint::accounts().with_supply(1_000));
```

`Wallet`/`TokenAccount` are `Copy`, so their *fresh* plural is just
`[Wallet::account(); N]`; their `accounts([..])` entry is the *pinned* plural.
`Mint` is not `Copy`, so `Mint::accounts()` is its fresh plural.
`AssociatedTokenAccount` deliberately has **no** plural: an ATA address is a pure
function of owner and mint, so "several ATAs" only means several owners — which
is exactly `Mint::with_holder`'s job.

### Composition: tuples, holdings, closure worlds

```rust,ignore
let [mint_a, mint_b] = test.add(Mint::accounts());
let (maker, taker) = test.add((
    Wallet::account().holding(mint_a, 1_000_000_000),
    Wallet::account().holding(mint_b, 1_000_000_000),
));
```

Tuples install heterogeneous fixtures in order and destructure their handles;
`holding` installs a funded associated token account inside the actor — the
actor-centric dual of `with_holder`. Closures are fixtures too: a world is a
plain function receiving `&mut Test`, and because it can call
`test.invariant(..)` while it builds, `test.add(escrow_world(1_000))` yields a
**self-verifying world** — names, state, and standing laws in one value.

### Application fixtures

An application composes the built-ins behind its own `Fixture` for protocol
state:

```rust,ignore
struct FundedVault { deposit: u64 }

impl Fixture for FundedVault {
    type Output = (Pubkey, Pubkey); // (authority, vault)

    fn install(self, test: &mut Test) -> Self::Output {
        let program = test.program_id();
        let authority = test.add(Wallet::account());
        let vault = test.derive_pda(&[b"vault".as_ref(), authority.as_ref()]);
        test.write(vault, program, VaultState { authority, amount: self.deposit });
        (authority, vault)
    }
}

let (authority, vault) = test.add(FundedVault { deposit: 1_000 });
```

## Dump & load real state

`Dump` copies real accounts (or a real program) from a live cluster into the
world, so a fixture can exercise on-chain state the harness would otherwise have
to hand-build. The copied bytes land in a committed `.parallax/` store next to
the project manifest — one `<address>.dump` file per primary, in a wincode
format — so the **first** run fetches once and every later run is fully offline
and deterministic.

```rust,ignore
#[parallax_test]
fn swaps_against_a_real_pool(test: &mut Test) {
    // Fetched once, at one slot, then served from `.parallax/` offline.
    let [pool, oracle] = test.add(Dump::accounts([POOL, ORACLE]));
    test.add(Dump::program(AMM_PROGRAM));

    test.send(SwapInstruction { pool, oracle }).succeeds();
}
```

- **The network is touched only on a miss.** `Dump::accounts([..])` returns the
  same addresses in the same arity. On a warm store, resolution is a pure disk
  read — no socket opens. On a miss, the misses are resolved in **one batched
  `getMultipleAccounts`** at **one slot**, written to the store, and the run goes
  green. The RPC endpoint comes from `Test::builder(id).rpc(url)` and defaults to
  public mainnet-beta — code-only, with no environment override.
- **Programs dump coherently.** `Dump::program(id)` fetches the executable
  account and, for the upgradeable loader, its programdata in the same batch,
  then loads it as a usable program and returns the id.
- **Slot coherence is guarded.** `Dump::refresh_all()` re-fetches every stored
  entry in one coherent batch onto a single recent slot. If a world combines
  entries whose slots span more than one epoch (~432k slots), it warns once and
  points at `refresh_all`.
- **Failures are guided.** In a world that has dumps, a transaction that fails on
  a read-only account the world never installed appends a hint naming that
  address and suggesting you add it to the dump fixture.
- **Clock is opt-in.** `Dump::accounts([..]).sync_clock()` (and the same on
  `Dump::program`) adopts the dumped slot's clock; off by default.

`Load` installs from an already-dumped file at an explicit path — no store, no
network, ever. Because a store file *is* a dump file, any `.parallax/` file is a
shareable artifact: copy it out (or commit it anywhere) and `Load` it by path to
share fixtures across tests, machines, and languages.

```rust,ignore
let accounts = test.add(Load::accounts("fixtures/pool.dump")); // -> Vec<Pubkey>
test.add(Load::program("fixtures/amm.dump"));                  // -> Pubkey
```

## Outcomes

Every execution returns an `Outcome`. It is `#[must_use]` — the whole point is to
assert on it — and it holds only stable, backend-neutral data. Assertions panic
with actionable messages (and are chainable, returning `&Self`); reads return
plain values or `Option`.

`succeeds()` yields a **transaction witness** — checks only exist on a proven
success, so an account assert on a failed transaction is a compile error.
Every fact takes its subject and one *expectation*: a plain value means
equality, a closure is a predicate over the measured value:

```rust,ignore
test.send(withdraw).succeeds().checks([
    Cu::spent(|cu| cu <= 20_000),
    Account::lamports(recipient, 1_000_000),          // value ⇒ equality
    Account::lamports(user, |x| x > 0),               // predicate ⇒ anything
    Account::owner(vault, program_id),
    Account::data(vault, |v: &Vault| v.amount > 600), // decoded through T's schema
    Account::data(config, [1, 0, 0, 0]),              // raw bytes
    Account::created(vault),                          // and removed / closed
    Mint::supply(mint, 1_000),
    TokenAccount::amount(user_ata, |x| x >= 500),
    ReturnData::is([7]),
]);
```

Everything is one type, [`CheckFn`] — leaf facts, `bundle([..])` groups, and
`CheckFn::new(|tx| ..)` closures over the whole transaction — so all of it
nests and mixes freely, and a protocol's standard assertions become a plain
function returning a bundle. Value expectations fail with `expected N, got M`;
predicates cannot print their source, so they fail with the exact location of
the line that built them plus the actual value. `fails(..)`/`fails_with(..)`
yield a failed-transaction witness with the reads (logs, error, compute
units) and deliberately no checks.

### Bundles and invariants

`bundle([..])` turns several checks into one; `test.invariant(..)` registers
any check — fact, bundle, or `CheckFn::new` closure — to run after every
*successful* committed send (failed sends commit nothing; simulations never
run invariants), so a protocol invariant is written once and enforced
everywhere:

```rust,ignore
fn solvent(pool: Pubkey) -> CheckFn {
    Account::data(pool, |p: &Pool| p.reserves >= p.obligations)
}

test.invariant(solvent(pool));                    // every send now enforces it
test.send(swap)
    .succeeds()
    .check(CheckFn::new(|tx| assert_eq!(tx.logs().len(), 3)));
```

An invariant sees each send's witness, so the accounts it judges must be part
of that transaction; guard on `tx.account(..)` presence when an invariant
only sometimes applies.

Reads pull structured data back out:

```rust,ignore
let out = test.simulate(instruction);
out.logs();                     // &[String], execution order
out.compute_units();            // u64
out.return_value(decode);       // Option<T> from return data
out.account(address);           // Option<&Account> post-state
out.account_as(address, decode);
out.accounts();                 // &[Account], first-appearance order
out.events(decode);             // Vec<T> from sol_log_data payloads
for change in out.account_changes() {   // writable before/after, first-appearance order
    change.was_created();       // bool — absent before, present after
    change.was_removed();       // bool — present before, absent after
    change.before();            // Option<&Account>
    change.after();
}
```

### Execution matrix

`send` commits; `simulate` does not. Each has an `…_all` variant (an atomic
instruction chain) and a `…_with` variant (raw transaction-input accounts,
useful when malformed input *is* the test case), and both compose:

|            | one instruction   | instruction chain    | + explicit inputs   |
| ---------- | ----------------- | -------------------- | ------------------- |
| commit     | `send`            | `send_all`           | `send_with` / `send_all_with`     |
| simulate   | `simulate`        | `simulate_all`       | `simulate_with` / `simulate_all_with` |

### Errors

`ProgramError` is a stable, non-exhaustive enum that never exposes the backend
type. Named variants cover the common runtime errors (`InsufficientFunds`,
`MissingRequiredSignature`, `InvalidAccountData`, `AccountAlreadyInitialized`,
…); `Custom(u32)` carries a program-defined code (assert it ergonomically with
`fails_with`, which takes anything `Into<u32>`); and `Runtime(String)` catches
anything outside the stable set. `co_signers(&[Pubkey])` builds read-only signer
metas for authorities like multisig members.

## Typed state is wincode-native

Parallax reads and writes account state with
[wincode](https://docs.rs/wincode) — a serialization standard, not a framework —
so the harness stays program-agnostic. A generated client's account type encodes
its discriminator as the leading schema field, so an on-chain account decodes
straight back into the client type.

```rust,ignore
// `write` serializes with wincode and installs a rent-exempt account owned by an
// explicit program (the substrate carries no ownership of its own). Returns the address.
test.write(vault, program_id, VaultState { authority, amount: 1_000 });

// `read` decodes the full account data back through the same schema.
let vault_state = test.read::<VaultState>(vault);
assert_eq!(vault_state.amount, 1_000);

// The returned Snapshot<T> derefs to T and also reports where it was read:
assert_eq!(vault_state.address(), vault);
assert!(vault_state.lamports() > 0);

// `read_at` decodes a suffix — a type covering only the bytes after a
// discriminator the caller frames separately.
let amount = test.read_at::<u64>(vault, DISCRIMINATOR_LEN + 32);
```

**Trailing-bytes rule.** wincode reads exactly `T`'s bytes and stops. Any
unconsumed tail must be **all zero** — Solana's zero-initialized reserved padding,
as a growable or migration-target account carries. A *non-zero* trailing byte is
the fingerprint of the wrong or a stale type read against the account, and
**panics** rather than silently returning a value decoded from a prefix. The same
contract applies to `read_at`'s suffix and to the typed `Account::data` checks.

**Owner is orthogonal in Rust.** A wincode read frames bytes only and never
checks the account's owner, so pair it with `Account::owner` when ownership matters.
This differs from TypeScript by design: there, codecs carry and validate `owner`
because generated bundles are self-framing.

`derive_pda(&[&[u8]])` (and `derive_pda_with_bump`) derive program addresses
from raw seed slices under the program under test; `derive_ata(owner, mint,
token_program)` derives an associated-token address without installing it.

### Loading programs: which to reach for

- **`Program::new(id, elf)`** / **`preload_program(id, elf)`** — you already hold
  the compiled bytes (a sibling CPI program, `include_bytes!`).
- **`Dump::program(id)`** — pull a real program from the network into the store.
- **`Load::program(path)`** — install a program from a dump file on disk.
- The primary program under test needs none of these — `#[parallax_test]` and
  `Test::builder` load it for you.

## Guarantees

- **Zero fees.** The runtime charges no signature or write-lock fees. A balance
  only moves when a program moves it.
- **Spoofed signers need no keypairs.** Signature checks are relaxed, so a test
  names any address as a signer without producing a key. A permissionless
  transaction borrows an inert internal fee payer.
- **Signer backfill vs. init targets (the writable-first rule).** An account a
  transaction names but the world never installed is filled in on `send` by a
  single rule, checking *writable first*: a **writable** account is an init
  target and enters **empty** (even when it also signs — a keypair account
  creating itself); a **read-only signer** (a co-signer, e.g. a multisig member)
  enters **funded**. Actors that pay are world state — install them with
  `Wallet`.
- **Byte-identical determinism.** Two fresh worlds running the identical scenario
  produce byte-identical results — the same `Outcome` (error, compute units,
  logs, return data, account changes) and the same post-state bytes. The backend
  seeds a fixed genesis blockhash and a zero-timestamp clock (no wall-clock, no
  RNG); fixture placement follows a per-world deterministic address sequence; and
  every observable ordering is first-appearance, never hash-map iteration. Both
  harness test suites assert this against two worlds.
- **Cross-harness fixture addresses.** The deterministic address sequence is
  identical across the Rust, Kit, and Web3.js harnesses, so a fixture address one
  computes matches the others'.
- **Explicit time control.** `warp_to_timestamp(ts)` sets the clock's Unix
  timestamp alone; `sync_clock` (via a `Dump`) adopts a dumped mainnet slot *and*
  a timestamp derived from it. Nothing advances the clock implicitly.

## See also

- [`docs/typescript_reference.md`](typescript_reference.md) — the same surface
  from Kit and Web3.js.
