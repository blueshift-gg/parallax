//! Generate the C header for the FFI surface.
//!
//! The header is a convenience for C/koffi consumers; the authoritative wire
//! contract is documented in `src/wire.rs`. Generation is best-effort: a
//! cbindgen hiccup emits a warning rather than failing the build, so the
//! `cargo build`/`test`/`doc` gates never hinge on header regeneration.

fn main() {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let include_dir = format!("{crate_dir}/../include");
    let header = format!("{include_dir}/parallax_svm.h");

    if let Err(error) = std::fs::create_dir_all(&include_dir) {
        println!("cargo:warning=parallax-svm-ffi: could not create {include_dir}: {error}");
        return;
    }

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_language(cbindgen::Language::C)
        .with_include_guard("PARALLAX_SVM_H")
        .with_no_includes()
        .with_sys_include("stdint.h")
        .with_sys_include("stdbool.h")
        .with_sys_include("stddef.h")
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&header);
        }
        Err(error) => {
            println!(
                "cargo:warning=parallax-svm-ffi: cbindgen could not generate {header}: {error}"
            );
        }
    }
}
