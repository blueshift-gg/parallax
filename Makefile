# Native-binary build tooling for the `parallax-svm-ffi` transport.
#
# `build-all` cross-compiles the cdylib for every supported platform and copies
# each artifact into its `npm/<platform>` package directory under the canonical
# per-OS filename the TypeScript loader resolves. The two macOS targets build
# natively; the Linux and Windows targets use `cargo zigbuild` (requires `zig`
# and `cargo-zigbuild`).

PLATFORMS := darwin-arm64 darwin-x64 linux-x64-gnu linux-arm64-gnu win32-x64-msvc

CRATE := parallax-svm-ffi

.PHONY: build build-all clean copy-local prepublish publish version

# Build the release cdylib for the host platform only.
build:
	cargo build --release -p $(CRATE)

# Build the native library for every platform and stage it into npm/.
build-all:
	cargo build --release -p $(CRATE) --target aarch64-apple-darwin
	cargo build --release -p $(CRATE) --target x86_64-apple-darwin
	cargo zigbuild --release -p $(CRATE) --target x86_64-unknown-linux-gnu
	cargo zigbuild --release -p $(CRATE) --target aarch64-unknown-linux-gnu
	cargo zigbuild --release -p $(CRATE) --target x86_64-pc-windows-gnu
	cp target/aarch64-apple-darwin/release/libparallax_svm_ffi.dylib  npm/darwin-arm64/libparallax_svm_ffi.dylib
	cp target/x86_64-apple-darwin/release/libparallax_svm_ffi.dylib   npm/darwin-x64/libparallax_svm_ffi.dylib
	cp target/x86_64-unknown-linux-gnu/release/libparallax_svm_ffi.so npm/linux-x64-gnu/libparallax_svm_ffi.so
	cp target/aarch64-unknown-linux-gnu/release/libparallax_svm_ffi.so npm/linux-arm64-gnu/libparallax_svm_ffi.so
	cp target/x86_64-pc-windows-gnu/release/parallax_svm_ffi.dll      npm/win32-x64-msvc/parallax_svm_ffi.dll
	@echo "All platform binaries built and copied into npm/."

# Copy the host release build into its platform package dir (local dev).
copy-local:
ifeq ($(shell uname -s),Darwin)
ifeq ($(shell uname -m),arm64)
	cp target/release/libparallax_svm_ffi.dylib npm/darwin-arm64/
else
	cp target/release/libparallax_svm_ffi.dylib npm/darwin-x64/
endif
else
ifeq ($(shell uname -m),aarch64)
	cp target/release/libparallax_svm_ffi.so npm/linux-arm64-gnu/
else
	cp target/release/libparallax_svm_ffi.so npm/linux-x64-gnu/
endif
endif

clean:
	cargo clean

# Warn if any platform package is missing its binary before publishing.
prepublish: build-all
	@for plat in $(PLATFORMS); do \
		count=$$(ls npm/$$plat/*.dylib npm/$$plat/*.so npm/$$plat/*.dll 2>/dev/null | wc -l); \
		if [ $$count -eq 0 ]; then \
			echo "WARNING: no binary in npm/$$plat/"; \
		else \
			echo "OK: npm/$$plat/"; \
		fi \
	done

# Publish every platform package (binaries must already be staged).
publish: prepublish
	@for plat in $(PLATFORMS); do \
		echo "Publishing parallax-svm-$$plat..."; \
		cd npm/$$plat && npm publish --access public && cd ../..; \
	done

# Bump the version in every npm/<platform>/package.json at once.
# Usage: make version V=0.2.0
version:
ifndef V
	$(error V is required, e.g. make version V=0.2.0)
endif
	node -e "\
		const fs = require('fs'); \
		for (const d of fs.readdirSync('npm')) { \
			const f = 'npm/' + d + '/package.json'; \
			const pkg = JSON.parse(fs.readFileSync(f, 'utf8')); \
			pkg.version = '$(V)'; \
			fs.writeFileSync(f, JSON.stringify(pkg, null, 2) + '\n'); \
			console.log('Updated', f, '->', '$(V)'); \
		}"
