import fs from "node:fs";
import path from "node:path";
import {
  rentMinimumBalance,
  TokenProgram,
  type Fixture,
  type FixtureHost,
  type MintInstall,
} from "./fixture.js";
import {
  Kernel,
  WireTokenProgram,
  type DumpTarget,
  type WireAccount,
  type WireInstruction,
} from "./kernel.js";
import {
  AccountChange,
  decodeAccount,
  type AccountCodec,
  type OutcomeAdapter,
  type RawExecutionResult,
} from "./outcome.js";

type FixtureValue<Value> = Value extends Fixture<infer Output, infer _Host>
  ? Awaited<Output>
  : never;

type Installed<Input> = Input extends readonly unknown[]
  ? { [Index in keyof Input]: FixtureValue<Input[Index]> }
  : FixtureValue<Input>;

/** Stable runtime limits accepted by both TypeScript test adapters. */
export interface TestOptions {
  /** Maximum compute units available to one transaction. */
  readonly computeUnitLimit?: bigint;
  /**
   * RPC endpoint that `dump` fixtures fetch from on a store miss. Code-only and
   * set once; unset, it defaults to the public mainnet-beta RPC. There is
   * deliberately no environment override, mirroring the Rust `rpc` builder.
   */
  readonly rpc?: string;
}

/** Dump-target role codes; must match the Rust wire (`src/dump.rs`). */
const DUMP_ROLE_ACCOUNT = 0;
const DUMP_ROLE_PROGRAM = 1;

/** Default RPC endpoint when `TestOptions.rpc` is unset. */
const DEFAULT_RPC_URL = "https://api.mainnet-beta.solana.com";

/**
 * The project directory whose committed `.parallax/` store dumps read and
 * write: the nearest ancestor of the working directory that has a
 * `package.json`, mirroring the Rust harness's `Cargo.toml` walk-up.
 */
function resolveProjectDir(): string {
  let dir = process.cwd();
  for (;;) {
    if (fs.existsSync(path.join(dir, "package.json"))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return process.cwd();
    dir = parent;
  }
}

/**
 * Adapter glue between a native address/account/instruction world and the
 * kernel's raw byte wire. Extends the read-only `OutcomeAdapter` an `Outcome`
 * uses with the conversions the kernel boundary needs.
 */
export interface HarnessAdapter<Address, Account, Instruction, Output>
  extends OutcomeAdapter<Address, Account> {
  addressToBytes(address: Address): Uint8Array;
  bytesToAddress(bytes: Uint8Array): Address;
  accountToWire(account: Account): WireAccount;
  buildAccount(account: WireAccount): Account;
  instructionToWire(instruction: Instruction): WireInstruction;
  deriveAta(owner: Address, mint: Address, tokenProgram: TokenProgram): Promise<Address>;
  deriveProgramAddress(
    seeds: readonly Uint8Array[],
    programId: Address,
  ): Promise<readonly [Address, number]>;
  outcome(
    result: RawExecutionResult,
    accounts: readonly Account[],
    changes: readonly AccountChange<Address, Account>[],
  ): Output;
}

function wireTokenProgram(tokenProgram: TokenProgram): WireTokenProgram {
  return tokenProgram === TokenProgram.Token2022
    ? WireTokenProgram.Token2022
    : WireTokenProgram.Legacy;
}

export class TestCore<Address, Account, Instruction, Output> {
  readonly #kernel: Kernel;
  readonly #adapter: HarnessAdapter<Address, Account, Instruction, Output>;
  readonly #primaryProgramId: Address | undefined;
  readonly #rpc: string;
  readonly #projectDir: string;

  protected constructor(
    adapter: HarnessAdapter<Address, Account, Instruction, Output>,
    programId?: Address,
    elf?: Uint8Array,
    options: TestOptions = {},
  ) {
    if ((programId === undefined) !== (elf === undefined)) {
      throw new Error("Test needs both a program address and ELF, or neither");
    }
    this.#adapter = adapter;
    this.#primaryProgramId = programId;
    this.#rpc = options.rpc ?? DEFAULT_RPC_URL;
    this.#projectDir = resolveProjectDir();
    let computeUnitLimit: bigint | null = null;
    if (options.computeUnitLimit !== undefined) {
      if (
        options.computeUnitLimit < 0n ||
        options.computeUnitLimit > 0xffff_ffff_ffff_ffffn
      ) {
        throw new Error("computeUnitLimit must fit a u64");
      }
      computeUnitLimit = options.computeUnitLimit;
    }
    this.#kernel = Kernel.create(
      programId === undefined ? null : adapter.addressToBytes(programId),
      elf ?? null,
      computeUnitLimit,
    );
  }

  get programId(): Address {
    if (this.#primaryProgramId === undefined) {
      throw new Error("this Test has no primary program");
    }
    return this.#primaryProgramId;
  }

  async add<
    const Input extends
      | Fixture<unknown, this & FixtureHost<Address>>
      | readonly Fixture<unknown, this & FixtureHost<Address>>[],
  >(input: Input): Promise<Installed<Input>> {
    // The install plumbing (`installWallet`, `dumpAccounts`, …) is `protected`
    // so it never reaches a consumer, but fixtures — free functions living
    // outside the class — must still call it. The seam bridges the two: a
    // fixture's `install(test)` is typed to `this & FixtureHost<Address>`, the
    // concrete world with the plumbing widened back to public, and `add` (which
    // has protected access) hands it exactly that view of `this`.
    const host = this as this & FixtureHost<Address>;
    if (!Array.isArray(input)) {
      return (await (input as Fixture<unknown, this & FixtureHost<Address>>).install(
        host,
      )) as Installed<Input>;
    }

    const installed: unknown[] = [];
    for (const fixture of input as readonly Fixture<
      unknown,
      this & FixtureHost<Address>
    >[]) {
      installed.push(await fixture.install(host));
    }
    return installed as Installed<Input>;
  }

  // --- Fixture install surface (FixtureHost) --------------------------------
  //
  // These methods are the plumbing fixtures drive; they are `protected` so they
  // stay off the public API surface (see `scripts/check-public-api.mjs`).
  // Fixtures reach them through the `add` seam, which widens `this` to
  // `this & FixtureHost<Address>`. `setAccount` and the exec/read/derive surface
  // below stay public.

  protected installWallet(address: Address | undefined, fund: bigint | undefined): Address {
    return this.#adapter.bytesToAddress(
      this.#kernel.installWallet(
        address === undefined ? null : this.#adapter.addressToBytes(address),
        fund ?? null,
      ),
    );
  }

  protected installMint(options: MintInstall<Address>): Address {
    const [mint] = this.#kernel.installMint({
      authority:
        options.authority === undefined
          ? null
          : this.#adapter.addressToBytes(options.authority),
      freezeAuthority:
        options.freezeAuthority === undefined
          ? null
          : this.#adapter.addressToBytes(options.freezeAuthority),
      supply: options.supply,
      decimals: options.decimals,
      tokenProgram: wireTokenProgram(options.tokenProgram),
      holders: options.holders.map(([owner, amount]) => [
        this.#adapter.addressToBytes(owner),
        amount,
      ]),
    });
    return this.#adapter.bytesToAddress(mint);
  }

  protected installTokenAccount(
    mint: Address,
    owner: Address,
    address: Address | undefined,
    amount: bigint,
    tokenProgram: TokenProgram,
  ): Address {
    return this.#adapter.bytesToAddress(
      this.#kernel.installTokenAccount(
        address === undefined ? null : this.#adapter.addressToBytes(address),
        this.#adapter.addressToBytes(mint),
        this.#adapter.addressToBytes(owner),
        amount,
        wireTokenProgram(tokenProgram),
      ),
    );
  }

  protected installAta(
    mint: Address,
    owner: Address,
    amount: bigint,
    tokenProgram: TokenProgram,
  ): Address {
    return this.#adapter.bytesToAddress(
      this.#kernel.installAta(
        this.#adapter.addressToBytes(mint),
        this.#adapter.addressToBytes(owner),
        amount,
        wireTokenProgram(tokenProgram),
      ),
    );
  }

  protected installRawAccount(
    address: Address,
    owner: Address,
    lamports: bigint | undefined,
    data: Uint8Array,
  ): Address {
    return this.#adapter.bytesToAddress(
      this.#kernel.installRawAccount(
        this.#adapter.addressToBytes(address),
        this.#adapter.addressToBytes(owner),
        lamports ?? rentMinimumBalance(data.length),
        data,
      ),
    );
  }

  /**
   * Preload a program's compiled bytes for cross-program invocations. Contrast
   * with the `dump`/`load` program fixtures, which pull a program from the
   * network or a dump file. Mirrors Rust `Test::preload_program`.
   */
  preloadProgram(programId: Address, elf: Uint8Array): Address {
    this.#kernel.loadProgram(this.#adapter.addressToBytes(programId), elf);
    return programId;
  }

  // --- Dump (mainnet account/program dumping) -------------------------------
  //
  // The core owns the store, coherence, the RPC shape, and installation. The
  // shell owns only the network transport: it POSTs the request body the core's
  // `dumpPlan` returns and hands the response to `dumpCommit`. On a warm store
  // there are no misses, so no fetch happens and the run is fully offline.

  protected async dumpAccounts(
    addresses: readonly Address[],
    syncClock: boolean,
  ): Promise<readonly Address[]> {
    const targets: DumpTarget[] = addresses.map(address => ({
      address: this.#adapter.addressToBytes(address),
      role: DUMP_ROLE_ACCOUNT,
    }));
    await this.#resolveDump(targets, syncClock, false);
    return addresses;
  }

  protected async dumpProgram(programId: Address, syncClock: boolean): Promise<Address> {
    await this.#resolveDump(
      [{ address: this.#adapter.addressToBytes(programId), role: DUMP_ROLE_PROGRAM }],
      syncClock,
      false,
    );
    return programId;
  }

  protected async refreshAll(): Promise<Address[]> {
    const misses = await this.#resolveDump([], false, true);
    return misses.map(miss => this.#adapter.bytesToAddress(miss.address));
  }

  // --- Load (install from an already-dumped file; no store, no network) -----
  //
  // One host method (`preloadProgram(id, elf)` above is the unrelated program
  // preload). The `load` factory maps the returned addresses to its shape.

  protected loadFile(path: string, isProgram: boolean): Address[] {
    return this.#kernel
      .load(path, isProgram)
      .map(bytes => this.#adapter.bytesToAddress(bytes));
  }

  async #resolveDump(
    targets: DumpTarget[],
    syncClock: boolean,
    refresh: boolean,
  ): Promise<DumpTarget[]> {
    const plan = this.#kernel.dumpPlan(this.#projectDir, targets, syncClock, refresh);
    if (plan.misses.length === 0) return [];
    const response = await this.#fetchDump(plan.requestBody);
    this.#kernel.dumpCommit(this.#projectDir, plan.misses, response, syncClock);
    return plan.misses;
  }

  async #fetchDump(requestBody: Uint8Array): Promise<Uint8Array> {
    const fetchImpl = globalThis.fetch;
    if (typeof fetchImpl !== "function") {
      throw new Error(
        "parallax dump: global fetch is unavailable; Node 18+ or a fetch polyfill is required",
      );
    }
    const response = await fetchImpl(this.#rpc, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: requestBody,
    });
    if (!response.ok) {
      throw new Error(
        `parallax dump: RPC request to ${this.#rpc} failed with HTTP ${response.status}`,
      );
    }
    return new Uint8Array(await response.arrayBuffer());
  }

  // --- Account reads --------------------------------------------------------

  setAccount(account: Account): void {
    this.#kernel.installRawAccount(
      this.#adapter.addressToBytes(this.#adapter.accountAddress(account)),
      this.#adapter.addressToBytes(this.#adapter.accountOwner(account)),
      this.#adapter.lamports(account),
      this.#adapter.accountData(account),
    );
  }

  account(address: Address): Account | null {
    const wire = this.#kernel.getAccount(this.#adapter.addressToBytes(address));
    return wire === null ? null : this.#adapter.buildAccount(wire);
  }

  accountAs<Value>(
    address: Address,
    decode: (data: Uint8Array) => Value,
  ): Value | null {
    const account = this.account(address);
    return account === null ? null : decode(this.#adapter.accountData(account));
  }

  /**
   * Read a typed account through its codec. Ownership, discriminator, and size
   * are validated before decoding; a missing account or any mismatch throws
   * with a precise message.
   *
   * The codec's optional `owner` is validated here because generated bundles
   * are self-framing — the deliberate mirror of Rust `Test::read`, which frames
   * bytes only, checks no owner, and pairs with an orthogonal `owned_by`.
   */
  read<Value>(codec: AccountCodec<Value, Address>, address: Address): Value {
    const account = this.account(address);
    if (account === null) {
      throw new Error(`read: no account at ${this.#adapter.renderAddress(address)}`);
    }
    return decodeAccount(codec, address, account, this.#adapter);
  }

  /**
   * Install a rent-exempt account holding an encoded value. The codec must
   * supply `encode` and `owner`; a discriminator, when present, frames the
   * encoded body. Returns the account address.
   */
  write<Value>(
    codec: AccountCodec<Value, Address>,
    address: Address,
    data: Value,
  ): Address {
    if (codec.encode === undefined) {
      throw new Error("write: codec has no encode");
    }
    if (codec.owner === undefined) {
      throw new Error("write: codec has no owner");
    }
    const body = Uint8Array.from(codec.encode(data));
    const discriminator = codec.discriminator ?? new Uint8Array();
    const bytes = new Uint8Array(discriminator.length + body.length);
    bytes.set(discriminator, 0);
    bytes.set(body, discriminator.length);
    return this.installRawAccount(address, codec.owner, undefined, bytes);
  }

  /**
   * Derive an associated-token address. `tokenProgram` defaults to `"legacy"` —
   * an idiomatic TypeScript convenience; Rust `derive_ata` takes the token
   * program explicitly.
   */
  deriveAta(
    owner: Address,
    mint: Address,
    tokenProgram: TokenProgram = "legacy",
  ): Promise<Address> {
    return this.#adapter.deriveAta(owner, mint, tokenProgram);
  }

  async derivePda(seeds: readonly Uint8Array[]): Promise<Address> {
    return (await this.derivePdaWithBump(seeds))[0];
  }

  derivePdaWithBump(
    seeds: readonly Uint8Array[],
  ): Promise<readonly [Address, number]> {
    return this.#adapter.deriveProgramAddress(seeds, this.programId);
  }

  lamports(address: Address): bigint {
    return this.#adapter.lamports(this.#requiredAccount(address));
  }

  tokens(address: Address): bigint {
    return this.#adapter.tokenAmount(this.#requiredAccount(address));
  }

  supply(address: Address): bigint {
    return this.#adapter.mintSupply(this.#requiredAccount(address));
  }

  warpToTimestamp(timestamp: bigint): void {
    if (
      timestamp < -0x8000_0000_0000_0000n ||
      timestamp > 0x7fff_ffff_ffff_ffffn
    ) {
      throw new Error("timestamp must fit an i64");
    }
    this.#kernel.warpToTimestamp(timestamp);
  }

  /**
   * Reconfigure the transaction compute-unit limit on an already-built world,
   * preserving every loaded program and installed account. The constructor
   * option `computeUnitLimit` is the build-time equivalent. Mirrors Rust
   * `Test::set_compute_unit_limit`.
   */
  setComputeUnitLimit(limit: bigint): void {
    if (limit < 0n || limit > 0xffff_ffff_ffff_ffffn) {
      throw new Error("computeUnitLimit must fit a u64");
    }
    this.#kernel.setComputeUnitLimit(limit);
  }

  // --- Execution ------------------------------------------------------------

  send(instruction: Instruction): Output {
    return this.#execute([instruction], [], true);
  }

  sendAll(instructions: readonly Instruction[]): Output {
    return this.#execute([...instructions], [], true);
  }

  sendWith(instruction: Instruction, accounts: readonly Account[]): Output {
    return this.#execute([instruction], [...accounts], true);
  }

  sendAllWith(
    instructions: readonly Instruction[],
    accounts: readonly Account[],
  ): Output {
    return this.#execute([...instructions], [...accounts], true);
  }

  simulate(instruction: Instruction): Output {
    return this.#execute([instruction], [], false);
  }

  simulateWith(instruction: Instruction, accounts: readonly Account[]): Output {
    return this.#execute([instruction], [...accounts], false);
  }

  simulateAll(instructions: readonly Instruction[]): Output {
    return this.#execute([...instructions], [], false);
  }

  simulateAllWith(
    instructions: readonly Instruction[],
    accounts: readonly Account[],
  ): Output {
    return this.#execute([...instructions], [...accounts], false);
  }

  free(): void {
    this.#kernel.free();
  }

  [Symbol.dispose](): void {
    this.free();
  }

  #execute(
    instructions: Instruction[],
    explicitAccounts: Account[],
    commit: boolean,
  ): Output {
    if (instructions.length === 0) {
      throw new Error("a transaction needs an instruction");
    }

    const wireAccounts = explicitAccounts.map(account =>
      this.#adapter.accountToWire(account),
    );
    const seen = new Set<string>();
    for (const account of explicitAccounts) {
      const key = this.#adapter.addressKey(this.#adapter.accountAddress(account));
      if (seen.has(key)) {
        throw new Error(`transaction input contains account ${key} more than once`);
      }
      seen.add(key);
    }

    const wireInstructions = instructions.map(instruction =>
      this.#adapter.instructionToWire(instruction),
    );
    const bundle = commit
      ? this.#kernel.send(wireInstructions, wireAccounts)
      : this.#kernel.simulate(wireInstructions, wireAccounts);

    const result: RawExecutionResult = {
      status:
        bundle.error === null
          ? { ok: true }
          : { ok: false, error: bundle.error },
      computeUnits: bundle.computeUnits,
      logs: bundle.logs,
      returnData: bundle.returnData,
      hint: bundle.hint,
    };
    // Build each post-state account once. A change's `after` is the very same
    // wire account object (the bundle dedupes it), so reuse the built value
    // rather than materializing — and base58-decoding — it a second time.
    const builtByWire = new Map<WireAccount, Account>();
    const accounts = bundle.accounts.map(account => {
      const built = this.#adapter.buildAccount(account);
      builtByWire.set(account, built);
      return built;
    });
    const changes = bundle.changes.map(
      change =>
        new AccountChange(
          this.#adapter.bytesToAddress(change.address),
          change.before === null ? null : this.#adapter.buildAccount(change.before),
          change.after === null
            ? null
            : (builtByWire.get(change.after) ??
              this.#adapter.buildAccount(change.after)),
        ),
    );
    return this.#adapter.outcome(result, accounts, changes);
  }

  #requiredAccount(address: Address): Account {
    const account = this.account(address);
    if (account === null) {
      throw new Error(`no account at ${this.#adapter.renderAddress(address)}`);
    }
    return account;
  }
}
