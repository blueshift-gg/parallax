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

export class TestCore<Address, Account, Instruction, Output>
  implements FixtureHost<Address>
{
  readonly #kernel: Kernel;
  readonly #adapter: HarnessAdapter<Address, Account, Instruction, Output>;
  readonly #primaryProgramId: Address | undefined;

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
      | Fixture<unknown, this>
      | readonly Fixture<unknown, this>[],
  >(input: Input): Promise<Installed<Input>> {
    if (!Array.isArray(input)) {
      return (await (input as Fixture<unknown, this>).install(this)) as Installed<Input>;
    }

    const installed: unknown[] = [];
    for (const fixture of input as readonly Fixture<unknown, this>[]) {
      installed.push(await fixture.install(this));
    }
    return installed as Installed<Input>;
  }

  // --- Fixture install surface (FixtureHost) --------------------------------

  installWallet(address: Address | undefined, fund: bigint | undefined): Address {
    return this.#adapter.bytesToAddress(
      this.#kernel.installWallet(
        address === undefined ? null : this.#adapter.addressToBytes(address),
        fund ?? null,
      ),
    );
  }

  installMint(options: MintInstall<Address>): Address {
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

  installTokenAccount(
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

  installAta(
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

  installRawAccount(
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

  loadProgram(programId: Address, elf: Uint8Array): Address {
    this.#kernel.loadProgram(this.#adapter.addressToBytes(programId), elf);
    return programId;
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

  simulate(instruction: Instruction): Output {
    return this.#execute([instruction], [], false);
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
    };
    const accounts = bundle.accounts.map(account =>
      this.#adapter.buildAccount(account),
    );
    const changes = bundle.changes.map(
      change =>
        new AccountChange(
          this.#adapter.bytesToAddress(change.address),
          change.before === null ? null : this.#adapter.buildAccount(change.before),
          change.after === null ? null : this.#adapter.buildAccount(change.after),
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
