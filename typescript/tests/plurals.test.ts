import { describe, expect, it } from "vitest";
import { getAddressDecoder, type Address as KitAddress } from "@solana/kit";
import { Address } from "@solana/web3.js";
import {
  Ctx as KitTest,
  mint as kitMint,
  wallet as kitWallet,
} from "../src/kit.js";
import { Ctx as Web3Test, wallet as web3Wallet } from "../src/web3.js";

let seed = 0;
function kitAddr(): KitAddress {
  seed += 1;
  const bytes = new Uint8Array(32);
  new DataView(bytes.buffer).setUint32(0, seed, true);
  return getAddressDecoder().decode(bytes) as KitAddress;
}

describe("plural fixtures", () => {
  // `count: N` mirrors Rust's const-generic plural: N distinct accounts, one
  // shared configuration, even for a mint.
  it("installs N fresh fixtures with count (Kit)", async () => {
    using test = new KitTest();
    const wallets = await test.add(kitWallet({ fund: 7n, count: 3 }));
    expect(wallets).toHaveLength(3);
    for (const w of wallets) expect(test.lamports(w)).toBe(7n);
    expect(new Set(wallets).size).toBe(3);

    const [m1, m2] = await test.add(kitMint({ supply: 1_000n, count: 2 }));
    expect(m1).not.toBe(m2);
    expect(test.supply(m1)).toBe(1_000n);
    expect(test.supply(m2)).toBe(1_000n);
  });

  // `accounts: [..]` mirrors Rust `Wallet::accounts([..])`: one config, pinned
  // at each address, destructurable.
  it("pins one fixture per address with accounts (Kit)", async () => {
    using test = new KitTest();
    const a = kitAddr();
    const b = kitAddr();
    const [alice, bob] = await test.add(
      kitWallet({ accounts: [a, b], fund: 5_000n }),
    );
    expect(alice).toBe(a);
    expect(bob).toBe(b);
    expect(test.lamports(a)).toBe(5_000n);
    expect(test.lamports(b)).toBe(5_000n);
  });

  it("installs N fresh fixtures with count (Web3.js)", async () => {
    using test = new Web3Test();
    const [a, b] = await test.add(web3Wallet({ fund: 3n, count: 2 }));
    expect(test.lamports(a)).toBe(3n);
    expect(test.lamports(b)).toBe(3n);
    expect((a as Address).equals(b as Address)).toBe(false);
  });
});
