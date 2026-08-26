.PHONY: dev build install clean

PREFIX ?= $(HOME)/.local
DESTDIR ?=

# Tauri dev (launches desktop app + Vite via beforeDevCommand)
dev:
	deno task tauri dev

# Build Tauri desktop app, skip installer bundling
build:
	deno task tauri build --no-bundle

# Install: copy release binary to ~/.local/bin
install: build
	install -D src-tauri/target/release/kansolo $(DESTDIR)$(PREFIX)/bin/kansolo

clean:
	rm -rf dist src-tauri/target
