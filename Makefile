BINARY := niri-display-manager
PREFIX ?= $(HOME)/.local
CARGO ?= cargo

.PHONY: all build release test fmt fmt-check clippy check coverage install uninstall clean

all: check

build:
	$(CARGO) build

release:
	$(CARGO) build --release

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

check: fmt-check clippy test

coverage:
	$(CARGO) llvm-cov --summary-only

install: release
	PREFIX="$(PREFIX)" ./install.sh --skip-build

uninstall:
	PREFIX="$(PREFIX)" ./install.sh --uninstall

clean:
	$(CARGO) clean
