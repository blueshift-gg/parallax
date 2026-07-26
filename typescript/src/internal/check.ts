import {
  decodeAccount,
  type AccountCodec,
  type Check,
  type Outcome,
  type OutcomeAdapter,
} from "./outcome.js";

/**
 * The built-in fact namespaces, mirroring Rust's `Assert` constructors: every
 * value is a plain `Check` function, so heterogeneous facts group in an array
 * — `check([CuBudget.le(5_000), Lamports.eq(vault, n)])`.
 */
export interface Checks<Address, Account> {
  /** Compute-unit budget facts: `CuBudget.le(5_000)`. */
  readonly CuBudget: Comparators<Address, Account>;
  /** Lamport-balance facts: `Lamports.eq(vault, amount)`. */
  readonly Lamports: AccountComparators<Address, Account>;
  /** Token-balance facts for Token or Token-2022 accounts. */
  readonly Tokens: AccountComparators<Address, Account>;
  /** Mint-supply facts for Token or Token-2022 mints. */
  readonly Supply: AccountComparators<Address, Account>;
  /** Account-ownership facts: `Owner.eq(vault, programId)`. */
  readonly Owner: {
    eq(address: Address, program: Address): Check<Address, Account>;
  };
  /** Raw account-data facts — the raw sibling of `State`. */
  readonly Data: {
    eq(
      address: Address,
      expected: Uint8Array | readonly number[],
    ): Check<Address, Account>;
  };
  /** Transaction return-data facts. */
  readonly ReturnData: {
    eq(expected: Uint8Array | readonly number[]): Check<Address, Account>;
  };
  /**
   * Typed account-state facts, decoded through a generated codec — the same
   * validate-and-decode path as `Test.read`.
   */
  readonly State: {
    /** Assert the account decodes to exactly `expected` (deep equality). */
    eq<Value>(
      codec: AccountCodec<Value, Address>,
      address: Address,
      expected: Value,
    ): Check<Address, Account>;
    /** Assert on the decoded state with a closure, for partial facts. */
    with<Value>(
      codec: AccountCodec<Value, Address>,
      address: Address,
      check: (state: Value) => void,
    ): Check<Address, Account>;
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

export interface Comparators<Address, Account> {
  eq(expected: bigint | number): Check<Address, Account>;
  le(expected: bigint | number): Check<Address, Account>;
  lt(expected: bigint | number): Check<Address, Account>;
  ge(expected: bigint | number): Check<Address, Account>;
  gt(expected: bigint | number): Check<Address, Account>;
}

export interface AccountComparators<Address, Account> {
  eq(address: Address, expected: bigint | number): Check<Address, Account>;
  le(address: Address, expected: bigint | number): Check<Address, Account>;
  lt(address: Address, expected: bigint | number): Check<Address, Account>;
  ge(address: Address, expected: bigint | number): Check<Address, Account>;
  gt(address: Address, expected: bigint | number): Check<Address, Account>;
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
    read: (outcome: O, address: Address) => bigint,
  ): AccountComparators<Address, Account> => {
    const make =
      (op: Op) =>
      (address: Address, expected: bigint | number): C =>
      outcome => {
        const actual = read(outcome, address);
        if (!HOLDS[op](actual, BigInt(expected))) {
          throw new Error(
            `${label} of ${adapter.renderAddress(address)}: expected ${op} ${expected}, got ${actual}`,
          );
        }
      };
    return { eq: make("=="), le: make("<="), lt: make("<"), ge: make(">="), gt: make(">") };
  };

  const cuMake =
    (op: Op) =>
    (expected: bigint | number): C =>
    outcome => {
      if (!HOLDS[op](outcome.computeUnits, BigInt(expected))) {
        throw new Error(
          `compute units: expected ${op} ${expected}, consumed ${outcome.computeUnits}`,
        );
      }
    };

  return {
    CuBudget: {
      eq: cuMake("=="),
      le: cuMake("<="),
      lt: cuMake("<"),
      ge: cuMake(">="),
      gt: cuMake(">"),
    },
    Lamports: comparators("lamports", (outcome, address) =>
      adapter.lamports(requiredAccount(outcome, address)),
    ),
    Tokens: comparators("token balance", (outcome, address) =>
      adapter.tokenAmount(requiredAccount(outcome, address)),
    ),
    Supply: comparators("mint supply", (outcome, address) =>
      adapter.mintSupply(requiredAccount(outcome, address)),
    ),
    Owner: {
      eq:
        (address, program): C =>
        outcome => {
          const owner = adapter.accountOwner(requiredAccount(outcome, address));
          if (adapter.addressKey(owner) !== adapter.addressKey(program)) {
            throw new Error(
              `account ${adapter.renderAddress(address)} is owned by ${adapter.renderAddress(owner)}, expected ${adapter.renderAddress(program)}`,
            );
          }
        },
    },
    Data: {
      eq:
        (address, expected): C =>
        outcome => {
          const data = adapter.accountData(requiredAccount(outcome, address));
          if (!bytesEqual(data, expected)) {
            throw new Error(
              `unexpected account data for ${adapter.renderAddress(address)}: expected ${describeBytes(expected)}, got ${describeBytes(data)}`,
            );
          }
        },
    },
    ReturnData: {
      eq:
        (expected): C =>
        outcome => {
          if (!bytesEqual(outcome.returnData, expected)) {
            throw new Error(
              `unexpected return data: expected ${describeBytes(expected)}, got ${describeBytes(outcome.returnData)}`,
            );
          }
        },
    },
    State: {
      eq:
        (codec, address, expected): C =>
        outcome => {
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
      with:
        (codec, address, check): C =>
        outcome => {
          check(
            decodeAccount(
              codec,
              address,
              requiredAccount(outcome, address),
              adapter,
            ),
          );
        },
    },
    Changes: {
      eq:
        (addresses): C =>
        outcome => {
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
      created:
        (address): C =>
        outcome => {
          if (!requiredChange(outcome, address).wasCreated()) {
            throw new Error(
              `account ${adapter.renderAddress(address)} was not created by this transaction`,
            );
          }
        },
      removed:
        (address): C =>
        outcome => {
          if (!requiredChange(outcome, address).wasRemoved()) {
            throw new Error(
              `account ${adapter.renderAddress(address)} was not removed by this transaction`,
            );
          }
        },
      closed:
        (address): C =>
        outcome => {
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
