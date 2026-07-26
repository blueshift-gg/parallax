# parallax-svm reference (TypeScript)

The full TypeScript surface — Kit and Web3.js. The
[package README](../README.md) is the tour; this is the manual. The shared model
and its guarantees live with the [Rust harness](../../README.md) and in
the [Guarantees](rust_reference.md#guarantees) section of the Rust
reference; the Rust API mirror is in [`docs/rust_reference.md`](rust_reference.md).

Import from `parallax-svm/kit` for Kit's `Address`/`Account` types, or
`parallax-svm/web3.js` for the Web3.js equivalents; the API is otherwise
identical. Fixture addresses are deterministic and match across the Kit, Web3.js,
and Rust harnesses.

## Opening a world

`Test.open(programId, programPath?, options?)` resolves the artifact through a
discovery chain mirroring Rust's `setup.rs`: an explicit `programPath` wins;
otherwise `PARALLAX_PROGRAM_PATH` (the same variable a test runner sets to a
freshly built program); failing both, an actionable error. The constructor
`new Test(programId?, elf?, options?)` takes in-memory ELF directly, and
`new Test()` with no program builds a world with only the runtime built-ins.

`TestOptions` is `{ computeUnitLimit?: bigint; rpc?: string }` — the
per-transaction CU ceiling and the endpoint `dump` fixtures fetch from (code-only,
defaulting to public mainnet-beta; no environment override). `setComputeUnitLimit`
reconfigures the ceiling on an already-built world.

A `Test` owns a native handle. Use `using` for automatic disposal (or call
`free()` / `test[Symbol.dispose]()`), so the kernel is released deterministically.

## Fixtures

`test.add(fixture)` installs a fixture and returns the address(es) it placed; it
is the only composition primitive, and accepts an array of fixtures too. Built-in
factories:

```ts
import {
  wallet, mint, tokenAccount, associatedTokenAccount, account, program,
} from "parallax-svm/kit";

const authority = await test.add(wallet());                         // funded actor
const poor = await test.add(wallet({ fund: 0n }));                  // exact balance
const pinned = await test.add(wallet({ address: SOME_ADDRESS }));   // specific address

const m = await test.add(
  mint({
    authority,                       // omit → fixed-supply
    freezeAuthority: authority,
    supply: 1_000n,
    decimals: 9,
    tokenProgram: TokenProgram.Token2022,
    holders: [[alice, 400n], [bob, 600n]],   // one funded ATA per [owner, amount]
  }),
);

const vault = await test.add(tokenAccount(m, authority, { amount: 600n }));
const ata = await test.add(associatedTokenAccount(m, authority, { amount: 400n }));
const raw = await test.add(account({ address, owner, lamports: 1n, data }));
await test.add(program(CPI_PROGRAM_ID, elf));   // preload ELF for CPI
```

Constructors take only what is conceptually required (`tokenAccount(mint, owner)`,
`account({ address, owner })`); everything else is an option.

### The `accounts` / `count` plurals

Installing several of a fixture is an option-bag plural that returns `Address[]`,
mirroring the Rust `accounts` vocabulary:

```ts
const [a, b] = await test.add(wallet({ accounts: [ADDR_A, ADDR_B], fund: 5_000n })); // pinned
const [w1, w2, w3] = await test.add(wallet({ count: 3, fund: 7n }));                 // fresh
const [m1, m2] = await test.add(mint({ count: 2, supply: 1_000n }));                 // fresh
const [t1, t2] = await test.add(tokenAccount(m, owner, { accounts: [T1, T2] }));     // pinned
```

`wallet` and `tokenAccount` offer both the pinned (`accounts`) and fresh (`count`)
plural; `mint` offers `count`. `associatedTokenAccount` has no plural — an ATA
address is a pure function of owner and mint, so several owners is what
`mint({ holders })` expresses. Application fixtures are plain objects
implementing `Fixture` (an `install(test)` method).

### Dump & load

`dump` copies real mainnet accounts into the world through the committed
`.parallax/` store — warm runs are fully offline and deterministic, and the
network is touched only on a miss (one batched fetch at one slot). See
[`docs/rust_reference.md`](rust_reference.md#dump--load-real-state) for the store
mechanics.

```ts
import { dump, load } from "parallax-svm/kit";

const [pool, oracle] = await test.add(dump({ accounts: [POOL, ORACLE] }));
await test.add(dump.program(AMM_PROGRAM));            // program + programdata, coherently
await test.add(dump({ accounts: [POOL], syncClock: true }));  // adopt the dumped slot's clock
await test.add(dump.refreshAll());                   // re-fetch every stored entry at one slot

const accounts = await test.add(load({ path: "fixtures/pool.dump" })); // Address[]
await test.add(load.program("fixtures/amm.dump"));                     // Address
```

Any `.parallax/` file is a shareable dump artifact `load` reads by path.

## Outcomes

`send` and `simulate`, each with an `…All` (instruction-chain) and a `…With`
(explicit-input) variant, return an `Outcome`:

|            | one              | chain              | + explicit inputs                  |
| ---------- | ---------------- | ------------------ | ---------------------------------- |
| commit     | `send`           | `sendAll`          | `sendWith` / `sendAllWith`         |
| simulate   | `simulate`       | `simulateAll`      | `simulateWith` / `simulateAllWith` |

Assertions throw with actionable messages and chain (`return this`); reads return
plain values or `null`:

The verdict is a method; every other assertion is a **check value** passed to
`check`, mirroring Rust — name the fact, bind its subject, then compare:

```ts
import { Account, Cu, Mint, TokenAccount } from "parallax-svm/kit";

test
  .send(withdraw)
  .succeeds()                                  // or: .fails({ type: "InsufficientFunds" })
  .check([                                     //     .failsWith(6001)  — custom code
    Cu.spent().le(20_000n),
    Account.lamports(recipient).eq(1_000_000n),
    Account.owner(vault).eq(test.programId),
    TokenAccount.amount(userAta).eq(600n),
    Mint.supply(mint).eq(1_000n),
    Account.created(vault),
    Account.state(VaultCodec, vault).eq({ authority, amount: 600n }),
  ]);
```

The namespaces mirror Rust exactly — `Cu.spent()`, `Account.lamports(addr)`,
`Account.owner(addr).eq`, `Account.data(addr).eq`,
`Account.state(codec, addr).eq` (deep equality) / `.with(cb)`,
`Account.created/removed/closed(addr)`, `TokenAccount.amount(addr)` /
`Mint.supply(mint)`, and `ReturnData.eq`; numeric facts take
`eq, le, lt, ge, gt`, and every bound fact pipes its value into a closure
with `.with(..)`. In TypeScript a check is simply a function of the outcome.

### Custom checks and invariants

Any `(outcome) => void` function is a check. `test.invariant(..)` registers one
— built-in or custom — to run after every committed send (never on
simulations), so a protocol invariant is written once and enforced everywhere:

```ts
import { Account, type Check } from "parallax-svm/kit";

const solvent: Check = Account.state(PoolCodec, pool).with(p =>
  assert.ok(p.reserves >= p.obligations),
);

test.invariant(solvent);                       // every send now enforces it
test.send(swap).succeeds().check(outcome => assert.equal(outcome.logs.length, 3));
```

An invariant sees each send's `Outcome`, so the accounts it judges must be part
of that transaction; guard on `outcome.account(..)` returning `null` when an
invariant only sometimes applies.

`ProgramError` is a tagged union: `{ type: "InsufficientFunds" }`,
`{ type: "Custom"; code }`, `{ type: "Runtime"; message }`, and the rest. Assert a
custom code with `failsWith(code)`. `coSigners([...])` builds read-only signer
metas for authorities like multisig members.

## Typed state and the `AccountCodec`

Typed reads and writes go through a structural `AccountCodec`, so a generated
client's codec plugs straight in:

```ts
interface AccountCodec<Value, Address> {
  decode(bytes: Uint8Array): Value;      // on the body (post-discriminator)
  encode?(value: Value): ArrayLike<number>;
  owner?: Address;                        // validated by read/state checks
  discriminator?: Uint8Array;             // stripped before decode, framed by write
  size?: number;                          // minimum raw length
}

const vaultState = test.read(VaultCodec, vault);   // codec first, then address
test.write(VaultCodec, vault, { authority, amount: 1_000n });
```

`read`, `write`, and the `Account.state` checks validate `owner`, `discriminator`, and
`size` against the raw account before decoding, throwing precisely on any
mismatch. This is the deliberate mirror of Rust: **TS codecs carry and validate
`owner`** because generated bundles are self-framing, whereas Rust frames bytes
only and keeps owner an orthogonal `Account::owner` / `Account.owner` check — available
standalone here too.

`deriveAta(owner, mint, tokenProgram?)` (defaulting `tokenProgram` to `"legacy"`),
`derivePda(seeds)`, and `derivePdaWithBump(seeds)` derive addresses;
`preloadProgram(id, elf)` loads in-memory ELF for CPI; `warpToTimestamp(ts)` sets
the clock's Unix timestamp. `setAccount`, `account`, `lamports`, `tokens`, and
`supply` round out the world queries.

## Native library

The shell loads the `parallax-svm-ffi` shared library, resolved in this order:

1. `PARALLAX_SVM_LIB`, if set, as an explicit path to the shared library.
2. The platform package for your OS and architecture
   (`parallax-svm-darwin-arm64`, `parallax-svm-linux-x64-gnu`, and so on),
   installed automatically as an optional dependency.
3. The repo-local build at `../target/release/libparallax_svm_ffi.*`, so
   `cargo build --release -p parallax-svm-ffi` from the repository root enables
   local development against an unpublished kernel.
