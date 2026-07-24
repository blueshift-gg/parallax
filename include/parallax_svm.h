#ifndef PARALLAX_SVM_H
#define PARALLAX_SVM_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

/**
 * The call succeeded.
 */
#define PARALLAX_OK 0

/**
 * A required pointer argument was null.
 */
#define PARALLAX_ERR_NULL_POINTER -1

/**
 * A wire buffer could not be decoded (malformed or truncated).
 */
#define PARALLAX_ERR_INVALID_WIRE -2

/**
 * A program artifact could not be loaded as a valid SBF ELF.
 */
#define PARALLAX_ERR_PROGRAM_LOAD -3

/**
 * Transaction execution set-up failed (before the runtime ran).
 */
#define PARALLAX_ERR_EXECUTION -4

/**
 * A Rust panic was caught at the boundary.
 */
#define PARALLAX_ERR_INTERNAL -99

/**
 * Wire tag for [`ProgramError::Custom`]; its code rides in `custom_code`.
 */
#define ERROR_TAG_CUSTOM 18

/**
 * Wire tag for [`ProgramError::Runtime`]; its message rides in `runtime_msg`.
 */
#define ERROR_TAG_RUNTIME 19

/**
 * Borrowed pointer to the calling thread's last-error C string, or null when
 * the last call succeeded. Valid until the next FFI call on this thread.
 */
const char *parallax_last_error(void);

/**
 * Build a world for `program_id` from in-memory ELF `elf`, optionally pinning a
 * transaction compute-unit limit. Pass `elf_len == 0` (with a null or empty
 * `elf`) to build a world with no primary program — only the runtime's
 * built-ins (e.g. the SPL programs). Returns an opaque handle, or null on
 * failure (`parallax_last_error` describes it). Free with [`parallax_free`].
 */
Test *parallax_new(const uint8_t (*program_id)[32],
                   const uint8_t *elf,
                   uint64_t elf_len,
                   bool has_compute_unit_limit,
                   uint64_t compute_unit_limit);

/**
 * Free a handle returned by [`parallax_new`]. Safe on null.
 */
void parallax_free(Test *handle);

/**
 * Free a result buffer previously returned through a `result_out`/`result_len`
 * pair. Pass the exact pointer and length the call produced. Safe on null.
 */
void parallax_free_bytes(uint8_t *ptr, uint64_t len);

/**
 * Reconfigure the transaction compute-unit limit on an existing world. Backs
 * the TypeScript runtime's `setComputeBudget`.
 */
int32_t parallax_set_compute_unit_limit(Test *handle, uint64_t limit);

/**
 * Preload a program (by id and ELF) for cross-program invocations. Also serves
 * as the program fixture install: the resulting address is the caller's `id`.
 */
int32_t parallax_load_program(Test *handle,
                              const uint8_t (*program_id)[32],
                              const uint8_t *elf,
                              uint64_t elf_len);

/**
 * Set the runtime clock's Unix timestamp.
 */
int32_t parallax_warp_to_timestamp(Test *handle, int64_t timestamp);

/**
 * Install a funded system wallet from a `WalletInput` option-bag. Writes the
 * resulting 32-byte address into `address_out`.
 */
int32_t parallax_install_wallet(Test *handle,
                                const uint8_t *input,
                                uint64_t input_len,
                                uint8_t *address_out);

/**
 * Install a token account from a `TokenAccountInput` option-bag. Writes the
 * resulting 32-byte address into `address_out`.
 */
int32_t parallax_install_token_account(Test *handle,
                                       const uint8_t *input,
                                       uint64_t input_len,
                                       uint8_t *address_out);

/**
 * Install an associated-token account from an `AtaInput` option-bag. Writes the
 * derived 32-byte address into `address_out`.
 */
int32_t parallax_install_ata(Test *handle,
                             const uint8_t *input,
                             uint64_t input_len,
                             uint8_t *address_out);

/**
 * Install a raw account from a `RawAccountInput` option-bag. Writes the
 * resulting 32-byte address into `address_out`.
 */
int32_t parallax_install_raw_account(Test *handle,
                                     const uint8_t *input,
                                     uint64_t input_len,
                                     uint8_t *address_out);

/**
 * Install a mint (and one associated-token account per holder) from a
 * `MintInput` option-bag. Returns a count-prefixed pubkey list through
 * `result_out`/`result_len_out`: index 0 is the mint, indices 1.. are holder
 * ATAs in holder order. Free the buffer with [`parallax_free_bytes`].
 */
int32_t parallax_install_mint(Test *handle,
                              const uint8_t *input,
                              uint64_t input_len,
                              uint8_t **result_out,
                              uint64_t *result_len_out);

/**
 * Read a raw account. Returns an `option<account>` blob through
 * `result_out`/`result_len_out` (a leading `0` byte means no account). Free the
 * buffer with [`parallax_free_bytes`].
 */
int32_t parallax_get_account(Test *handle,
                             const uint8_t (*address)[32],
                             uint8_t **result_out,
                             uint64_t *result_len_out);

/**
 * Execute and commit an instruction chain, returning an outcome bundle through
 * `result_out`/`result_len_out`. `accounts` is a count-prefixed list of
 * explicit input accounts (pass a null pointer with length 0 for none). Free
 * the bundle with [`parallax_free_bytes`].
 */
int32_t parallax_send(Test *handle,
                      const uint8_t *instructions,
                      uint64_t instructions_len,
                      const uint8_t *accounts,
                      uint64_t accounts_len,
                      uint8_t **result_out,
                      uint64_t *result_len_out);

/**
 * Execute an instruction chain without committing, returning an outcome bundle.
 * Arguments match [`parallax_send`].
 */
int32_t parallax_simulate(Test *handle,
                          const uint8_t *instructions,
                          uint64_t instructions_len,
                          const uint8_t *accounts,
                          uint64_t accounts_len,
                          uint8_t **result_out,
                          uint64_t *result_len_out);

#endif  /* PARALLAX_SVM_H */
