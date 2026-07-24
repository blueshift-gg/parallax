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
  /** Exact lamport balance, mirroring Rust `Wallet::fund`. Defaults to
   * `DEFAULT_WALLET_LAMPORTS`. */
  fund?: bigint;
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
  loadProgram(programId: Address, elf: Uint8Array): Address;
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
  return {
    wallet(options: WalletOptions<Address> = {}): Fixture<Address, Host> {
      return {
        install: test => test.installWallet(options.address, options.fund),
      };
    },

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

    mint(options: MintOptions<Address> = {}): Fixture<Address, Host> {
      return {
        install: test =>
          test.installMint({
            authority: options.authority,
            freezeAuthority: options.freezeAuthority,
            supply: options.supply ?? 0n,
            decimals: options.decimals ?? 6,
            tokenProgram: options.tokenProgram ?? TokenProgram.Legacy,
            holders: options.holders ?? [],
          }),
      };
    },

    tokenAccount(
      mint: Address,
      owner: Address,
      options: TokenAccountOptions<Address> = {},
    ): Fixture<Address, Host> {
      return {
        install: test =>
          test.installTokenAccount(
            mint,
            owner,
            options.address,
            options.amount ?? 0n,
            options.tokenProgram ?? TokenProgram.Legacy,
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
        install: test => test.loadProgram(programId, elf),
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
