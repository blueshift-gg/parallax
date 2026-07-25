import { getMintDecoder, getTokenDecoder } from "@solana-program/token";
import {
  address,
  AccountRole,
  getAddressDecoder,
  getAddressEncoder,
  getProgramDerivedAddress,
  isSignerRole,
  isWritableRole,
  lamports,
  type Account,
  type Address,
  type Instruction,
} from "@solana/kit";
import { readFile } from "node:fs/promises";
import {
  createFixtureFactories,
  DEFAULT_WALLET_LAMPORTS,
  TokenProgram,
  type AccountOptions as SharedAccountOptions,
  type AssociatedTokenAccountOptions,
  type DumpOptions as SharedDumpOptions,
  type Fixture as SharedFixture,
  type FixtureHost,
  type LoadOptions,
  type MintHolder as SharedMintHolder,
  type MintOptions as SharedMintOptions,
  type TokenAccountOptions as SharedTokenAccountOptions,
  type WalletOptions as SharedWalletOptions,
} from "./internal/fixture.js";
import {
  Outcome as SharedOutcome,
  type AccountChange as SharedAccountChange,
  type AccountCodec as SharedAccountCodec,
} from "./internal/outcome.js";
import {
  SPL_ASSOCIATED_TOKEN_PROGRAM_ID,
  SPL_TOKEN_2022_PROGRAM_ID,
  SPL_TOKEN_PROGRAM_ID,
  SYSTEM_PROGRAM_ID,
} from "./internal/programs.js";
import {
  TestCore,
  type HarnessAdapter,
  type TestOptions as SharedTestOptions,
} from "./internal/test.js";

export { DEFAULT_WALLET_LAMPORTS, TokenProgram };
export type { ProgramError } from "./internal/outcome.js";
export type { AssociatedTokenAccountOptions };

type WorldAccount = Account<Uint8Array>;
export type Fixture<Output> = SharedFixture<Output, Test>;
export type Outcome = SharedOutcome<Address, WorldAccount>;
export type AccountChange = SharedAccountChange<Address, WorldAccount>;
export type AccountCodec<Value> = SharedAccountCodec<Value, Address>;
export type WalletOptions = SharedWalletOptions<Address>;
export type MintOptions = SharedMintOptions<Address>;
export type MintHolder = SharedMintHolder<Address>;
export type TokenAccountOptions = SharedTokenAccountOptions<Address>;
export type AccountOptions = SharedAccountOptions<Address>;
export type DumpOptions = SharedDumpOptions<Address>;
export type { LoadOptions };
export type TestOptions = SharedTestOptions;

/** Account metas for read-only co-signers, e.g. multisig signers. */
export function coSigners(
  addresses: readonly Address[],
): { address: Address; role: AccountRole }[] {
  return addresses.map(address => ({
    address,
    role: AccountRole.READONLY_SIGNER,
  }));
}

/** Value-equality for addresses, independent of the backend representation. */
export function addressesEqual(left: Address, right: Address): boolean {
  return left === right;
}

const addressEncoder = getAddressEncoder();
const addressDecoder = getAddressDecoder();
const systemProgram = address(SYSTEM_PROGRAM_ID);

// Kit's `Address` is a base58 string with no stored bytes, so every boundary
// crossing base58-decodes it to 32 bytes (~6us) on the way in and base58-encodes
// results back (~3.5us) on the way out — by far the shell's largest per-send
// cost, and the reason a Kit send runs ~3x a Web3.js send whose `Address` keeps
// its bytes. Both directions are pure functions of their input, a test reuses
// the same handful of addresses across many instructions, and every account
// owner repeats across an outcome, so memoize both. The maps are bounded: past a
// generous ceiling they are cleared wholesale, so a long-lived process cannot
// grow them without limit. Cached byte arrays are treated as immutable — the
// only consumers are wire serializers, which copy them.
const ADDRESS_CACHE_LIMIT = 50_000;
const bytesByAddress = new Map<Address, Uint8Array>();
const addressByBytes = new Map<string, Address>();

function encoded(value: Address): Uint8Array {
  let bytes = bytesByAddress.get(value);
  if (bytes === undefined) {
    if (bytesByAddress.size >= ADDRESS_CACHE_LIMIT) bytesByAddress.clear();
    bytes = new Uint8Array(addressEncoder.encode(value));
    bytesByAddress.set(value, bytes);
  }
  return bytes;
}

function decoded(bytes: Uint8Array): Address {
  const key = Buffer.from(bytes).toString("latin1");
  let value = addressByBytes.get(key);
  if (value === undefined) {
    if (addressByBytes.size >= ADDRESS_CACHE_LIMIT) addressByBytes.clear();
    value = addressDecoder.decode(bytes) as Address;
    addressByBytes.set(key, value);
  }
  return value;
}

const adapter: HarnessAdapter<Address, WorldAccount, Instruction, Outcome> = {
  addressKey: value => value,
  addressToBytes: encoded,
  bytesToAddress: decoded,
  accountAddress: account => account.address,
  accountData: account => account.data,
  accountOwner: account => account.programAddress,
  accountToWire: account => ({
    address: encoded(account.address),
    owner: encoded(account.programAddress),
    lamports: BigInt(account.lamports),
    data: account.data,
    executable: account.executable,
  }),
  buildAccount: account => ({
    address: decoded(account.address),
    programAddress: decoded(account.owner),
    lamports: lamports(account.lamports),
    data: account.data,
    executable: account.executable,
    space: BigInt(account.data.length),
  }),
  instructionToWire: instruction => ({
    programId: encoded(instruction.programAddress),
    data: instruction.data ? Uint8Array.from(instruction.data) : new Uint8Array(),
    accounts: (instruction.accounts ?? []).map(meta => ({
      pubkey: encoded(meta.address),
      signer: isSignerRole(meta.role),
      writable: isWritableRole(meta.role),
    })),
  }),
  tokenAmount: account => BigInt(getTokenDecoder().decode(account.data).amount),
  mintSupply: account => BigInt(getMintDecoder().decode(account.data).supply),
  async deriveAta(owner, mint, tokenProgram) {
    const tokenProgramId = address(
      tokenProgram === TokenProgram.Token2022
        ? SPL_TOKEN_2022_PROGRAM_ID
        : SPL_TOKEN_PROGRAM_ID,
    );
    return (await getProgramDerivedAddress({
      programAddress: address(SPL_ASSOCIATED_TOKEN_PROGRAM_ID),
      seeds: [encoded(owner), encoded(tokenProgramId), encoded(mint)],
    }))[0];
  },
  deriveProgramAddress: (seeds, programAddress) =>
    getProgramDerivedAddress({ programAddress, seeds: [...seeds] }),
  outcome: (raw, accounts, changes) =>
    new SharedOutcome(raw, accounts, adapter, changes),
  isClosed: account =>
    BigInt(account.lamports) === 0n &&
    account.data.length === 0 &&
    account.programAddress === systemProgram,
  lamports: account => BigInt(account.lamports),
  renderAddress: value => value,
};

/** An isolated fixture-first test world using Kit address and account types. */
export class Test extends TestCore<Address, WorldAccount, Instruction, Outcome> {
  constructor(programId?: Address, elf?: Uint8Array, options: TestOptions = {}) {
    super(adapter, programId, elf, options);
  }

  /**
   * Open a world for a program discovered on disk. The artifact is resolved
   * through a discovery chain, mirroring the Rust harness's `setup.rs`: an
   * explicit `programPath` argument wins; otherwise the `PARALLAX_PROGRAM_PATH`
   * environment variable (which a test runner sets to a freshly built
   * artifact); failing both, an actionable error.
   *
   * Contrast with `preloadProgram`, which loads already-in-memory bytes, and
   * the `dump`/`load` program fixtures, which pull from the network or a file.
   */
  static async open(
    programId: Address,
    programPath?: string,
    options?: TestOptions,
  ): Promise<Test> {
    const resolved = programPath ?? process.env.PARALLAX_PROGRAM_PATH;
    if (!resolved) {
      throw new Error(
        "no program artifact: pass a path to Test.open(id, path), or set PARALLAX_PROGRAM_PATH",
      );
    }
    return new Test(programId, await readFile(resolved), options);
  }
}

const fixtures = createFixtureFactories<Address, Test & FixtureHost<Address>>();

export const {
  account,
  associatedTokenAccount,
  dump,
  load,
  mint,
  program,
  tokenAccount,
  wallet,
} = fixtures;
