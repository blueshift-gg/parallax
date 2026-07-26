#!/usr/bin/env bash
# Parallax smoke driver: proves the harness works on this machine, both
# languages, no program artifact needed (the program-less test worlds).
# Optional: PARALLAX_PROGRAM_PATH=<some .so> extends to the parity suites.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

echo "== Rust: core + FFI suites =="
cargo test --manifest-path "$root/Cargo.toml" --quiet 2>&1 | grep -E "test result" | head -4

echo "== FFI kernel: release dylib for the TS shell =="
cargo build --release --quiet -p parallax-svm-ffi --manifest-path "$root/Cargo.toml"

echo "== TypeScript: build + fixture harness (program-less) =="
cd "$root/typescript"
npm run --silent build
PARALLAX_SVM_LIB="$root/target/release/libparallax_svm_ffi.dylib" npm test --silent 2>&1 | grep -E "Tests" | tail -1

if [ -n "${PARALLAX_PROGRAM_PATH:-}" ]; then
  echo "== Program parity (artifact provided) =="
  PARALLAX_SVM_LIB="$root/target/release/libparallax_svm_ffi.dylib" npm run --silent test:program 2>&1 | grep -E "Tests" | tail -1
fi
bash "$root/scripts/check-vocabulary.sh"
echo "SMOKE OK"
