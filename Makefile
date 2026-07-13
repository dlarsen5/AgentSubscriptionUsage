PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN := target/release/agent_usage

.PHONY: build install uninstall clean

build:
	cargo build --release

install: build
	install -Dm755 $(BIN) $(BINDIR)/agent_usage
	@echo "installed $(BINDIR)/agent_usage"

uninstall:
	rm -f $(BINDIR)/agent_usage

clean:
	cargo clean
