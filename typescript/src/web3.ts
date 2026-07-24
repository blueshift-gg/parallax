import { getMintDecoder, getTokenDecoder } from "@solana-program/token";
import {
  Address,
  type KeyedAccountInfo,
  type TransactionInstruction,
} from "@solana/web3.js";
import { readFile } from "node:fs/promises";
import {
  createFixtureFactories,
  DEFAULT_WALLET_LAMPORTS,
  TokenProgram,
  type AccountOptions as SharedAccountOptions,
  type AssociatedTokenAccountOptions,
  type DumpOptions as SharedDumpOptions,
  type Fixture as SharedFixture,
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

export type Fixture<Output> = SharedFixture<Output, Test>;
export type Outcome = SharedOutcome<Address, KeyedAccountInfo>;
export type AccountChange = SharedAccountChange<Address, KeyedAccountInfo>;
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
): { pubkey: Address; isSigner: boolean; isWritable: boolean }[] {
  return addresses.map(pubkey => ({
    pubkey,
    isSigner: true,
    isWritable: false,
  }));
}

/** Value-equality for addresses, independent of the backend representation. */
export function addressesEqual(left: Address, right: Address): boolean {
  return left.equals(right);
}

const systemProgram = new Address(SYSTEM_PROGRAM_ID);

const adapter: HarnessAdapter<
  Address,
  KeyedAccountInfo,
  TransactionInstruction,
  Outcome
> = {
  addressKey: value => value.toBase58(),
  addressToBytes: value => new Uint8Array(value.toBytes()),
  bytesToAddress: bytes => new Address(bytes),
  accountAddress: account => account.accountId,
  accountData: account => account.accountInfo.data,
  accountOwner: account => account.accountInfo.owner,
  accountToWire: account => ({
    address: new Uint8Array(account.accountId.toBytes()),
    owner: new Uint8Array(account.accountInfo.owner.toBytes()),
    lamports: account.accountInfo.lamports,
    data: account.accountInfo.data,
    executable: account.accountInfo.executable,
  }),
  buildAccount: account => ({
    accountId: new Address(account.address),
    accountInfo: {
      data: account.data,
      executable: account.executable,
      lamports: account.lamports,
      owner: new Address(account.owner),
      rentEpoch: 0n,
      space: BigInt(account.data.length),
    },
  }),
  instructionToWire: instruction => ({
    programId: new Uint8Array(instruction.programId.toBytes()),
    data: new Uint8Array(instruction.data),
    accounts: instruction.keys.map(meta => ({
      pubkey: new Uint8Array(meta.pubkey.toBytes()),
      signer: meta.isSigner,
      writable: meta.isWritable,
    })),
  }),
  tokenAmount: account =>
    BigInt(getTokenDecoder().decode(account.accountInfo.data).amount),
  mintSupply: account =>
    BigInt(getMintDecoder().decode(account.accountInfo.data).supply),
  async deriveAta(owner, mint, tokenProgram) {
    return (await Address.findProgramAddress(
      [
        owner.toBytes(),
        new Address(
          tokenProgram === TokenProgram.Token2022
            ? SPL_TOKEN_2022_PROGRAM_ID
            : SPL_TOKEN_PROGRAM_ID,
        ).toBytes(),
        mint.toBytes(),
      ],
      new Address(SPL_ASSOCIATED_TOKEN_PROGRAM_ID),
    ))[0];
  },
  deriveProgramAddress: (seeds, programId) =>
    Address.findProgramAddress([...seeds], programId),
  outcome: (raw, accounts, changes) =>
    new SharedOutcome(raw, accounts, adapter, changes),
  isClosed: account =>
    account.accountInfo.lamports === 0n &&
    account.accountInfo.data.length === 0 &&
    account.accountInfo.owner.equals(systemProgram),
  lamports: account => account.accountInfo.lamports,
  renderAddress: value => value.toBase58(),
};

/** An isolated fixture-first test world using Web3.js address and account types. */
export class Test extends TestCore<
  Address,
  KeyedAccountInfo,
  TransactionInstruction,
  Outcome
> {
  constructor(programId?: Address, elf?: Uint8Array, options: TestOptions = {}) {
    super(adapter, programId, elf, options);
  }

  static async load(
    programId: Address,
    programPath = process.env.PARALLAX_PROGRAM_PATH,
    options?: TestOptions,
  ): Promise<Test> {
    if (!programPath) {
      throw new Error(
        "PARALLAX_PROGRAM_PATH is not set; run through your test runner or pass an artifact path",
      );
    }
    return new Test(programId, await readFile(programPath), options);
  }
}

const fixtures = createFixtureFactories<Address, Test>();

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
