/**
 * Well-known program addresses, as base58 strings. The harness owns the account
 * layouts (the Rust kernel builds every mint, token, and ATA fixture), so these
 * are only used adapter-side for naming: associated-token address derivation and
 * the system-program identity behind `isClosed`.
 */
export const SPL_TOKEN_PROGRAM_ID = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
export const SPL_TOKEN_2022_PROGRAM_ID = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
export const SPL_ASSOCIATED_TOKEN_PROGRAM_ID =
  "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
export const SYSTEM_PROGRAM_ID = "11111111111111111111111111111111";
