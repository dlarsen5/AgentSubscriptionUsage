PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN := target/release/usage

.PHONY: build check install uninstall clean

build:
	cargo build --release

check:
	cargo fmt --check
	cargo clippy --release -- -D warnings
	cargo test --release

install: build
	install -Dm755 $(BIN) $(BINDIR)/usage
	@echo "installed $(BINDIR)/usage"

uninstall:
	rm -f $(BINDIR)/usage

clean:
	cargo clean
