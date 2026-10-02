BINARY := niri-display-manager
PREFIX ?= $(HOME)/.local
CARGO ?= cargo

ICON_SVG := data/icons/hicolor/scalable/apps/niri-display-manager.svg
ICON_PNG := data/icons/hicolor/128x128/apps/niri-display-manager.png

.PHONY: all build release icons test fmt fmt-check clippy check coverage install uninstall clean

all: check

build:
	$(CARGO) build

release:
	$(CARGO) build --release

# Render the scalable icon into the committed 128x128 PNG asset.
#
# The installer renders the PNG into target/icon-staging at install time and
# falls back to the committed file when no renderer is available, so this
# target is mainly for refreshing the repository asset after icon changes.
icons:
	@if command -v rsvg-convert >/dev/null 2>&1; then \
		mkdir -p $(dir $(ICON_PNG)); \
		rsvg-convert -w 128 -h 128 -o $(ICON_PNG) $(ICON_SVG); \
	elif command -v magick >/dev/null 2>&1; then \
		mkdir -p $(dir $(ICON_PNG)); \
		magick -background none $(ICON_SVG) -resize 128x128 $(ICON_PNG); \
	elif command -v convert >/dev/null 2>&1; then \
		mkdir -p $(dir $(ICON_PNG)); \
		convert -background none $(ICON_SVG) -resize 128x128 $(ICON_PNG); \
	else \
		printf 'error: no SVG renderer found (install librsvg or imagemagick)\n' >&2; \
		exit 1; \
	fi
	@printf 'rendered %s\n' $(ICON_PNG)

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
