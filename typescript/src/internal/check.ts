import {
  decodeAccount,
  type AccountCodec,
  type Check,
  type Outcome,
  type OutcomeAdapter,
} from "./outcome.js";

/**
 * The built-in fact namespaces, mirroring Rust: name the fact, bind its
 * subject, then compare — `check([Cu.spent().le(5_000),
 * Account.lamports(vault).eq(n)])`. Every value is a plain `Check` function,
 * so heterogeneous facts group in an array.
 */
export interface Checks<Address, Account> {
  /** Compute-unit facts: `Cu.spent().le(5_000)`. */
  readonly Cu: {
    /** The compute units the transaction consumed. */
    spent(): Comparators<Address, Account>;
  };
  /** Account-scoped facts: lamports, owner, raw data, and typed state. */
  readonly Account: {
    /** The lamport balance of the account at `address`. */
    lamports(address: Address): Comparators<Address, Account>;
    /** The owner of the account: `Account.owner(vault).eq(programId)`. */
    owner(address: Address): {
      eq(program: Address): Check<Address, Account>;
    };
    /** The raw data bytes of the account — the raw sibling of `state`. */
    data(address: Address): {
      eq(expected: Uint8Array | readonly number[]): Check<Address, Account>;
    };
    /**
     * The typed state of the account, decoded through a generated codec — the
     * same validate-and-decode path as `Test.read`.
     */
    state<Value>(
      codec: AccountCodec<Value, Address>,
      address: Address,
    ): {
      /** Assert the account decodes to exactly `expected` (deep equality). */
      eq(expected: Value): Check<Address, Account>;
      /** Assert on the decoded state with a closure, for partial facts. */
      with(check: (state: Value) => void): Check<Address, Account>;
    };
  };
  /** Token-program facts; both read Token or Token-2022 accounts. */
  readonly Token: {
    /** The token balance of the token account at `address`. */
    amount(address: Address): Comparators<Address, Account>;
    /** The supply of the mint at `address`. */
    supply(address: Address): Comparators<Address, Account>;
  };
  /** Transaction return-data facts. */
  readonly ReturnData: {
    eq(expected: Uint8Array | readonly number[]): Check<Address, Account>;
  };
  /** Changed-account facts, from the transaction's writable before/after set. */
  readonly Changes: {
    /** Assert the exact changed set, in first-appearance order. */
    eq(addresses: readonly Address[]): Check<Address, Account>;
    /** Assert the transaction created the account (absent before). */
    created(address: Address): Check<Address, Account>;
    /** Assert the transaction removed the account (absent after). */
    removed(address: Address): Check<Address, Account>;
    /** Assert Solana's closed-account state at `address`. */
    closed(address: Address): Check<Address, Account>;
  };
}

/** A bound numeric fact awaiting its comparator. */
export interface Comparators<Address, Account> {
  eq(expected: bigint | number): Check<Address, Account>;
  le(expected: bigint | number): Check<Address, Account>;
  lt(expected: bigint | number): Check<Address, Account>;
  ge(expected: bigint | number): Check<Address, Account>;
  gt(expected: bigint | number): Check<Address, Account>;
}

type Op = "==" | "<=" | "<" | ">=" | ">";

const HOLDS: Record<Op, (actual: bigint, expected: bigint) => boolean> = {
  "==": (a, e) => a === e,
  "<=": (a, e) => a <= e,
  "<": (a, e) => a < e,
  ">=": (a, e) => a >= e,
  ">": (a, e) => a > e,
};

function bytesEqual(actual: Uint8Array, expected: ArrayLike<number>): boolean {
  return (
    actual.length === expected.length &&
    actual.every((byte, index) => byte === expected[index])
  );
}

function describeBytes(bytes: ArrayLike<number>): string {
  return `[${Array.from(bytes).join(", ")}]`;
}

/** Deep equality over decoded state: primitives, bigints, bytes, arrays, objects. */
function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (a instanceof Uint8Array && b instanceof Uint8Array) {
    return bytesEqual(a, b);
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  if (
    typeof a === "object" &&
    typeof b === "object" &&
    a !== null &&
    b !== null
  ) {
    const keysA = Object.keys(a);
    const keysB = Object.keys(b);
    return (
      keysA.length === keysB.length &&
      keysA.every(key =>
        deepEqual(
          (a as Record<string, unknown>)[key],
          (b as Record<string, unknown>)[key],
        ),
      )
    );
  }
  return false;
}

function render(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) => {
    if (typeof v === "bigint") return `${v}n`;
    if (v instanceof Uint8Array) return describeBytes(v);
    return v;
  });
}

/** Build the fact namespaces over one adapter's account/address model. */
export function createChecks<Address, Account>(
  adapter: OutcomeAdapter<Address, Account>,
): Checks<Address, Account> {
  type C = Check<Address, Account>;
  type O = Outcome<Address, Account>;

  const requiredAccount = (outcome: O, address: Address): Account => {
    const account = outcome.account(address);
    if (account === null) {
      throw new Error(
        `outcome does not contain account ${adapter.renderAddress(address)}`,
      );
    }
    return account;
  };

  const requiredChange = (outcome: O, address: Address) => {
    const key = adapter.addressKey(address);
    const change = outcome.accountChanges.find(
      candidate => adapter.addressKey(candidate.address) === key,
    );
    if (change === undefined) {
      throw new Error(
        `this transaction did not change account ${adapter.renderAddress(address)}`,
      );
    }
    return change;
  };

  const comparators = (
    label: string,
    read: (outcome: O) => bigint,
  ): Comparators<Address, Account> => {
    const make =
      (op: Op) =>
      (expected: bigint | number): C =>
      outcome => {
        const actual = read(outcome);
        if (!HOLDS[op](actual, BigInt(expected))) {
          throw new Error(
            `${label}: expected ${op} ${expected}, got ${actual}`,
          );
        }
      };
    return {
      eq: make("=="),
      le: make("<="),
      lt: make("<"),
      ge: make(">="),
      gt: make(">"),
    };
  };

  const accountValue = (
    label: string,
    read: (account: Account) => bigint,
  ) => (address: Address) =>
    comparators(`${label} of ${adapter.renderAddress(address)}`, outcome =>
      read(requiredAccount(outcome, address)),
    );

  return {
    Cu: {
      spent: () =>
        comparators("compute units", outcome => outcome.computeUnits),
    },
    Account: {
      lamports: accountValue("lamports", adapter.lamports),
      owner: address => ({
        eq: (program): C => outcome => {
          const owner = adapter.accountOwner(requiredAccount(outcome, address));
          if (adapter.addressKey(owner) !== adapter.addressKey(program)) {
            throw new Error(
              `account ${adapter.renderAddress(address)} is owned by ${adapter.renderAddress(owner)}, expected ${adapter.renderAddress(program)}`,
            );
          }
        },
      }),
      data: address => ({
        eq: (expected): C => outcome => {
          const data = adapter.accountData(requiredAccount(outcome, address));
          if (!bytesEqual(data, expected)) {
            throw new Error(
              `unexpected account data for ${adapter.renderAddress(address)}: expected ${describeBytes(expected)}, got ${describeBytes(data)}`,
            );
          }
        },
      }),
      state: (codec, address) => ({
        eq: (expected): C => outcome => {
          const actual = decodeAccount(
            codec,
            address,
            requiredAccount(outcome, address),
            adapter,
          );
          if (!deepEqual(actual, expected)) {
            throw new Error(
              `unexpected state for ${adapter.renderAddress(address)}: expected ${render(expected)}, got ${render(actual)}`,
            );
          }
        },
        with: (check): C => outcome => {
          check(
            decodeAccount(
              codec,
              address,
              requiredAccount(outcome, address),
              adapter,
            ),
          );
        },
      }),
    },
    Token: {
      amount: accountValue("token balance", adapter.tokenAmount),
      supply: accountValue("mint supply", adapter.mintSupply),
    },
    ReturnData: {
      eq: (expected): C => outcome => {
        if (!bytesEqual(outcome.returnData, expected)) {
          throw new Error(
            `unexpected return data: expected ${describeBytes(expected)}, got ${describeBytes(outcome.returnData)}`,
          );
        }
      },
    },
    Changes: {
      eq: (addresses): C => outcome => {
        const actual = outcome.accountChanges.map(change =>
          adapter.addressKey(change.address),
        );
        const expected = addresses.map(address => adapter.addressKey(address));
        if (
          actual.length !== expected.length ||
          actual.some((key, index) => key !== expected[index])
        ) {
          throw new Error(
            `unexpected changed-account set (first-appearance order): expected [${addresses
              .map(address => adapter.renderAddress(address))
              .join(", ")}], got [${outcome.accountChanges
              .map(change => adapter.renderAddress(change.address))
              .join(", ")}]`,
          );
        }
      },
      created: (address): C => outcome => {
        if (!requiredChange(outcome, address).wasCreated()) {
          throw new Error(
            `account ${adapter.renderAddress(address)} was not created by this transaction`,
          );
        }
      },
      removed: (address): C => outcome => {
        if (!requiredChange(outcome, address).wasRemoved()) {
          throw new Error(
            `account ${adapter.renderAddress(address)} was not removed by this transaction`,
          );
        }
      },
      closed: (address): C => outcome => {
        const account = outcome.account(address);
        if (account !== null && !adapter.isClosed(account)) {
          throw new Error(
            `account ${adapter.renderAddress(address)} is not closed`,
          );
        }
      },
    },
  };
}
