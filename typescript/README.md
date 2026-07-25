# parallax-svm

The TypeScript half of [Parallax](https://github.com/blueshift-gg/parallax), a
fixture-based testing harness for Solana programs on LiteSVM. It is the Kit and
Web3.js sibling of the Rust crate, and a thin shell over the same Rust core:
every harness semantic — fixture placement, deterministic addresses, backfill,
the zero-fee model, account-change tracking, dumping, and error mapping — lives
in the `parallax-svm-ffi` native kernel, reached through a small binary wire
format. The shell only converts Kit/Web3.js types to and from that wire, so a
program is exercised from three vantage points (Rust, Kit, Web3.js) that agree by
construction.

The member names, the fixture vocabulary, and the semantic guarantees are the
same as the [Rust harness](https://github.com/blueshift-gg/parallax#readme) —
this document covers the TypeScript surface; the root README covers the shared
model in depth.

```bash
npm install --save-dev parallax-svm @solana/kit
```

```ts
import { Test, wallet } from "parallax-svm/kit";
import { PROGRAM_ADDRESS, VaultClient } from "./client/index.js";

using test = await Test.open(PROGRAM_ADDRESS, "target/deploy/vault.so");
const user = await test.add(wallet({ fund: 1_000_000n }));
const deposit = await new VaultClient().createDepositInstruction({
  user,
  amount: 1_000n,
});

test
  .send(deposit)
  .succeeds()
  .hasLamports(deposit.vaultAddress, 1_000n)
  .cuAtMost(10_000n);
```

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
network is touched only on a miss (one batched fetch at one slot). See the root
README for the store mechanics.

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

```ts
test
  .send(withdraw)
  .succeeds()                                  // or: .fails({ type: "InsufficientFunds" })
  .cuAtMost(20_000n)                           //     .failsWith(6001)  — custom code
  .hasLamports(recipient, 1_000_000n)
  .hasTokens(vault, 600n)
  .hasSupply(mint, 1_000n)
  .hasState(VaultCodec, vault, s => assert.equal(s.amount, 600n))
  .ownedBy(vault, test.programId)
  .isClosed(tempAccount);

const out = test.simulate(instruction);
out.isOk();  out.isErr();  out.error;          // ProgramError | null
out.logs;  out.computeUnits;  out.returnData;  // fields, not methods
out.account(address);  out.accountAs(address, decode);  out.accounts();
out.returnValue(decode);  out.events(decode);
for (const change of out.accountChanges) {     // writable before/after
  change.wasCreated();  change.wasRemoved();  change.before;  change.after;
}
```

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
  owner?: Address;                        // validated by read/hasState
  discriminator?: Uint8Array;             // stripped before decode, framed by write
  size?: number;                          // minimum raw length
}

const vaultState = test.read(VaultCodec, vault);   // codec first, then address
test.write(VaultCodec, vault, { authority, amount: 1_000n });
```

`read`, `write`, and `Outcome.hasState` validate `owner`, `discriminator`, and
`size` against the raw account before decoding, throwing precisely on any
mismatch. This is the deliberate mirror of Rust: **TS codecs carry and validate
`owner`** because generated bundles are self-framing, whereas Rust frames bytes
only and keeps owner an orthogonal `owned_by` / `ownedBy` assertion — available
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

Licensed under either of Apache-2.0 or MIT at your option.
