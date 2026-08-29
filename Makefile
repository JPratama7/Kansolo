.PHONY: dev build build-ts install clean check test-ts test-rust bundle bump bump-version

PREFIX ?= $(HOME)/.local
DESTDIR ?=

# Tauri dev (launches desktop app + Vite via beforeDevCommand)
dev:
	deno task tauri dev

# Build Tauri desktop app, skip installer bundling
build:
	deno task tauri build --no-bundle

# Build frontend only (Vite)
build-ts:
	deno task build

# Install: copy release binary to ~/.local/bin
install: build
	install -D src-tauri/target/release/kansolo $(DESTDIR)$(PREFIX)/bin/kansolo

clean:
	rm -rf dist src-tauri/target

# Rust check (all targets)
check:
	cd src-tauri && cargo check --all-targets

# Rust tests
test-rust:
	cd src-tauri && cargo test

# TS tests
test-ts:
	deno test --allow-read --allow-env

# Build Tauri app with installer bundles
bundle:
	deno task tauri build

# Bump version in manifest files only (no cargo check, no commit).
# Used by CI release workflow. Usage: make bump-version VERSION=1.2.0
bump-version:
	@test -n "$(VERSION)" || { echo "usage: make bump-version VERSION=x.y.z"; exit 1; }
	@sed -i 's/"version": "[0-9.]\+"/"version": "$(VERSION)"/' src-tauri/tauri.conf.json
	@sed -i '0,/^version = "[0-9.]\+"/s//version = "$(VERSION)"/' src-tauri/Cargo.toml
	@sed -i 's/"version": "[0-9.]\+"/"version": "$(VERSION)"/' package.json

# Bump version across manifests, refresh Cargo.lock, commit, and tag.
# Local use. Usage: make bump VERSION=1.2.0
bump: bump-version
	@cd src-tauri && cargo check
	@git add src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock package.json
	@git commit -m "chore: release v$(VERSION)"
	@git tag v$(VERSION)
	@echo "tagged v$(VERSION). push: git push --tags"
