import koffi from "koffi";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { ProgramError } from "./outcome.js";

// ---------------------------------------------------------------------------
// Library discovery
// ---------------------------------------------------------------------------

interface PlatformLibrary {
  /** Platform npm package that ships the prebuilt shared library. */
  readonly pkg: string;
  /** File name of the shared library, shared by the npm package and the
   *  repo-local `target/release` build. */
  readonly lib: string;
}

const PLATFORMS: Readonly<Record<string, PlatformLibrary>> = {
  "darwin-arm64": { pkg: "parallax-svm-darwin-arm64", lib: "libparallax_svm_ffi.dylib" },
  "darwin-x64": { pkg: "parallax-svm-darwin-x64", lib: "libparallax_svm_ffi.dylib" },
  "linux-x64": { pkg: "parallax-svm-linux-x64-gnu", lib: "libparallax_svm_ffi.so" },
  "linux-arm64": { pkg: "parallax-svm-linux-arm64-gnu", lib: "libparallax_svm_ffi.so" },
  "win32-x64": { pkg: "parallax-svm-win32-x64-msvc", lib: "parallax_svm_ffi.dll" },
};

const require = createRequire(import.meta.url);

/**
 * Resolve the native `parallax-svm-ffi` shared library, in priority order:
 *
 * 1. The `PARALLAX_SVM_LIB` environment variable (an explicit path override).
 * 2. The platform-specific npm package (`parallax-svm-<platform>`).
 * 3. The repo-local `../target/release` build, so `cargo build --release -p
 *    parallax-svm-ffi` enables local development without a published package.
 */
function libraryPath(): string {
  const override = process.env.PARALLAX_SVM_LIB;
  if (override) return override;

  const key = `${process.platform}-${process.arch}`;
  const platform = PLATFORMS[key];
  if (!platform) {
    throw new Error(
      `parallax-svm has no prebuilt native library for ${key}; set PARALLAX_SVM_LIB to a shared library path`,
    );
  }

  // Published platform package.
  try {
    const manifest = require.resolve(`${platform.pkg}/package.json`);
    return path.join(path.dirname(manifest), platform.lib);
  } catch {
    // Fall through to the repo-local development build.
  }

  // Repo-local `cargo build --release -p parallax-svm-ffi` artifact. From this
  // module (`<pkg>/{src,dist}/internal`) the workspace root is three levels up.
  const here = path.dirname(fileURLToPath(import.meta.url));
  return path.resolve(here, "../../../target/release", platform.lib);
}

const lib = koffi.load(libraryPath());

// ---------------------------------------------------------------------------
// Extern declarations (see include/parallax_svm.h)
// ---------------------------------------------------------------------------

const parallax_last_error = lib.func("const char *parallax_last_error()");
const parallax_new = lib.func(
  "void *parallax_new(const void *program_id, const void *elf, uint64_t elf_len, bool has_compute_unit_limit, uint64_t compute_unit_limit)",
);
const parallax_free = lib.func("void parallax_free(void *handle)");
const parallax_free_bytes = lib.func("void parallax_free_bytes(void *ptr, uint64_t len)");
const parallax_set_compute_unit_limit = lib.func(
  "int32_t parallax_set_compute_unit_limit(void *handle, uint64_t limit)",
);
const parallax_load_program = lib.func(
  "int32_t parallax_load_program(void *handle, const void *program_id, const void *elf, uint64_t elf_len)",
);
const parallax_warp_to_timestamp = lib.func(
  "int32_t parallax_warp_to_timestamp(void *handle, int64_t timestamp)",
);
const parallax_install_wallet = lib.func(
  "int32_t parallax_install_wallet(void *handle, const void *input, uint64_t input_len, void *address_out)",
);
const parallax_install_token_account = lib.func(
  "int32_t parallax_install_token_account(void *handle, const void *input, uint64_t input_len, void *address_out)",
);
const parallax_install_ata = lib.func(
  "int32_t parallax_install_ata(void *handle, const void *input, uint64_t input_len, void *address_out)",
);
const parallax_install_raw_account = lib.func(
  "int32_t parallax_install_raw_account(void *handle, const void *input, uint64_t input_len, void *address_out)",
);
const parallax_install_mint = lib.func(
  "int32_t parallax_install_mint(void *handle, const void *input, uint64_t input_len, _Out_ void **result_out, _Out_ uint64_t *result_len_out)",
);
const parallax_get_account = lib.func(
  "int32_t parallax_get_account(void *handle, const void *address, _Out_ void **result_out, _Out_ uint64_t *result_len_out)",
);
const parallax_send = lib.func(
  "int32_t parallax_send(void *handle, const void *instructions, uint64_t instructions_len, const void *accounts, uint64_t accounts_len, _Out_ void **result_out, _Out_ uint64_t *result_len_out)",
);
const parallax_simulate = lib.func(
  "int32_t parallax_simulate(void *handle, const void *instructions, uint64_t instructions_len, const void *accounts, uint64_t accounts_len, _Out_ void **result_out, _Out_ uint64_t *result_len_out)",
);

function lastError(): string {
  return parallax_last_error() ?? "unknown error";
}

// ---------------------------------------------------------------------------
// Neutral wire types: raw bytes crossing the boundary. Adapters convert their
// native address/account/instruction types to and from these.
// ---------------------------------------------------------------------------

/** An account reduced to raw bytes. */
export interface WireAccount {
  readonly address: Uint8Array;
  readonly owner: Uint8Array;
  readonly lamports: bigint;
  readonly data: Uint8Array;
  readonly executable: boolean;
}

/** An instruction reduced to raw bytes and account roles. */
export interface WireInstruction {
  readonly programId: Uint8Array;
  readonly data: Uint8Array;
  readonly accounts: readonly {
    readonly pubkey: Uint8Array;
    readonly signer: boolean;
    readonly writable: boolean;
  }[];
}

/** A writable account's before/after state, absent when created/removed. */
export interface WireChange {
  readonly address: Uint8Array;
  readonly before: WireAccount | null;
  readonly after: WireAccount | null;
}

/** The full result of executing a transaction, decoded from the outcome bundle. */
export interface WireOutcome {
  readonly error: ProgramError | null;
  readonly computeUnits: bigint;
  readonly returnData: Uint8Array;
  readonly logs: string[];
  readonly changes: WireChange[];
  readonly accounts: WireAccount[];
}

/** Wire tag for the owning token program (mirrors the Rust `TokenProgram`). */
export const enum WireTokenProgram {
  Legacy = 0,
  Token2022 = 1,
}

/** Decoded option-bag for a mint install. */
export interface MintInstall {
  readonly authority: Uint8Array | null;
  readonly freezeAuthority: Uint8Array | null;
  readonly supply: bigint;
  readonly decimals: number;
  readonly tokenProgram: WireTokenProgram;
  readonly holders: readonly (readonly [owner: Uint8Array, amount: bigint])[];
}

// ---------------------------------------------------------------------------
// Wire reader/writer
// ---------------------------------------------------------------------------

class Writer {
  readonly #chunks: Buffer[] = [];

  bool(value: boolean): void {
    this.#chunks.push(Buffer.from([value ? 1 : 0]));
  }

  u8(value: number): void {
    this.#chunks.push(Buffer.from([value & 0xff]));
  }

  u32(value: number): void {
    const buf = Buffer.allocUnsafe(4);
    buf.writeUInt32LE(value >>> 0);
    this.#chunks.push(buf);
  }

  u64(value: bigint): void {
    const buf = Buffer.allocUnsafe(8);
    buf.writeBigUInt64LE(value);
    this.#chunks.push(buf);
  }

  bytes(value: Uint8Array): void {
    this.#chunks.push(Buffer.from(value));
  }

  lengthPrefixed(value: Uint8Array): void {
    this.u32(value.length);
    this.bytes(value);
  }

  pubkey(value: Uint8Array): void {
    if (value.length !== 32) {
      throw new Error(`expected a 32-byte address, got ${value.length} bytes`);
    }
    this.bytes(value);
  }

  optionPubkey(value: Uint8Array | null): void {
    this.bool(value !== null);
    if (value !== null) this.pubkey(value);
  }

  optionU64(value: bigint | null): void {
    this.bool(value !== null);
    if (value !== null) this.u64(value);
  }

  account(value: WireAccount): void {
    this.pubkey(value.address);
    this.pubkey(value.owner);
    this.u64(value.lamports);
    this.lengthPrefixed(value.data);
    this.bool(value.executable);
  }

  finish(): Buffer {
    return Buffer.concat(this.#chunks);
  }
}

class Reader {
  #pos = 0;

  constructor(private readonly buf: Buffer) {}

  u8(): number {
    return this.buf.readUInt8(this.#pos++);
  }

  bool(): boolean {
    return this.u8() !== 0;
  }

  u32(): number {
    const value = this.buf.readUInt32LE(this.#pos);
    this.#pos += 4;
    return value;
  }

  u64(): bigint {
    const value = this.buf.readBigUInt64LE(this.#pos);
    this.#pos += 8;
    return value;
  }

  take(n: number): Uint8Array {
    const slice = Uint8Array.from(this.buf.subarray(this.#pos, this.#pos + n));
    this.#pos += n;
    return slice;
  }

  pubkey(): Uint8Array {
    return this.take(32);
  }

  lengthPrefixed(): Uint8Array {
    return this.take(this.u32());
  }

  string(): string {
    const len = this.u32();
    const value = this.buf.toString("utf8", this.#pos, this.#pos + len);
    this.#pos += len;
    return value;
  }

  account(): WireAccount {
    const address = this.pubkey();
    const owner = this.pubkey();
    const lamports = this.u64();
    const data = this.lengthPrefixed();
    const executable = this.bool();
    return { address, owner, lamports, data, executable };
  }

  optionAccount(): WireAccount | null {
    return this.bool() ? this.account() : null;
  }
}

// The ProgramError tag table (wire tags 0..17); 18 = Custom, 19 = Runtime are
// handled with their trailing payloads.
const ERROR_TAGS: readonly ProgramError["type"][] = [
  "InvalidArgument",
  "InvalidInstructionData",
  "InvalidAccountData",
  "AccountDataTooSmall",
  "InsufficientFunds",
  "IncorrectProgramId",
  "MissingRequiredSignature",
  "AccountAlreadyInitialized",
  "UninitializedAccount",
  "MissingAccount",
  "InvalidSeeds",
  "ArithmeticOverflow",
  "AccountNotRentExempt",
  "InvalidAccountOwner",
  "IncorrectAuthority",
  "Immutable",
  "BorshIoError",
  "ComputeBudgetExceeded",
];

function errorFromWire(tag: number, customCode: number, message: string): ProgramError {
  if (tag === 18) return { type: "Custom", code: customCode };
  if (tag === 19) return { type: "Runtime", message };
  const named = ERROR_TAGS[tag];
  return named !== undefined
    ? ({ type: named } as ProgramError)
    : { type: "Runtime", message: message || `error tag ${tag}` };
}

// ---------------------------------------------------------------------------
// Input serialization
// ---------------------------------------------------------------------------

function serializeInstructions(instructions: readonly WireInstruction[]): Buffer {
  const w = new Writer();
  w.u32(instructions.length);
  for (const instruction of instructions) {
    w.pubkey(instruction.programId);
    w.lengthPrefixed(instruction.data);
    w.u32(instruction.accounts.length);
    for (const meta of instruction.accounts) {
      w.pubkey(meta.pubkey);
      w.bool(meta.signer);
      w.bool(meta.writable);
    }
  }
  return w.finish();
}

function serializeAccounts(accounts: readonly WireAccount[]): Buffer {
  const w = new Writer();
  w.u32(accounts.length);
  for (const account of accounts) w.account(account);
  return w.finish();
}

// ---------------------------------------------------------------------------
// Output deserialization
// ---------------------------------------------------------------------------

function deserializeOutcome(buf: Buffer): WireOutcome {
  const r = new Reader(buf);
  let error: ProgramError | null = null;
  if (r.bool()) {
    const tag = r.u8();
    const customCode = r.u32();
    const message = r.string();
    error = errorFromWire(tag, customCode, message);
  }
  const computeUnits = r.u64();
  const returnData = r.lengthPrefixed();
  const numLogs = r.u32();
  const logs: string[] = [];
  for (let i = 0; i < numLogs; i += 1) logs.push(r.string());
  const numChanges = r.u32();
  const changes: WireChange[] = [];
  for (let i = 0; i < numChanges; i += 1) {
    changes.push({
      address: r.pubkey(),
      before: r.optionAccount(),
      after: r.optionAccount(),
    });
  }
  const numAccounts = r.u32();
  const accounts: WireAccount[] = [];
  for (let i = 0; i < numAccounts; i += 1) accounts.push(r.account());
  return { error, computeUnits, returnData, logs, changes, accounts };
}

function deserializePubkeys(buf: Buffer): Uint8Array[] {
  const r = new Reader(buf);
  const count = r.u32();
  const pubkeys: Uint8Array[] = [];
  for (let i = 0; i < count; i += 1) pubkeys.push(r.pubkey());
  return pubkeys;
}

function deserializeOptionalAccount(buf: Buffer): WireAccount | null {
  return new Reader(buf).optionAccount();
}

// ---------------------------------------------------------------------------
// Handle wrapper
// ---------------------------------------------------------------------------

const cleanup = new FinalizationRegistry((handle: unknown) => {
  parallax_free(handle);
});

function isNullPointer(handle: unknown): boolean {
  return handle == null || koffi.address(handle) === 0n;
}

/**
 * A thin, typed wrapper over one `parallax-svm-ffi` world handle. Every method
 * serializes its option-bag, crosses the boundary once, and deserializes the
 * result. The Rust core owns all harness semantics; this class only marshals.
 */
export class Kernel {
  #handle: unknown;
  #freed = false;

  private constructor(handle: unknown) {
    this.#handle = handle;
    cleanup.register(this, handle, this);
  }

  /**
   * Build a world. `programId`/`elf` load the primary program; pass a zero
   * address and a null ELF (with `elf.length === 0`) for a program-less world.
   */
  static create(
    programId: Uint8Array | null,
    elf: Uint8Array | null,
    computeUnitLimit: bigint | null,
  ): Kernel {
    const idBuf = Buffer.from(programId ?? new Uint8Array(32));
    const hasElf = elf !== null && elf.length > 0;
    const elfBuf = hasElf ? Buffer.from(elf) : Buffer.alloc(0);
    const hasLimit = computeUnitLimit !== null;
    const handle = parallax_new(
      idBuf,
      elfBuf,
      BigInt(elfBuf.length),
      hasLimit,
      hasLimit ? computeUnitLimit : 0n,
    );
    if (isNullPointer(handle)) {
      throw new Error(`could not build world: ${lastError()}`);
    }
    return new Kernel(handle);
  }

  setComputeUnitLimit(limit: bigint): void {
    this.#check(parallax_set_compute_unit_limit(this.#alive(), limit));
  }

  warpToTimestamp(timestamp: bigint): void {
    this.#check(parallax_warp_to_timestamp(this.#alive(), timestamp));
  }

  loadProgram(programId: Uint8Array, elf: Uint8Array): void {
    this.#check(
      parallax_load_program(
        this.#alive(),
        Buffer.from(programId),
        Buffer.from(elf),
        BigInt(elf.length),
      ),
    );
  }

  installWallet(address: Uint8Array | null, fund: bigint | null): Uint8Array {
    const w = new Writer();
    w.optionPubkey(address);
    w.optionU64(fund);
    return this.#install(parallax_install_wallet, w.finish());
  }

  installTokenAccount(
    address: Uint8Array | null,
    mint: Uint8Array,
    owner: Uint8Array,
    amount: bigint,
    tokenProgram: WireTokenProgram,
  ): Uint8Array {
    const w = new Writer();
    w.optionPubkey(address);
    w.pubkey(mint);
    w.pubkey(owner);
    w.u64(amount);
    w.u8(tokenProgram);
    return this.#install(parallax_install_token_account, w.finish());
  }

  installAta(
    mint: Uint8Array,
    owner: Uint8Array,
    amount: bigint,
    tokenProgram: WireTokenProgram,
  ): Uint8Array {
    const w = new Writer();
    w.pubkey(mint);
    w.pubkey(owner);
    w.u64(amount);
    w.u8(tokenProgram);
    return this.#install(parallax_install_ata, w.finish());
  }

  installRawAccount(
    address: Uint8Array,
    owner: Uint8Array,
    lamports: bigint,
    data: Uint8Array,
  ): Uint8Array {
    const w = new Writer();
    w.pubkey(address);
    w.pubkey(owner);
    w.u64(lamports);
    w.lengthPrefixed(data);
    return this.#install(parallax_install_raw_account, w.finish());
  }

  /** Install a mint plus one ATA per holder. Returns `[mint, ...holderAtas]`. */
  installMint(input: MintInstall): Uint8Array[] {
    const w = new Writer();
    w.optionPubkey(input.authority);
    w.optionPubkey(input.freezeAuthority);
    w.u64(input.supply);
    w.u8(input.decimals);
    w.u8(input.tokenProgram);
    w.u32(input.holders.length);
    for (const [owner, amount] of input.holders) {
      w.pubkey(owner);
      w.u64(amount);
    }
    const bytes = w.finish();
    return deserializePubkeys(
      this.#result(parallax_install_mint, [bytes, BigInt(bytes.length)]),
    );
  }

  getAccount(address: Uint8Array): WireAccount | null {
    return deserializeOptionalAccount(
      this.#result(parallax_get_account, [Buffer.from(address)]),
    );
  }

  send(
    instructions: readonly WireInstruction[],
    accounts: readonly WireAccount[],
  ): WireOutcome {
    return this.#execute(parallax_send, instructions, accounts);
  }

  simulate(
    instructions: readonly WireInstruction[],
    accounts: readonly WireAccount[],
  ): WireOutcome {
    return this.#execute(parallax_simulate, instructions, accounts);
  }

  free(): void {
    if (this.#freed) return;
    this.#freed = true;
    cleanup.unregister(this);
    parallax_free(this.#handle);
    this.#handle = null;
  }

  #execute(
    fn: typeof parallax_send,
    instructions: readonly WireInstruction[],
    accounts: readonly WireAccount[],
  ): WireOutcome {
    const ix = serializeInstructions(instructions);
    const acct = serializeAccounts(accounts);
    return deserializeOutcome(
      this.#result(fn, [ix, BigInt(ix.length), acct, BigInt(acct.length)]),
    );
  }

  #install(fn: typeof parallax_install_wallet, input: Buffer): Uint8Array {
    const out = Buffer.allocUnsafe(32);
    this.#check(fn(this.#alive(), input, BigInt(input.length), out));
    return Uint8Array.from(out);
  }

  #result(fn: (...args: unknown[]) => number, args: unknown[]): Buffer {
    const ptrOut = [null];
    const lenOut = [0n];
    this.#check(fn(this.#alive(), ...args, ptrOut, lenOut));
    const ptr = ptrOut[0];
    const len = Number(lenOut[0]);
    const blob = Buffer.from(koffi.decode(ptr, "uint8_t", len) as number[]);
    parallax_free_bytes(ptr, BigInt(len));
    return blob;
  }

  #alive(): unknown {
    if (this.#freed) throw new Error("this Test world has been freed");
    return this.#handle;
  }

  #check(code: number): void {
    if (code !== 0) {
      throw new Error(`parallax-svm error (${code}): ${lastError()}`);
    }
  }
}
