# parallax-svm-derive

The `#[parallax_test]` attribute macro for
[`parallax-svm`](https://github.com/blueshift-gg/parallax). This crate is an
implementation detail; depend on `parallax-svm` and use it through the prelude:

```rust,ignore
use parallax_svm::prelude::*;

#[parallax_test]
fn initializes(test: &mut Test) {
    let authority = test.add(Wallet::new());
    test.send(InitializeInstruction { authority }).succeeds();
}
```

`#[parallax_test]` expands to a plain `#[test]` that builds an isolated `Test`
world loaded with the current crate's compiled program. It uses `crate::ID` as
the program address by default; pass `#[parallax_test(program_id = EXPR)]` to
target another program.

Licensed under either of Apache-2.0 or MIT at your option.
