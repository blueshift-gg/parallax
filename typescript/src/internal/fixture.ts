export const TokenProgram = {
  Legacy: "legacy",
  Token2022: "token2022",
} as const;

export type TokenProgram = (typeof TokenProgram)[keyof typeof TokenProgram];

export interface Fixture<Output, Host> {
  install(test: Host): Output | Promise<Output>;
}

/** The default balance a `wallet()` fixture funds: ten SOL, matching Rust. */
export const DEFAULT_WALLET_LAMPORTS = 10_000_000_000n;

/**
 * A wallet and the amount of a mint to seed it with, as an `[owner, amount]`
 * pair. Mirrors one entry of Rust `Mint::with_holder`.
 */
export type MintHolder<Address> = readonly [owner: Address, amount: bigint];

export interface WalletOptions<Address> {
  address?: Address;
  /**
   * Token balances to hold through the wallet's associated token accounts,
   * installed alongside it — `[mint, amount]` pairs. Mirrors Rust
   * `Wallet::holding`.
   */
  holdings?: readonly (readonly [Address, bigint])[];
  /** Exact lamport balance, mirroring Rust `Wallet::fund`. Defaults to
   * `DEFAULT_WALLET_LAMPORTS`. */
  fund?: bigint;
  /**
   * Install one wallet at each address — the pinned plural, mirroring Rust
   * `Wallet::accounts([..])`. Returns `Address[]`.
   */
  accounts?: readonly Address[];
  /**
   * Install `count` fresh wallets sharing this config — the count plural.
   * Returns `Address[]`.
   */
  count?: number;
}

export interface MintOptions<Address> {
  /**
   * Mint authority, mirroring Rust `Mint::with_authority`. Omitted, the mint is
   * fixed-supply (its `mintAuthority` is `COption::None`).
   */
  authority?: Address;
  /**
   * Freeze authority, mirroring Rust `Mint::with_freeze_authority`. Omitted, the
   * mint cannot freeze accounts.
   */
  freezeAuthority?: Address;
  supply?: bigint;
  decimals?: number;
  tokenProgram?: TokenProgram;
  /**
   * Wallets to fund with an associated token account for this mint, mirroring
   * Rust `Mint::with_holder`. One ATA is installed per `[owner, amount]` pair.
   */
  holders?: readonly MintHolder<Address>[];
  /**
   * Install `count` fresh mints sharing this config — the count plural,
   * mirroring Rust `Mint::accounts()`. Returns `Address[]`.
   */
  count?: number;
}

/** A raw, backend-neutral account fixture. Address and owner are required. */
export interface AccountOptions<Address> {
  address: Address;
  owner: Address;
  lamports?: bigint;
  data?: Uint8Array;
}

export interface TokenAccountOptions<Address> {
  address?: Address;
  amount?: bigint;
  tokenProgram?: TokenProgram;
  /**
   * Install one token account at each address — the pinned plural, mirroring
   * Rust `TokenAccount::accounts([..])`. Returns `Address[]`.
   */
  accounts?: readonly Address[];
  /**
   * Install `count` fresh token accounts sharing this config — the count
   * plural. Returns `Address[]`.
   */
  count?: number;
}

export interface AssociatedTokenAccountOptions {
  amount?: bigint;
  tokenProgram?: TokenProgram;
}

/**
 * Default Solana rent-exempt minimum for `dataLen` bytes:
 * `(dataLen + 128) * 3480 * 2`. Matches the kernel's default rent so `write`
 * and the `account` fixture produce rent-exempt accounts without a syscall.
 */
export function rentMinimumBalance(dataLen: number): bigint {
  return BigInt(dataLen + 128) * 3480n * 2n;
}

/** Option-bag for a mint install, resolved from `MintOptions`. */
export interface MintInstall<Address> {
  readonly authority: Address | undefined;
  readonly freezeAuthority: Address | undefined;
  readonly supply: bigint;
  readonly decimals: number;
  readonly tokenProgram: TokenProgram;
  readonly holders: readonly MintHolder<Address>[];
}

/**
 * The install surface a fixture drives. Every method installs into the Rust
 * kernel and returns the deterministic address the kernel assigned, so the
 * TypeScript side never derives placement itself.
 */
export interface FixtureHost<Address> {
  installWallet(address: Address | undefined, fund: bigint | undefined): Address;
  installMint(options: MintInstall<Address>): Address;
  installTokenAccount(
    mint: Address,
    owner: Address,
    address: Address | undefined,
    amount: bigint,
    tokenProgram: TokenProgram,
  ): Address;
  installAta(
    mint: Address,
    owner: Address,
    amount: bigint,
    tokenProgram: TokenProgram,
  ): Address;
  installRawAccount(
    address: Address,
    owner: Address,
    lamports: bigint | undefined,
    data: Uint8Array,
  ): Address;
  preloadProgram(programId: Address, elf: Uint8Array): Address;
  dumpAccounts(
    addresses: readonly Address[],
    syncClock: boolean,
  ): Promise<readonly Address[]>;
  dumpProgram(programId: Address, syncClock: boolean): Promise<Address>;
  refreshAll(): Promise<Address[]>;
  loadFile(path: string, isProgram: boolean): Address[];
}

/** Option-bag for `dump({ accounts, syncClock? })`. */
export interface DumpOptions<Address> {
  /** Mainnet addresses to dump. Returned unchanged, in the same arity. */
  readonly accounts: readonly Address[];
  /** Adopt the dumped slot's clock. Opt-in; off by default. */
  readonly syncClock?: boolean;
}

/** Option-bag for `load({ path })`. */
export interface LoadOptions {
  /** Path to a dump file (the same format the `.parallax/` store writes). */
  readonly path: string;
}

export function createFixtureFactories<
  Address,
  Host extends FixtureHost<Address>,
>() {
  // `wallet` mirrors Rust's singular/plural: `wallet({ accounts: [A, B] })` pins
  // one wallet per address, `wallet({ count: N })` installs N fresh wallets, and
  // both yield `Address[]`; the bare option-bag stays singular.
  function wallet(
    options: WalletOptions<Address> & {
      accounts: readonly Address[];
      address?: never;
      count?: never;
    },
  ): Fixture<Address[], Host>;
  function wallet(
    options: WalletOptions<Address> & {
      count: number;
      address?: never;
      accounts?: never;
    },
  ): Fixture<Address[], Host>;
  function wallet(
    options?: WalletOptions<Address> & { accounts?: never; count?: never },
  ): Fixture<Address, Host>;
  function wallet(
    options: WalletOptions<Address> = {},
  ): Fixture<Address, Host> | Fixture<Address[], Host> {
    if (options.accounts !== undefined) {
      const addresses = options.accounts;
      return {
        install: test =>
          addresses.map(address => test.installWallet(address, options.fund)),
      };
    }
    if (options.count !== undefined) {
      const count = options.count;
      return {
        install: test =>
          Array.from({ length: count }, () =>
            test.installWallet(options.address, options.fund),
          ),
      };
    }
    return {
      install: test => {
        const address = test.installWallet(options.address, options.fund);
        for (const [mint, amount] of options.holdings ?? []) {
          test.installAta(mint, address, amount, TokenProgram.Legacy);
        }
        return address;
      },
    };
  }

  // `mint({ count: N })` installs N fresh mints sharing this config → Address[].
  function mint(
    options: MintOptions<Address> & { count: number },
  ): Fixture<Address[], Host>;
  function mint(options?: MintOptions<Address>): Fixture<Address, Host>;
  function mint(
    options: MintOptions<Address> = {},
  ): Fixture<Address, Host> | Fixture<Address[], Host> {
    const one = (test: Host): Address =>
      test.installMint({
        authority: options.authority,
        freezeAuthority: options.freezeAuthority,
        supply: options.supply ?? 0n,
        decimals: options.decimals ?? 6,
        tokenProgram: options.tokenProgram ?? TokenProgram.Legacy,
        holders: options.holders ?? [],
      });
    if (options.count !== undefined) {
      const count = options.count;
      return { install: test => Array.from({ length: count }, () => one(test)) };
    }
    return { install: one };
  }

  // `tokenAccount(mint, owner, { accounts: [A, B] })` pins one per address;
  // `{ count: N }` installs N fresh accounts. Both yield `Address[]`.
  function tokenAccount(
    mint: Address,
    owner: Address,
    options: TokenAccountOptions<Address> & {
      accounts: readonly Address[];
      address?: never;
      count?: never;
    },
  ): Fixture<Address[], Host>;
  function tokenAccount(
    mint: Address,
    owner: Address,
    options: TokenAccountOptions<Address> & {
      count: number;
      address?: never;
      accounts?: never;
    },
  ): Fixture<Address[], Host>;
  function tokenAccount(
    mint: Address,
    owner: Address,
    options?: TokenAccountOptions<Address> & { accounts?: never; count?: never },
  ): Fixture<Address, Host>;
  function tokenAccount(
    mint: Address,
    owner: Address,
    options: TokenAccountOptions<Address> = {},
  ): Fixture<Address, Host> | Fixture<Address[], Host> {
    const amount = options.amount ?? 0n;
    const tokenProgram = options.tokenProgram ?? TokenProgram.Legacy;
    if (options.accounts !== undefined) {
      const addresses = options.accounts;
      return {
        install: test =>
          addresses.map(address =>
            test.installTokenAccount(mint, owner, address, amount, tokenProgram),
          ),
      };
    }
    if (options.count !== undefined) {
      const count = options.count;
      return {
        install: test =>
          Array.from({ length: count }, () =>
            test.installTokenAccount(mint, owner, options.address, amount, tokenProgram),
          ),
      };
    }
    return {
      install: test =>
        test.installTokenAccount(mint, owner, options.address, amount, tokenProgram),
    };
  }

  return {
    wallet,
    mint,
    tokenAccount,

    account(options: AccountOptions<Address>): Fixture<Address, Host> {
      return {
        install: test =>
          test.installRawAccount(
            options.address,
            options.owner,
            options.lamports,
            options.data ?? new Uint8Array(),
          ),
      };
    },

    associatedTokenAccount(
      mint: Address,
      owner: Address,
      options: AssociatedTokenAccountOptions = {},
    ): Fixture<Address, Host> {
      return {
        install: test =>
          test.installAta(
            mint,
            owner,
            options.amount ?? 0n,
            options.tokenProgram ?? TokenProgram.Legacy,
          ),
      };
    },

    program(programId: Address, elf: Uint8Array): Fixture<Address, Host> {
      return {
        install: test => test.preloadProgram(programId, elf),
      };
    },

    // `dump({ accounts })` copies mainnet accounts into the world through the
    // committed `.parallax/` store; `dump.program(id)` dumps and loads a real
    // program; `dump.refreshAll()` re-fetches every stored entry at one slot.
    dump: Object.assign(
      <const T extends readonly Address[]>(options: {
        accounts: T;
        syncClock?: boolean;
      }): Fixture<T, Host> => ({
        install: async test => {
          await test.dumpAccounts(options.accounts, options.syncClock ?? false);
          return options.accounts;
        },
      }),
      {
        program(
          programId: Address,
          options: { syncClock?: boolean } = {},
        ): Fixture<Address, Host> {
          return {
            install: test => test.dumpProgram(programId, options.syncClock ?? false),
          };
        },
        refreshAll(): Fixture<Address[], Host> {
          return { install: test => test.refreshAll() };
        },
      },
    ),

    // `load({ path })` installs accounts from an already-dumped file (the same
    // format the store writes); `load.program(path)` loads a dumped program.
    // No store, no network — the core reads and parses the file.
    load: Object.assign(
      (options: LoadOptions): Fixture<Address[], Host> => ({
        install: test => test.loadFile(options.path, false),
      }),
      {
        program(path: string): Fixture<Address, Host> {
          return {
            install: test => {
              const [id] = test.loadFile(path, true);
              if (id === undefined) {
                throw new Error(`load: ${path} contains no program`);
              }
              return id;
            },
          };
        },
      },
    ),
  };
}
