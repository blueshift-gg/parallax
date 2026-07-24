// Round-trip micro-benchmarks for the TypeScript shell, measured THROUGH the
// native FFI boundary. Run against the built `dist/` (npm run build) with:
//
//   node scripts/bench.mjs [coreSendNs]
//
// `coreSendNs` is the core-only send cost measured by the Rust
// `bench_send_round_trip` benchmark (default 2800 ns); the script subtracts it
// from the full TS round-trip to report the per-send FFI + wire + marshalling
// tax. Everything else — instruction encode, the boundary crossing, the outcome
// bundle decode, and adapter materialization — is what the shell adds on top of
// a core send.

import { address, AccountRole, getAddressDecoder } from "@solana/kit";
import { account, Test, wallet } from "../dist/kit.js";

const CORE_SEND_NS = Number(process.argv[2] ?? 2800);
const SYSTEM = address("11111111111111111111111111111111");
const MAX_U64 = 18_446_744_073_709_551_615n;

function transferData(lamports) {
  const data = new Uint8Array(12);
  const view = new DataView(data.buffer);
  view.setUint32(0, 2, true);
  view.setBigUint64(4, lamports, true);
  return data;
}

function addressFromSeed(seed) {
  const bytes = new Uint8Array(32);
  bytes[0] = seed;
  return getAddressDecoder().decode(bytes);
}

function bench(name, iters, body) {
  for (let i = 0; i < Math.max(1, iters >> 3); i += 1) body();
  const start = process.hrtime.bigint();
  for (let i = 0; i < iters; i += 1) body();
  const ns = Number(process.hrtime.bigint() - start) / iters;
  const perSec = (1e9 / ns).toFixed(0);
  console.log(`${name}: ${ns.toFixed(0)} ns/iter (${perSec}/s)`);
  return ns;
}

function transferIx(payer, recipient) {
  return {
    programAddress: SYSTEM,
    accounts: [
      { address: payer, role: AccountRole.WRITABLE_SIGNER },
      { address: recipient, role: AccountRole.WRITABLE },
    ],
    data: transferData(1n),
  };
}

async function benchSend(label, iters, recipientDataLen) {
  const test = new Test();
  const payer = await test.add(wallet({ fund: MAX_U64 }));
  const recipient = addressFromSeed(9);
  if (recipientDataLen > 0) {
    await test.add(
      account({
        address: recipient,
        owner: SYSTEM,
        lamports: 1_000_000n,
        data: new Uint8Array(recipientDataLen).fill(7),
      }),
    );
  }
  const ix = transferIx(payer, recipient);
  const ns = bench(label, iters, () => {
    test.send(ix);
  });
  console.log(
    `  FFI tax vs core (${CORE_SEND_NS} ns): ${(ns - CORE_SEND_NS).toFixed(0)} ns/send`,
  );
  test.free();
  return ns;
}

async function benchInstall() {
  // Per-fixture wallet install through the FFI: one boundary crossing plus the
  // litesvm set_account it drives, against a single persistent world (the world
  // build is one-time, not per fixture).
  const test = new Test();
  bench("ts_install_wallet", 20_000, () => {
    test.installWallet(undefined, MAX_U64);
  });
  test.free();
}

async function main() {
  console.log(`# TypeScript FFI round-trip benchmarks (core send = ${CORE_SEND_NS} ns)`);
  await benchSend("ts_send_small_0b", 20_000, 0);
  await benchSend("ts_send_large_10k", 20_000, 10_240);
  await benchInstall();
}

await main();
