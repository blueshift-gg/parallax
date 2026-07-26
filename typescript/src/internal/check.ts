import {
  decodeAccount,
  errorsEqual,
  type AccountCodec,
  type Check,
  type Outcome,
  type OutcomeAdapter,
  type ProgramError,
} from "./outcome.js";

/**
 * The verdict facts, mirroring Rust's `Outcome::success()`/`Outcome::error()`.
 * Shells re-export this as the value `Outcome`, alongside the outcome type.
 */
export const OutcomeFacts = {
  /** Assert the transaction succeeded. Optional before other facts — they
   * self-diagnose on a failed transaction — and useful to state intent. */
  success<Address, Account>(): Check<Address, Account> {
    return tx => {
      if (tx.failure !== null) {
        throw new Error(
          `expected success, got ${JSON.stringify(tx.failure)}${tx.diagnostics()}`,
        );
      }
    };
  },
  /** Assert the transaction failed with exactly this error: a custom code, or
   * a full `ProgramError`. */
  error<Address, Account>(
    expected: number | ProgramError,
  ): Check<Address, Account> {
    const wanted: ProgramError =
      typeof expected === "number" ? { type: "Custom", code: expected } : expected;
    return tx => {
      if (tx.failure === null || !errorsEqual(tx.failure, wanted)) {
        throw new Error(
          `expected error ${JSON.stringify(wanted)}, got ${JSON.stringify(tx.failure)}${tx.diagnostics()}`,
        );
      }
    };
  },
};

/** Facts self-diagnose: evaluated against a failed transaction they throw with
 * the transaction's error and logs rather than silently judging pre-state. */
function live<Address, Account>(tx: Outcome<Address, Account>): void {
  if (tx.failure !== null) {
    throw new Error(
      `fact checked against a failed transaction: ${JSON.stringify(tx.failure)}${tx.diagnostics()}`,
    );
  }
}

/**
 * Convert several checks into one check. A bundle is itself a `Check`, so
 * bundles nest, and a protocol's standard assertions become a plain function
 * returning one. Mirrors Rust `bundle`.
 */
export function bundle<Address, Account>(
  checks: Iterable<Check<Address, Account>>,
): Check<Address, Account> {
  const collected = [...checks];
  return tx => {
    for (const check of collected) check(tx);
  };
}

/** A value expectation (equality) or a predicate over the measured value. */
export type ExpectedValue<T> = T | ((actual: T) => boolean);

/** An expectation over raw bytes: exact bytes, or a predicate over them. */
export type ExpectedBytes =
  | Uint8Array
  | readonly number[]
  | ((actual: Uint8Array) => boolean);

/**
 * The fact namespaces, mirroring Rust: every fact takes its subject and one
 * expectation — a plain value means equality, a closure is a predicate.
 */
export interface Checks<Address, Account> {
  /** Compute-unit facts: `Cu.spent(cu => cu <= 5_000n)`, `Cu.spent(1_556n)`. */
  readonly Cu: {
    spent(expected: ExpectedValue<bigint | number>): Check<Address, Account>;
  };
  /** Account-scoped facts: lamports, owner, data, and the lifecycle. */
  readonly Account: {
    lamports(
      address: Address,
      expected: ExpectedValue<bigint | number>,
    ): Check<Address, Account>;
    owner(
      address: Address,
      expected: Address | ((actual: Address) => boolean),
    ): Check<Address, Account>;
    /**
     * The account's data: raw bytes with `data(address, expected)`, or decoded
     * through a generated codec with `data(codec, address, predicate)` — the
     * same validate-and-decode path as `Test.read`.
     */
    data(address: Address, expected: ExpectedBytes): Check<Address, Account>;
    data<Value>(
      codec: AccountCodec<Value, Address>,
      address: Address,
      predicate: (actual: Value) => boolean,
    ): Check<Address, Account>;
    /** Assert the transaction created the account (absent before). */
    created(address: Address): Check<Address, Account>;
    /** Assert the transaction removed the account (absent after). */
    removed(address: Address): Check<Address, Account>;
    /** Assert Solana's closed-account state at `address`. */
    closed(address: Address): Check<Address, Account>;
  };
  /** The mint fact; reads Token or Token-2022 mints. */
  readonly Mint: {
    supply(
      address: Address,
      expected: ExpectedValue<bigint | number>,
    ): Check<Address, Account>;
  };
  /** The token-account fact; reads Token or Token-2022 accounts. */
  readonly TokenAccount: {
    amount(
      address: Address,
      expected: ExpectedValue<bigint | number>,
    ): Check<Address, Account>;
  };
  /** Transaction return-data facts. */
  readonly ReturnData: {
    is(expected: ExpectedBytes): Check<Address, Account>;
  };
}

function describeBytes(bytes: ArrayLike<number>): string {
  return `[${Array.from(bytes).join(", ")}]`;
}

function bytesEqual(actual: Uint8Array, expected: ArrayLike<number>): boolean {
  return (
    actual.length === expected.length &&
    actual.every((byte, index) => byte === expected[index])
  );
}

function renderValue(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) =>
    typeof v === "bigint" ? `${v}n` : v,
  );
}

/** Build the fact namespaces over one adapter's account/address model. */
export function createChecks<Address, Account>(
  adapter: OutcomeAdapter<Address, Account>,
): Checks<Address, Account> {
  type C = Check<Address, Account>;
  type O = Outcome<Address, Account>;

  const requiredAccount = (tx: O, address: Address): Account => {
    live(tx);
    const account = tx.account(address);
    if (account === null) {
      throw new Error(
        `transaction does not contain account ${adapter.renderAddress(address)}`,
      );
    }
    return account;
  };

  const requiredChange = (tx: O, address: Address) => {
    live(tx);
    const key = adapter.addressKey(address);
    const change = tx.accountChanges.find(
      candidate => adapter.addressKey(candidate.address) === key,
    );
    if (change === undefined) {
      throw new Error(
        `this transaction did not change account ${adapter.renderAddress(address)}`,
      );
    }
    return change;
  };

  // A value expectation prints `expected N, got M`; a predicate cannot print
  // its source, so it fails with the actual value alone (the stack trace
  // names the test line that built it).
  const numeric =
    (label: string, read: (tx: O) => bigint) =>
    (expected: ExpectedValue<bigint | number>): C =>
    tx => {
      live(tx);
      const actual = read(tx);
      if (typeof expected === "function") {
        if (!expected(actual)) {
          throw new Error(`${label}: predicate failed — actual: ${actual}`);
        }
      } else if (actual !== BigInt(expected)) {
        throw new Error(`${label}: expected ${expected}, got ${actual}`);
      }
    };

  const bytes =
    (label: string, read: (tx: O) => Uint8Array) =>
    (expected: ExpectedBytes): C =>
    tx => {
      live(tx);
      const actual = read(tx);
      if (typeof expected === "function") {
        if (!expected(actual)) {
          throw new Error(
            `${label}: predicate failed — actual: ${describeBytes(actual)}`,
          );
        }
      } else if (!bytesEqual(actual, expected)) {
        throw new Error(
          `${label}: expected ${describeBytes(expected)}, got ${describeBytes(actual)}`,
        );
      }
    };

  function data(address: Address, expected: ExpectedBytes): C;
  function data<Value>(
    codec: AccountCodec<Value, Address>,
    address: Address,
    predicate: (actual: Value) => boolean,
  ): C;
  function data<Value>(
    first: Address | AccountCodec<Value, Address>,
    second: Address | ExpectedBytes,
    third?: (actual: Value) => boolean,
  ): C {
    if (third !== undefined) {
      const codec = first as AccountCodec<Value, Address>;
      const address = second as Address;
      return tx => {
        const actual = decodeAccount(
          codec,
          address,
          requiredAccount(tx, address),
          adapter,
        );
        if (!third(actual)) {
          throw new Error(
            `data of ${adapter.renderAddress(address)}: predicate failed — actual: ${renderValue(actual)}`,
          );
        }
      };
    }
    const address = first as Address;
    return bytes(`data of ${adapter.renderAddress(address)}`, tx =>
      adapter.accountData(requiredAccount(tx, address)),
    )(second as ExpectedBytes);
  }

  return {
    Cu: {
      spent: expected => numeric("compute units", tx => tx.computeUnits)(expected),
    },
    Account: {
      lamports: (address, expected) =>
        numeric(`lamports of ${adapter.renderAddress(address)}`, tx =>
          adapter.lamports(requiredAccount(tx, address)),
        )(expected),
      owner: (address, expected) => tx => {
        const actual = adapter.accountOwner(requiredAccount(tx, address));
        const label = `owner of ${adapter.renderAddress(address)}`;
        if (typeof expected === "function") {
          if (!(expected as (actual: Address) => boolean)(actual)) {
            throw new Error(
              `${label}: predicate failed — actual: ${adapter.renderAddress(actual)}`,
            );
          }
        } else if (adapter.addressKey(actual) !== adapter.addressKey(expected)) {
          throw new Error(
            `${label}: expected ${adapter.renderAddress(expected)}, got ${adapter.renderAddress(actual)}`,
          );
        }
      },
      data,
      created: address => tx => {
        if (!requiredChange(tx, address).wasCreated()) {
          throw new Error(
            `account ${adapter.renderAddress(address)} was not created by this transaction`,
          );
        }
      },
      removed: address => tx => {
        if (!requiredChange(tx, address).wasRemoved()) {
          throw new Error(
            `account ${adapter.renderAddress(address)} was not removed by this transaction`,
          );
        }
      },
      closed: address => tx => {
        live(tx);
        const account = tx.account(address);
        if (account !== null && !adapter.isClosed(account)) {
          throw new Error(
            `account ${adapter.renderAddress(address)} is not closed`,
          );
        }
      },
    },
    Mint: {
      supply: (address, expected) =>
        numeric(`supply of ${adapter.renderAddress(address)}`, tx =>
          adapter.mintSupply(requiredAccount(tx, address)),
        )(expected),
    },
    TokenAccount: {
      amount: (address, expected) =>
        numeric(`token balance of ${adapter.renderAddress(address)}`, tx =>
          adapter.tokenAmount(requiredAccount(tx, address)),
        )(expected),
    },
    ReturnData: {
      is: expected => bytes("return data", tx => tx.returnData)(expected),
    },
  };
}
