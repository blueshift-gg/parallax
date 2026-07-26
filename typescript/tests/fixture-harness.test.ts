import { describe, expect, it } from "vitest";
import {
  AccountRole,
  address,
  getAddressDecoder,
  lamports,
  type Account as KitAccount,
  type Address as KitAddress,
  type Instruction,
} from "@solana/kit";
import { Address, TransactionInstruction } from "@solana/web3.js";
import { getTokenDecoder } from "@solana-program/token";
import {
  Account as KAccount,
  Outcome as KOutcome,
  Cu as KCu,
  ReturnData as KReturnData,
  TokenAccount as KTokenAccount,
  DEFAULT_WALLET_LAMPORTS,
  Ctx as KitTest,
  account as kitAccount,
  addressesEqual as kitAddressesEqual,
  associatedTokenAccount as kitAssociatedTokenAccount,
  coSigners as kitCoSigners,
  mint as kitMint,
  wallet as kitWallet,
  type AccountCodec as KitAccountCodec,
  type Fixture as KitFixture,
} from "../src/kit.js";
import {
  Account as WAccount,
  Outcome as WOutcome,
  Cu as WCu,
  ReturnData as WReturnData,
  TokenAccount as WTokenAccount,
  Ctx as Web3Test,
  account as web3Account,
  addressesEqual as web3AddressesEqual,
  associatedTokenAccount as web3AssociatedTokenAccount,
  coSigners as web3CoSigners,
  mint as web3Mint,
  wallet as web3Wallet,
  type AccountCodec as Web3AccountCodec,
  type Fixture as Web3Fixture,
} from "../src/web3.js";

const tokenProgram = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

const COUNTER_DISCRIMINATOR = new Uint8Array([7]);

/** A hand-built codec exercising discriminator/owner/size framing. */
function counterCodec<A>(owner: A) {
  return {
    owner,
    discriminator: COUNTER_DISCRIMINATOR,
    size: COUNTER_DISCRIMINATOR.length + 8,
    decode(bytes: Uint8Array) {
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      return { count: view.getBigUint64(0, true) };
    },
    encode(value: { count: bigint }) {
      const body = new Uint8Array(8);
      new DataView(body.buffer).setBigUint64(0, value.count, true);
      return body;
    },
  };
}

function transferData(amount: bigint): Uint8Array {
  const data = new Uint8Array(9);
  data[0] = 3;
  new DataView(data.buffer).setBigUint64(1, amount, true);
  return data;
}

const systemProgram = "11111111111111111111111111111111";

/** Encode a System program `Transfer` (`SystemInstruction` variant 2). */
function systemTransferData(lamports: bigint): Uint8Array {
  const data = new Uint8Array(12);
  const view = new DataView(data.buffer);
  view.setUint32(0, 2, true);
  view.setBigUint64(4, lamports, true);
  return data;
}

// Deterministic throwaway addresses for tests that need a raw address the world
// never installs (codec owners, wrong-account probes, backfilled signers).
// Distinct from the fixtures' internal `parallax/fresh-address` sequence.
let addressSeed = 0;
function seedBytes(): Uint8Array {
  addressSeed += 1;
  const bytes = new Uint8Array(32);
  new DataView(bytes.buffer).setUint32(0, addressSeed, true);
  return bytes;
}
function kitAddr(): KitAddress {
  return getAddressDecoder().decode(seedBytes()) as KitAddress;
}
function web3Addr(): Address {
  return new Address(seedBytes());
}

describe("fixture-first test harness", () => {
  it("provides the Kit fixture and outcome path", async () => {
    using test = new KitTest();
    const [authority, recipient] = await test.add([
      kitWallet(),
      kitWallet(),
    ] as const);
    const mint = await test.add(kitMint({ authority, supply: 10_000n }));
    const alice = await test.add(
      kitAssociatedTokenAccount(mint, authority, { amount: 5_000n }),
    );
    const bob = await test.add(kitAssociatedTokenAccount(mint, recipient));

    const transfer: Instruction = {
      programAddress: address(tokenProgram),
      accounts: [
        { address: alice, role: AccountRole.WRITABLE },
        { address: bob, role: AccountRole.WRITABLE },
        { address: authority, role: AccountRole.READONLY_SIGNER },
      ],
      data: transferData(1_000n),
    };

    const outcome = test
      .execute(transfer)
      .check(KOutcome.success())
      .check(KTokenAccount.amount(alice, 4_000n))
      .check(KTokenAccount.amount(bob, 1_000n))
      .check(KCu.spent(cu => cu <= 20_000n));
    expect(outcome.accountChanges.map(change => change.address)).toEqual([
      alice,
      bob,
    ]);
    expect(test.supply(mint)).toBe(10_000n);

    test
      .execute({ ...transfer, data: transferData(10_000n) })
      .check(KOutcome.error(1));
    // A failed transaction commits nothing; unchanged balances are world reads.
    expect(test.tokens(alice)).toBe(4_000n);
    expect(test.tokens(bob)).toBe(1_000n);

    test.simulate(transfer).checks([KOutcome.success(), KTokenAccount.amount(bob, 2_000n)]);
    expect(test.tokens(bob)).toBe(1_000n);

    const protocol: KitFixture<readonly [typeof authority, typeof mint]> = {
      install: () => [authority, mint] as const,
    };
    expect(await test.add(protocol)).toEqual([authority, mint]);
  });

  it("provides the same fixture and outcome path for Web3.js", async () => {
    using test = new Web3Test();
    const [authority, recipient] = await test.add([
      web3Wallet(),
      web3Wallet(),
    ] as const);
    const mint = await test.add(web3Mint({ authority, supply: 10_000n }));
    const alice = await test.add(
      web3AssociatedTokenAccount(mint, authority, { amount: 5_000n }),
    );
    const bob = await test.add(web3AssociatedTokenAccount(mint, recipient));

    const transfer = new TransactionInstruction({
      programId: new Address(tokenProgram),
      keys: [
        { pubkey: alice, isSigner: false, isWritable: true },
        { pubkey: bob, isSigner: false, isWritable: true },
        { pubkey: authority, isSigner: true, isWritable: false },
      ],
      data: transferData(1_000n),
    });

    const outcome = test
      .execute(transfer)
      .check(WOutcome.success())
      .check(WTokenAccount.amount(alice, 4_000n))
      .check(WTokenAccount.amount(bob, 1_000n))
      .check(WCu.spent(cu => cu <= 20_000n));
    expect(
      outcome.accountChanges.map(change => change.address.toBase58()),
    ).toEqual([alice.toBase58(), bob.toBase58()]);
    expect(test.supply(mint)).toBe(10_000n);

    test
      .execute(
        new TransactionInstruction({
          programId: new Address(tokenProgram),
          keys: transfer.keys,
          data: transferData(10_000n),
        }),
      )
      .check(WOutcome.error(1));
    expect(test.tokens(alice)).toBe(4_000n);
    expect(test.tokens(bob)).toBe(1_000n);

    test.simulate(transfer).checks([WOutcome.success(), WTokenAccount.amount(bob, 2_000n)]);
    expect(test.tokens(bob)).toBe(1_000n);

    const protocol: Web3Fixture<readonly [Address, Address]> = {
      install: () => [authority, mint] as const,
    };
    expect((await test.add(protocol)).map(value => value.toBase58())).toEqual([
      authority.toBase58(),
      mint.toBase58(),
    ]);
  });

  it("uses the same deterministic fixture addresses in both adapters", async () => {
    using kit = new KitTest();
    using web3 = new Web3Test();
    const kitAddress = await kit.add(kitWallet());
    const web3Address = await web3.add(web3Wallet());
    expect(kitAddress).toBe(web3Address.toBase58());
  });

  it("validates stable runtime limits before entering either backend", () => {
    using zeroKit = new KitTest(undefined, undefined, { computeUnitLimit: 0n });
    using zeroWeb3 = new Web3Test(undefined, undefined, {
      computeUnitLimit: 0n,
    });
    expect(
      () => new KitTest(undefined, undefined, { computeUnitLimit: -1n }),
    ).toThrow("computeUnitLimit must fit a u64");
    expect(
      () =>
        new KitTest(undefined, undefined, {
          computeUnitLimit: 0x1_0000_0000_0000_0000n,
        }),
    ).toThrow("computeUnitLimit must fit a u64");
    expect(() => zeroKit.warpToTimestamp(-0x8000_0000_0000_0001n)).toThrow(
      "timestamp must fit an i64",
    );
    expect(() => zeroWeb3.warpToTimestamp(0x8000_0000_0000_0000n)).toThrow(
      "timestamp must fit an i64",
    );
  });

  // Memory discipline across the native boundary: an explicit free unregisters
  // the finalizer and is idempotent (no double free of the world handle), and
  // any use afterwards is a guarded throw, never a use-after-free into freed
  // native memory.
  it("frees the world safely: double free is a no-op, use-after-free throws", () => {
    const test = new KitTest();
    test.free();
    expect(() => test.free()).not.toThrow();
    expect(() => test.warpToTimestamp(1n)).toThrow(/freed/);
    expect(() => test.account(getAddressDecoder().decode(new Uint8Array(32).fill(1)) as KitAddress)).toThrow(
      /freed/,
    );
  });
});

describe("typed account ergonomics", () => {
  it("reads and writes typed accounts and installs raw accounts (Kit)", async () => {
    using test = new KitTest();
    const owner = kitAddr();
    const codec = counterCodec(owner);
    const counter = test.write(codec, kitAddr(), { count: 42n });

    expect(test.read(codec, counter)).toEqual({ count: 42n });
    expect(kitAddressesEqual(test.account(counter)!.programAddress, owner)).toBe(
      true,
    );
    expect(test.lamports(counter)).toBe(BigInt(9 + 128) * 3480n * 2n);

    expect(() => test.read(counterCodec(kitAddr()), counter)).toThrow(
      /owned by/,
    );
    expect(() => test.read(codec, kitAddr())).toThrow(/no account/);

    const wrongDisc = await test.add(
      kitAccount({
        address: kitAddr(),
        owner,
        data: new Uint8Array([9, 0, 0, 0, 0, 0, 0, 0, 0]),
      }),
    );
    expect(() => test.read(codec, wrongDisc)).toThrow(/discriminator/);

    const tooSmall = await test.add(
      kitAccount({ address: kitAddr(), owner, data: new Uint8Array([7, 0, 0]) }),
    );
    expect(() => test.read(codec, tooSmall)).toThrow(/at least/);
  });

  it("reads and writes typed accounts and installs raw accounts (Web3.js)", async () => {
    using test = new Web3Test();
    const owner = web3Addr();
    const codec = counterCodec(owner);
    const counter = test.write(codec, web3Addr(), { count: 42n });

    expect(test.read(codec, counter)).toEqual({ count: 42n });
    expect(
      web3AddressesEqual(test.account(counter)!.accountInfo.owner, owner),
    ).toBe(true);
    expect(test.lamports(counter)).toBe(BigInt(9 + 128) * 3480n * 2n);

    expect(() => test.read(counterCodec(web3Addr()), counter)).toThrow(
      /owned by/,
    );
    expect(() => test.read(codec, web3Addr())).toThrow(/no account/);

    const wrongDisc = await test.add(
      web3Account({
        address: web3Addr(),
        owner,
        data: new Uint8Array([9, 0, 0, 0, 0, 0, 0, 0, 0]),
      }),
    );
    expect(() => test.read(codec, wrongDisc)).toThrow(/discriminator/);
  });

  it("asserts decoded account state via State checks and read (Kit)", async () => {
    using test = new KitTest();
    const [authority, recipient] = await test.add([
      kitWallet(),
      kitWallet(),
    ] as const);
    const mint = await test.add(
      kitMint({
        authority,
        supply: 10_000n,
        holders: [[authority, 5_000n]],
      }),
    );
    const alice = await test.deriveAta(authority, mint);
    const bob = await test.add(kitAssociatedTokenAccount(mint, recipient));

    const transfer: Instruction = {
      programAddress: address(tokenProgram),
      accounts: [
        { address: alice, role: AccountRole.WRITABLE },
        { address: bob, role: AccountRole.WRITABLE },
        { address: authority, role: AccountRole.READONLY_SIGNER },
      ],
      data: transferData(1_000n),
    };

    const tokenCodec = {
      owner: address(tokenProgram),
      decode: (bytes: Uint8Array) => getTokenDecoder().decode(bytes),
    } satisfies KitAccountCodec<{ amount: bigint }>;

    const outcome = test.execute(transfer).checks([KOutcome.success(), 
      KAccount.owner(alice, address(tokenProgram)),
      KAccount.data(tokenCodec, alice, state => BigInt(state.amount) === 4_000n),
      KAccount.data(tokenCodec, bob, state => BigInt(state.amount) === 1_000n),
    ]);

    // The owner fact mirrors Rust's orthogonal check: it judges owner alone.
    expect(() => outcome.check(KAccount.owner(alice, kitAddr()))).toThrow(/owner of/);

    expect(BigInt(test.read(tokenCodec, alice).amount)).toBe(4_000n);
    expect(() =>
      test.read(
        {
          owner: kitAddr(),
          decode: (bytes: Uint8Array) => getTokenDecoder().decode(bytes),
        },
        alice,
      ),
    ).toThrow(/owned by/);
  });

  it("asserts decoded account state via State checks and read (Web3.js)", async () => {
    using test = new Web3Test();
    const [authority, recipient] = await test.add([
      web3Wallet(),
      web3Wallet(),
    ] as const);
    const mint = await test.add(
      web3Mint({
        authority,
        supply: 10_000n,
        holders: [[authority, 5_000n]],
      }),
    );
    const alice = await test.deriveAta(authority, mint);
    const bob = await test.add(web3AssociatedTokenAccount(mint, recipient));

    const transfer = new TransactionInstruction({
      programId: new Address(tokenProgram),
      keys: [
        { pubkey: alice, isSigner: false, isWritable: true },
        { pubkey: bob, isSigner: false, isWritable: true },
        { pubkey: authority, isSigner: true, isWritable: false },
      ],
      data: transferData(1_000n),
    });

    const tokenCodec = {
      owner: new Address(tokenProgram),
      decode: (bytes: Uint8Array) => getTokenDecoder().decode(bytes),
    } satisfies Web3AccountCodec<{ amount: bigint }>;

    test
      .execute(transfer)
      .checks([
        WOutcome.success(),
        WAccount.data(tokenCodec, alice, state => BigInt(state.amount) === 4_000n),
        WAccount.data(tokenCodec, bob, state => BigInt(state.amount) === 1_000n),
      ]);

    expect(BigInt(test.read(tokenCodec, alice).amount)).toBe(4_000n);
  });

  it("funds mint holders with associated token accounts", async () => {
    using kit = new KitTest();
    const [kitAuthority, kitAlice, kitBob] = await kit.add([
      kitWallet(),
      kitWallet(),
      kitWallet(),
    ] as const);
    const kitMintAddress = await kit.add(
      kitMint({
        authority: kitAuthority,
        supply: 9_000n,
        holders: [[kitAlice, 5_000n], [kitBob, 0n]],
      }),
    );
    expect(kit.tokens(await kit.deriveAta(kitAlice, kitMintAddress))).toBe(
      5_000n,
    );
    expect(kit.tokens(await kit.deriveAta(kitBob, kitMintAddress))).toBe(0n);
    expect(kit.supply(kitMintAddress)).toBe(9_000n);

    using web3 = new Web3Test();
    const [web3Authority, web3Alice] = await web3.add([
      web3Wallet(),
      web3Wallet(),
    ] as const);
    const web3MintAddress = await web3.add(
      web3Mint({
        authority: web3Authority,
        holders: [[web3Alice, 7_000n]],
      }),
    );
    expect(web3.tokens(await web3.deriveAta(web3Alice, web3MintAddress))).toBe(
      7_000n,
    );
  });

  // The kernel owns ATA derivation; `deriveAta` is the one derivation the shell
  // still performs (for naming). Cross-check it against the address the kernel
  // returns from an ATA install, for both token programs.
  it("derives the same ATA address the kernel installs", async () => {
    using kit = new KitTest();
    const [owner] = await kit.add([kitWallet()] as const);
    for (const tokenProgram of ["legacy", "token2022"] as const) {
      const mintAddress = await kit.add(kitMint({ authority: owner, tokenProgram }));
      const installed = await kit.add(
        kitAssociatedTokenAccount(mintAddress, owner, { amount: 1n, tokenProgram }),
      );
      expect(await kit.deriveAta(owner, mintAddress, tokenProgram)).toBe(installed);
    }

    using web3 = new Web3Test();
    const [web3Owner] = await web3.add([web3Wallet()] as const);
    for (const tokenProgram of ["legacy", "token2022"] as const) {
      const mintAddress = await web3.add(
        web3Mint({ authority: web3Owner, tokenProgram }),
      );
      const installed = await web3.add(
        web3AssociatedTokenAccount(mintAddress, web3Owner, {
          amount: 1n,
          tokenProgram,
        }),
      );
      expect((await web3.deriveAta(web3Owner, mintAddress, tokenProgram)).toBase58()).toBe(
        installed.toBase58(),
      );
    }
  });

  it("builds co-signer metas and auto-registers missing signers (Kit)", async () => {
    using test = new KitTest();
    const [authority, recipient] = await test.add([
      kitWallet(),
      kitWallet(),
    ] as const);
    const mint = await test.add(
      kitMint({
        authority,
        supply: 2_000n,
        holders: [[authority, 2_000n]],
      }),
    );
    const alice = await test.deriveAta(authority, mint);
    const bob = await test.add(kitAssociatedTokenAccount(mint, recipient));

    const extra = kitAddr();
    const cosigners = kitCoSigners([extra]);
    expect(cosigners).toEqual([
      { address: extra, role: AccountRole.READONLY_SIGNER },
    ]);
    expect(test.account(extra)).toBeNull();

    const transfer: Instruction = {
      programAddress: address(tokenProgram),
      accounts: [
        { address: alice, role: AccountRole.WRITABLE },
        { address: bob, role: AccountRole.WRITABLE },
        { address: authority, role: AccountRole.READONLY_SIGNER },
        ...cosigners,
      ],
      data: transferData(500n),
    };

    test.execute(transfer).checks([KOutcome.success(), KTokenAccount.amount(bob, 500n)]);
  });

  it("builds co-signer metas and auto-registers missing signers (Web3.js)", async () => {
    using test = new Web3Test();
    const [authority, recipient] = await test.add([
      web3Wallet(),
      web3Wallet(),
    ] as const);
    const mint = await test.add(
      web3Mint({
        authority,
        supply: 2_000n,
        holders: [[authority, 2_000n]],
      }),
    );
    const alice = await test.deriveAta(authority, mint);
    const bob = await test.add(web3AssociatedTokenAccount(mint, recipient));

    const extra = web3Addr();
    const cosigners = web3CoSigners([extra]);
    expect(cosigners).toEqual([
      { pubkey: extra, isSigner: true, isWritable: false },
    ]);

    const transfer = new TransactionInstruction({
      programId: new Address(tokenProgram),
      keys: [
        { pubkey: alice, isSigner: false, isWritable: true },
        { pubkey: bob, isSigner: false, isWritable: true },
        { pubkey: authority, isSigner: true, isWritable: false },
        ...cosigners,
      ],
      data: transferData(500n),
    });

    test.execute(transfer).checks([WOutcome.success(), WTokenAccount.amount(bob, 500n)]);
  });
});

describe("writable-first account backfill", () => {
  const amount = 1_000_000_000n;

  // A missing writable account is an init target and enters empty — even when
  // it signs, as a keypair account being created does. Payers are world state:
  // a payer the test never installs has nothing to move, so the transfer must
  // fail. Installed as a wallet, the same transfer goes through.
  it("treats a missing writable signer as an empty init target, not a funded payer (Kit)", async () => {
    using test = new KitTest();
    const payer = kitAddr();
    const recipient = kitAddr();

    const transfer = (): Instruction => ({
      programAddress: address(systemProgram),
      accounts: [
        { address: payer, role: AccountRole.WRITABLE_SIGNER },
        { address: recipient, role: AccountRole.WRITABLE },
      ],
      data: systemTransferData(amount),
    });

    expect(test.simulate(transfer()).isErr()).toBe(true);

    await test.add(kitWallet({ address: payer }));
    test.execute(transfer()).checks([KOutcome.success(), KAccount.lamports(recipient, amount)]);
  });

  it("treats a missing writable signer as an empty init target, not a funded payer (Web3.js)", async () => {
    using test = new Web3Test();
    const payer = web3Addr();
    const recipient = web3Addr();

    const transfer = () =>
      new TransactionInstruction({
        programId: new Address(systemProgram),
        keys: [
          { pubkey: payer, isSigner: true, isWritable: true },
          { pubkey: recipient, isSigner: false, isWritable: true },
        ],
        data: systemTransferData(amount),
      });

    expect(test.simulate(transfer()).isErr()).toBe(true);

    await test.add(web3Wallet({ address: payer }));
    test.execute(transfer()).checks([WOutcome.success(), WAccount.lamports(recipient, amount)]);
  });

  // A read-only signer (a co-signer, e.g. a multisig member) is an actor and
  // enters funded, even though the world never installed it.
  it("backfills a read-only co-signer as a funded account (Kit)", async () => {
    using test = new KitTest();
    const payer = await test.add(kitWallet());
    const recipient = kitAddr();
    const cosigner = kitAddr();

    const transfer: Instruction = {
      programAddress: address(systemProgram),
      accounts: [
        { address: payer, role: AccountRole.WRITABLE_SIGNER },
        { address: recipient, role: AccountRole.WRITABLE },
        { address: cosigner, role: AccountRole.READONLY_SIGNER },
      ],
      data: systemTransferData(amount),
    };

    test
      .execute(transfer)
      .check(KOutcome.success())
      .check(KAccount.lamports(recipient, amount))
      .check(KAccount.lamports(cosigner, DEFAULT_WALLET_LAMPORTS));
  });

  it("backfills a read-only co-signer as a funded account (Web3.js)", async () => {
    using test = new Web3Test();
    const payer = await test.add(web3Wallet());
    const recipient = web3Addr();
    const cosigner = web3Addr();

    const transfer = new TransactionInstruction({
      programId: new Address(systemProgram),
      keys: [
        { pubkey: payer, isSigner: true, isWritable: true },
        { pubkey: recipient, isSigner: false, isWritable: true },
        { pubkey: cosigner, isSigner: true, isWritable: false },
      ],
      data: systemTransferData(amount),
    });

    test
      .execute(transfer)
      .check(WOutcome.success())
      .check(WAccount.lamports(recipient, amount))
      .check(WAccount.lamports(cosigner, DEFAULT_WALLET_LAMPORTS));
  });
});

// The execution matrix is {send, simulate} x {one, all} x {plain, with}.
// `send`/`sendAll`/`sendWith`/`simulate` are covered above; these exercise the
// completions — `sendAllWith`, `simulateWith`, `simulateAll`, `simulateAllWith`
// — one happy assertion each, mirroring the Rust matrix.
describe("execution matrix completions", () => {
  const amount = 1_000_000n;
  const transfer = (from: KitAddress, to: KitAddress): Instruction => ({
    programAddress: address(systemProgram),
    accounts: [
      { address: from, role: AccountRole.WRITABLE_SIGNER },
      { address: to, role: AccountRole.WRITABLE },
    ],
    data: systemTransferData(amount),
  });
  const fundedSystemAccount = (addr: KitAddress): KitAccount<Uint8Array> => ({
    address: addr,
    programAddress: address(systemProgram),
    lamports: lamports(DEFAULT_WALLET_LAMPORTS),
    data: new Uint8Array(),
    executable: false,
    space: 0n,
  });

  it("simulateAll runs a chain without committing", async () => {
    using test = new KitTest();
    const payer = await test.add(kitWallet());
    const first = kitAddr();
    const second = kitAddr();
    test
      .simulate([transfer(payer, first), transfer(payer, second)])
      .check(KOutcome.success());
    expect(test.account(first)).toBeNull();
  });

  it("sendAllWith seeds explicit inputs for a committed chain", async () => {
    using test = new KitTest();
    const payer = kitAddr(); // never installed
    const recipient = kitAddr();
    test
      .executeWith([transfer(payer, recipient)], [fundedSystemAccount(payer)])
      .check(KOutcome.success())
      .check(KAccount.lamports(recipient, amount));
  });

  it("simulateWith seeds explicit inputs without committing", async () => {
    using test = new KitTest();
    const payer = kitAddr(); // never installed
    const recipient = kitAddr();
    expect(test.simulate(transfer(payer, recipient)).isErr()).toBe(true);
    test
      .simulateWith(transfer(payer, recipient), [fundedSystemAccount(payer)])
      .check(KOutcome.success());
    expect(test.account(payer)).toBeNull();
  });

  it("simulateAllWith seeds explicit inputs for a simulated chain", async () => {
    using test = new KitTest();
    const payer = kitAddr(); // never installed
    const recipient = kitAddr();
    test
      .simulateWith([transfer(payer, recipient)], [fundedSystemAccount(payer)])
      .check(KOutcome.success());
    expect(test.account(payer)).toBeNull();
  });

  it("reconfigures the compute-unit limit on a built world", async () => {
    using test = new KitTest();
    const payer = await test.add(kitWallet());
    const recipient = kitAddr();
    test.setComputeUnitLimit(1_000_000n); // ample headroom for a transfer
    test.execute(transfer(payer, recipient)).check(KOutcome.success());
    expect(() => test.setComputeUnitLimit(-1n)).toThrow(/u64/);
  });

  it("Outcome.accounts returns the full post-state set", async () => {
    using test = new KitTest();
    const payer = await test.add(kitWallet());
    const recipient = kitAddr();
    const outcome = test.execute(transfer(payer, recipient)).check(KOutcome.success());
    const addresses = outcome.accounts().map(account => account.address);
    expect(addresses).toContain(payer);
    expect(addresses).toContain(recipient);
  });
});

// Determinism is a product property: two fresh worlds running the identical
// scenario, through the shell, must produce deep-equal results — the outcome
// surfaces (error, compute units, logs, return data, changes) and every
// post-state account. The mirror of the Rust `two_worlds_produce_byte_identical`
// test, exercised end to end through the native boundary.
describe("determinism", () => {
  type KitOutcome = ReturnType<InstanceType<typeof KitTest>["send"]>;

  const systemTransfer = (
    from: KitAddress,
    to: KitAddress,
    lamports: bigint,
  ): Instruction => ({
    programAddress: address(systemProgram),
    accounts: [
      { address: from, role: AccountRole.WRITABLE_SIGNER },
      { address: to, role: AccountRole.WRITABLE },
    ],
    data: systemTransferData(lamports),
  });

  const fixedAddress = (fill: number): KitAddress =>
    getAddressDecoder().decode(new Uint8Array(32).fill(fill)) as KitAddress;

  const outcomeSnapshot = (outcome: KitOutcome) => ({
    failure: outcome.failure,
    computeUnits: outcome.computeUnits,
    logs: outcome.logs,
    returnData: outcome.returnData,
    changes: outcome.accountChanges.map(change => ({
      address: change.address,
      before: change.before,
      after: change.after,
    })),
  });

  async function runScenario() {
    using test = new KitTest();
    const [payer, alice, bob] = await test.add([
      kitWallet(),
      kitWallet(),
      kitWallet(),
    ] as const);
    const mint = await test.add(
      kitMint({
        authority: payer,
        supply: 1_000n,
        holders: [[alice, 400n], [bob, 600n]],
      }),
    );
    const recipient = fixedAddress(9);
    const ghost = fixedAddress(8);

    // A successful send, then a failed one: the uninstalled writable signer
    // enters as an empty init target with no lamports, so its transfer fails.
    const ok = outcomeSnapshot(
      test.execute(systemTransfer(payer, recipient, 1_000_000n)),
    );
    const fail = outcomeSnapshot(
      test.execute(systemTransfer(ghost, recipient, 1_000_000n)),
    );

    const aliceAta = await test.deriveAta(alice, mint);
    const bobAta = await test.deriveAta(bob, mint);
    const accounts = [payer, alice, bob, mint, aliceAta, bobAta, recipient].map(
      addr => test.account(addr),
    );

    return { ok, fail, accounts };
  }

  it("produces identical results across two fresh worlds", async () => {
    const first = await runScenario();
    const second = await runScenario();

    expect(second).toEqual(first);

    // Guard against a degenerate all-empty "determinism": the scenario really
    // did a successful send that changed accounts and a send that failed.
    expect(first.ok.failure).toBeNull();
    expect(first.ok.changes.length).toBeGreaterThan(0);
    expect(first.fail.failure).not.toBeNull();
  });
});

describe("checks and invariants", () => {
  const amount = 1_000_000_000n;

  // A check is a value: a built-in fact, a closure, or an array of checks runs
  // through `check`; `invariant` registers one for every committed send, so
  // the second over-cap transfer fails with no assertion at the call site.
  // Simulations commit nothing and are never judged.
  it("runs value checks and enforces registered invariants (Kit)", async () => {
    using test = new KitTest();
    const payer = await test.add(kitWallet());
    const recipient = kitAddr();
    test.invariant(outcome => {
      const account = outcome.account(recipient);
      if (account !== null && account.lamports > 1_500_000_000n) {
        throw new Error(`cap exceeded: ${account.lamports} lamports`);
      }
    });

    const transfer = (): Instruction => ({
      programAddress: address(systemProgram),
      accounts: [
        { address: payer, role: AccountRole.WRITABLE_SIGNER },
        { address: recipient, role: AccountRole.WRITABLE },
      ],
      data: systemTransferData(amount),
    });

    test
      .execute(transfer())
      .check(KOutcome.success())
      .check(KCu.spent(cu => cu <= 10_000))
      .check(outcome => expect(outcome.isOk()).toBe(true))
      .checks([KCu.spent(cu => cu <= 10_000), KReturnData.is([]), KAccount.data(recipient, []), KAccount.created(recipient)]);

    expect(() => test.execute(transfer())).toThrow("cap exceeded");
    test.simulate(transfer()).check(KOutcome.success());
  });

  it("runs value checks and enforces registered invariants (Web3.js)", async () => {
    using test = new Web3Test();
    const payer = await test.add(web3Wallet());
    const recipient = web3Addr();
    test.invariant(outcome => {
      const account = outcome.account(recipient);
      if (account !== null && account.accountInfo.lamports > 1_500_000_000n) {
        throw new Error(`cap exceeded: ${account.accountInfo.lamports} lamports`);
      }
    });

    const transfer = () =>
      new TransactionInstruction({
        programId: new Address(systemProgram),
        keys: [
          { pubkey: payer, isSigner: true, isWritable: true },
          { pubkey: recipient, isSigner: false, isWritable: true },
        ],
        data: systemTransferData(amount),
      });

    test.execute(transfer()).checks([WOutcome.success(), WCu.spent(cu => cu <= 10_000), WReturnData.is([])]);
    expect(() => test.execute(transfer())).toThrow("cap exceeded");
    test.simulate(transfer()).check(WOutcome.success());
  });
});
