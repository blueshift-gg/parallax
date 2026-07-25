import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AccountRole,
  address,
  getAddressDecoder,
  type Address,
  type Instruction,
} from "@solana/kit";
import { Test as KitTest, dump, load } from "../src/kit.js";

const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

const addressDecoder = getAddressDecoder();
function addr(fill: number): Address {
  return addressDecoder.decode(new Uint8Array(32).fill(fill));
}

/** A throwaway project dir with a `package.json`, so the store resolves to it. */
function makeProject(): string {
  const dir = mkdtempSync(path.join(tmpdir(), "parallax-dump-"));
  writeFileSync(path.join(dir, "package.json"), JSON.stringify({ name: "tmp" }));
  return dir;
}

/** A fetch stub returning `response` bytes; records how many times it ran. */
function fetchReturning(response: Uint8Array): { calls: () => number } {
  let calls = 0;
  vi.stubGlobal("fetch", async () => {
    calls += 1;
    return { ok: true, arrayBuffer: async () => response.buffer };
  });
  return { calls: () => calls };
}

/** A fetch stub that fails the test if the network is touched at all. */
function fetchThatThrows(): void {
  vi.stubGlobal("fetch", () => {
    throw new Error("this run must not touch the network");
  });
}

/** A `getMultipleAccounts` response with the given per-address values. */
function rpcResponse(
  slot: number,
  values: (null | { lamports: number; owner: string; data: string })[],
): Uint8Array {
  return new TextEncoder().encode(
    JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      result: {
        context: { slot },
        value: values.map(value =>
          value === null
            ? null
            : {
                lamports: value.lamports,
                owner: value.owner,
                executable: false,
                data: [value.data, "base64"],
                rentEpoch: 0,
              },
        ),
      },
    }),
  );
}

/** Run `body` with the working directory pinned to `dir`, restored afterwards. */
async function inProject(dir: string, body: () => Promise<void>): Promise<void> {
  const cwd = process.cwd();
  process.chdir(dir);
  try {
    await body();
  } finally {
    process.chdir(cwd);
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("dump fixture", () => {
  const owner = addr(6);
  const data = new Uint8Array([1, 2, 3, 4]);
  const dataB64 = Buffer.from(data).toString("base64");

  it("serves a warm store fully offline and deterministically", async () => {
    const project = makeProject();
    const target = addr(5);

    await inProject(project, async () => {
      // First world: a miss fetches once and writes the store.
      fetchReturning(rpcResponse(1000, [{ lamports: 777, owner, data: dataB64 }]));
      const first = new KitTest();
      const [a] = await first.add(dump({ accounts: [target] }));

      // Second world: the store is warm, so any fetch is a failure.
      fetchThatThrows();
      const second = new KitTest();
      const [b] = await second.add(dump({ accounts: [target] }));

      try {
        expect(a).toBe(target);
        expect(b).toBe(target);
        const accountA = first.account(target);
        const accountB = second.account(target);
        expect(accountA).not.toBeNull();
        expect(accountA!.lamports).toBe(777n);
        expect(accountA!.data).toEqual(data);
        // Two fresh worlds install byte-identical state from the warm store.
        expect(accountB).toEqual(accountA);
      } finally {
        first.free();
        second.free();
      }
    });
    rmSync(project, { recursive: true, force: true });
  });

  it("fetches once on a miss, writes the store, and installs the account", async () => {
    const project = makeProject();
    const target = addr(7);
    let calls = 0;
    let requestBody: unknown;
    vi.stubGlobal("fetch", async (_url: string, init: { body: Uint8Array }) => {
      calls += 1;
      requestBody = JSON.parse(new TextDecoder().decode(init.body));
      const bytes = rpcResponse(1234, [{ lamports: 5, owner, data: dataB64 }]);
      return { ok: true, arrayBuffer: async () => bytes.buffer };
    });

    await inProject(project, async () => {
      const test = new KitTest();
      try {
        const [got] = await test.add(dump({ accounts: [target] }));
        expect(got).toBe(target);
        expect(calls).toBe(1);
        // One batched getMultipleAccounts call, base64, over the one address.
        expect((requestBody as { method: string }).method).toBe(
          "getMultipleAccounts",
        );
        expect((requestBody as { params: [string[]] }).params[0]).toEqual([
          target,
        ]);
        const account = test.account(target);
        expect(account!.data).toEqual(data);
        // The store writes one self-contained `<address>.dump` file.
        expect(existsSync(path.join(project, ".parallax", `${target}.dump`))).toBe(
          true,
        );
      } finally {
        test.free();
      }
    });
    rmSync(project, { recursive: true, force: true });
  });

  it("expands a program dump to the program and its programdata (core-side)", async () => {
    const project = makeProject();
    const programId = addr(9);
    let requestBody: { params: [string[]] } | undefined;
    // Both accounts return null (not a real program) so nothing is loaded; the
    // point is that the shell POSTs the two addresses the core expanded to.
    vi.stubGlobal("fetch", async (_url: string, init: { body: Uint8Array }) => {
      requestBody = JSON.parse(new TextDecoder().decode(init.body));
      const bytes = rpcResponse(1, [null, null]);
      return { ok: true, arrayBuffer: async () => bytes.buffer };
    });

    await inProject(project, async () => {
      const test = new KitTest();
      try {
        const id = await test.add(dump.program(programId));
        expect(id).toBe(programId);
        // Two addresses: the program and its derived programdata.
        expect(requestBody!.params[0].length).toBe(2);
        expect(requestBody!.params[0][0]).toBe(programId);
        expect(requestBody!.params[0][1]).not.toBe(programId);
      } finally {
        test.free();
      }
    });
    rmSync(project, { recursive: true, force: true });
  });

  it("loads a dumped file by path, offline, into a fresh world", async () => {
    const project = makeProject();
    const target = addr(11);
    // First, Dump writes `.parallax/<target>.dump` (a self-contained file).
    await inProject(project, async () => {
      vi.stubGlobal("fetch", async () => {
        const bytes = rpcResponse(1600, [{ lamports: 55, owner, data: dataB64 }]);
        return { ok: true, arrayBuffer: async () => bytes.buffer };
      });
      const test = new KitTest();
      try {
        await test.add(dump({ accounts: [target] }));
      } finally {
        test.free();
      }
    });
    const file = path.join(project, ".parallax", `${target}.dump`);
    expect(existsSync(file)).toBe(true);

    // A fresh world (no store, no network) Loads that exact file by path.
    vi.stubGlobal("fetch", () => {
      throw new Error("Load must not touch the network");
    });
    const test = new KitTest();
    try {
      const loaded = await test.add(load({ path: file }));
      expect(loaded).toEqual([target]);
      const account = test.account(target);
      expect(account!.lamports).toBe(55n);
      expect(account!.data).toEqual(data);
    } finally {
      test.free();
    }
    rmSync(project, { recursive: true, force: true });
  });

  it("attaches a guided hint when a send fails in a world with dumps", async () => {
    const project = makeProject();
    const target = addr(5);

    await inProject(project, async () => {
      fetchReturning(rpcResponse(1, [{ lamports: 1, owner, data: "" }]));
      const test = new KitTest();
      try {
        await test.add(dump({ accounts: [target] }));
        const bogus = addr(200);
        const failing: Instruction = {
          programAddress: address(TOKEN_PROGRAM),
          accounts: [{ address: bogus, role: AccountRole.READONLY }],
          data: new Uint8Array(),
        };
        const outcome = test.send(failing);
        expect(outcome.isErr()).toBe(true);
        expect(() => outcome.succeeds()).toThrow(/missing account/);
        expect(() => outcome.succeeds()).toThrow(/dump accounts fixture/);
      } finally {
        test.free();
      }
    });
    rmSync(project, { recursive: true, force: true });
  });
});
