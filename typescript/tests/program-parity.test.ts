import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  getAddressDecoder,
  lamports,
  type Address as KitAddress,
} from "@solana/kit";
import { Address as Web3Address } from "@solana/web3.js";
import {
  CuBudget as KCu,
  Lamports as KLamports,
  Test as KitTest,
  wallet as kitWallet,
} from "../src/kit.js";
import {
  CuBudget as WCu,
  Lamports as WLamports,
  Test as Web3Test,
  wallet as web3Wallet,
} from "../src/web3.js";
import {
  PROGRAM_ADDRESS,
  PROGRAM_ERROR_CODES,
  QuasarVaultClient as KitVaultClient,
} from "./fixtures/vault/clients/kit/quasar-vault/client.js";
import {
  PROGRAM_ERROR_CODES as WEB3_PROGRAM_ERROR_CODES,
  QuasarVaultClient as Web3VaultClient,
} from "./fixtures/vault/clients/web3/quasar-vault/client.js";

// These suites need a compiled program artifact and its generated client, so
// they are skipped unless PARALLAX_PROGRAM_PATH points at a built `.so` — a
// vault program matching the generated client fixture under `tests/fixtures`.
// `npm run test:program` is a no-op until that env var is set, exactly as the
// Rust harness gates its program-parity tests on the same variable.
const programPath = process.env.PARALLAX_PROGRAM_PATH;
const elfPath = programPath
  ? fileURLToPath(new URL(programPath, `file://${process.cwd()}/`))
  : "";
const userBytes = new Uint8Array(32).fill(1);
const startingLamports = 10_000_000_000n;
const depositAmount = 1_000_000_000n;
const withdrawalAmount = 400_000_000n;

describe.skipIf(!programPath)("vault program parity", () => {
  it("runs the Rust contract through the Kit adapter", async () => {
    const client = new KitVaultClient();
    const user = getAddressDecoder().decode(userBytes) as KitAddress;
    using test = await KitTest.open(PROGRAM_ADDRESS, elfPath, {
      computeUnitLimit: 10_000n,
    });
    await test.add(kitWallet({ address: user }));

    // The builder surfaces the PDA it derives, so the test names the vault off
    // the instruction rather than re-deriving it through findVaultAddress.
    const depositInstruction = await client.createDepositInstruction({
      user,
      amount: depositAmount,
    });
    const vault = depositInstruction.vaultAddress;

    const deposit = test
      .send(depositInstruction)
      .succeeds()
      .check(KCu.le(1_556n))
      .check(KLamports.eq(vault, depositAmount))
      .check(KLamports.eq(user, startingLamports - depositAmount));
    expect(deposit.accountChanges.map(change => change.address)).toEqual([
      user,
      vault,
    ]);
    expect(deposit.accountChanges[1]?.before).toBeNull();
    expect(deposit.accountChanges[1]?.wasCreated()).toBe(true);
    expect(deposit.accountChanges[1]?.wasRemoved()).toBe(false);
    expect(deposit.accountChanges[0]?.wasCreated()).toBe(false);
    expect(deposit.accountChanges[0]?.wasRemoved()).toBe(false);

    // Deposit leaves the vault system-owned; withdraw's CPI spends from it
    // with the vault's PDA seeds signing.
    test.setAccount({
      address: vault,
      data: new Uint8Array(),
      executable: false,
      lamports: lamports(depositAmount),
      programAddress: "11111111111111111111111111111111" as typeof PROGRAM_ADDRESS,
      space: 0n,
    });
    test
      .simulate(
        await client.createWithdrawInstruction({
          user,
          amount: withdrawalAmount,
        }),
      )
      .succeeds()
      .check(KCu.le(1_600n))
      .check(KLamports.eq(vault, depositAmount - withdrawalAmount))
      .check(
        KLamports.eq(user, startingLamports - depositAmount + withdrawalAmount),
      );
    expect(test.lamports(vault)).toBe(depositAmount);
    expect(test.lamports(user)).toBe(startingLamports - depositAmount);

    const wrongVault = getAddressDecoder().decode(
      new Uint8Array(32).fill(9),
    ) as KitAddress;
    const rejected = test
      .send(
        await client.createDepositInstructionRaw(
          { user, amount: 1n },
          { vault: wrongVault },
        ),
      )
      .failsWith(PROGRAM_ERROR_CODES.InvalidPda);
    expect(rejected.account(wrongVault)).toBeNull();
    expect(rejected.accountChanges).toEqual([]);
    expect(test.account(wrongVault)).toBeNull();

    test
      .send(
        await client.createWithdrawInstruction({
          user,
          amount: depositAmount + 1n,
        }),
      )
      .fails({ type: "InsufficientFunds" })
      .check(KLamports.eq(vault, depositAmount));
    test.warpToTimestamp(42n);

    using limited = await KitTest.open(PROGRAM_ADDRESS, elfPath, {
      computeUnitLimit: 1n,
    });
    await limited.add(kitWallet({ address: user }));
    limited
      .send(await client.createDepositInstruction({ user, amount: 1n }))
      .fails({ type: "Runtime", message: "ProgramFailedToComplete" });
  });

  it("runs the same contract through the Web3.js adapter", async () => {
    const client = new Web3VaultClient();
    const user = new Web3Address(userBytes);
    using test = await Web3Test.open(Web3VaultClient.programId, elfPath, {
      computeUnitLimit: 10_000n,
    });
    await test.add(web3Wallet({ address: user }));

    // The builder surfaces the PDA it derives, so the test names the vault off
    // the instruction rather than re-deriving it through findVaultAddress.
    const depositInstruction = await client.createDepositInstruction({
      user,
      amount: depositAmount,
    });
    const vault = depositInstruction.vaultAddress;

    const deposit = test
      .send(depositInstruction)
      .succeeds()
      .check(WCu.le(1_556n))
      .check(WLamports.eq(vault, depositAmount))
      .check(WLamports.eq(user, startingLamports - depositAmount));
    expect(deposit.accountChanges.map(change => change.address)).toEqual([
      user,
      vault,
    ]);
    expect(deposit.accountChanges[1]?.before).toBeNull();
    expect(deposit.accountChanges[1]?.wasCreated()).toBe(true);
    expect(deposit.accountChanges[1]?.wasRemoved()).toBe(false);
    expect(deposit.accountChanges[0]?.wasCreated()).toBe(false);
    expect(deposit.accountChanges[0]?.wasRemoved()).toBe(false);

    test.setAccount({
      accountId: vault,
      // Deposit leaves the vault system-owned; withdraw's CPI spends from it
      // with the vault's PDA seeds signing.
      accountInfo: {
        data: new Uint8Array(),
        executable: false,
        lamports: depositAmount,
        owner: new Web3Address("11111111111111111111111111111111"),
        rentEpoch: 0n,
        space: 0n,
      },
    });
    test
      .simulate(
        await client.createWithdrawInstruction({
          user,
          amount: withdrawalAmount,
        }),
      )
      .succeeds()
      .check(WCu.le(1_600n))
      .check(WLamports.eq(vault, depositAmount - withdrawalAmount))
      .check(
        WLamports.eq(user, startingLamports - depositAmount + withdrawalAmount),
      );
    expect(test.lamports(vault)).toBe(depositAmount);
    expect(test.lamports(user)).toBe(startingLamports - depositAmount);

    const wrongVault = new Web3Address(new Uint8Array(32).fill(9));
    const rejected = test
      .send(
        await client.createDepositInstructionRaw(
          { user, amount: 1n },
          { vault: wrongVault },
        ),
      )
      .failsWith(WEB3_PROGRAM_ERROR_CODES.InvalidPda);
    expect(rejected.account(wrongVault)).toBeNull();
    expect(rejected.accountChanges).toEqual([]);
    expect(test.account(wrongVault)).toBeNull();

    test
      .send(
        await client.createWithdrawInstruction({
          user,
          amount: depositAmount + 1n,
        }),
      )
      .fails({ type: "InsufficientFunds" })
      .check(WLamports.eq(vault, depositAmount));
    test.warpToTimestamp(42n);

    using limited = await Web3Test.open(Web3VaultClient.programId, elfPath, {
      computeUnitLimit: 1n,
    });
    await limited.add(web3Wallet({ address: user }));
    limited
      .send(await client.createDepositInstruction({ user, amount: 1n }))
      .fails({ type: "Runtime", message: "ProgramFailedToComplete" });
  });

});
