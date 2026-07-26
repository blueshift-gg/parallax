# Parallax design & guarantees

The guarantees the harness makes and the rules that shape its public surface.
The [README](../README.md) is the tour; [`docs/reference.md`](reference.md) is
the API manual.

## Semantic guarantees

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

## Design rules

The public surface is deliberate, and these rules double as the contribution bar:

- **No conceptually-optional constructor arguments.** `Mint::new()`,
  `TokenAccount::new(mint, owner)` — required data only; everything else is a
  builder method.
- **One way to do each thing.** No parallel APIs for the same outcome; `test.add`
  is the sole composition primitive.
- **A shared `accounts` plural vocabulary.** Installing several of a fixture, and
  dumping several accounts, read the same way across the built-ins.
- **Code-only configuration.** RPC endpoint, CU limit, program path — all set in
  the test. `PARALLAX_PROGRAM_PATH` is artifact *discovery* a runner injects, not
  configuration.
- **Panic vs. `Option`, on purpose.** Presence queries return `Option`;
  assertions panic with actionable, address-naming messages.
- **Actions lead names; reads are nouns.** `send`, `write`, `warp_to_timestamp`
  act; `account`, `logs`, `compute_units` read.
- **`#[must_use]` where consuming is the point** — an `Outcome` you never assert
  on, a `TestBuilder` you never `build`.
- **The public surface is snapshot-gated.** The backend stays private, and the
  TypeScript surface is guarded by a committed snapshot
  (`typescript/scripts/check-public-api.mjs`).

## Parallax and LiteSVM

Parallax is a harness for [LiteSVM](https://github.com/LiteSVM/litesvm): LiteSVM
executes the transactions, Parallax provides the fixtures, the assertions, and
the cross-language layer.
