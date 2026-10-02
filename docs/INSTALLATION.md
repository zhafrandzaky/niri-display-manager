# Installation

This guide targets CachyOS and Arch Linux with a niri Wayland session.

## 1. Prerequisites

Install the runtime and build dependencies:

```sh
sudo pacman -S --needed rustup gtk4 libadwaita wl-mirror jq librsvg base-devel
```

| Package | Why it is needed |
|---|---|
| `rustup` | Rust toolchain manager; the project needs Rust >= 1.92 (edition 2024) |
| `gtk4` | GTK 4 development files (headers, pkg-config metadata) |
| `libadwaita` | Libadwaita development files |
| `wl-mirror` | HDMI mirroring backend used by the Mirror / Presentation profile |
| `jq` | Optional; convenient for inspecting `niri msg --json` output manually |
| `librsvg` | Optional; renders the 128x128 PNG icon at install time (ImageMagick is an alternative) |
| `base-devel` | Linker and standard build tools |

Provision the Rust toolchain (only needed once per user):

```sh
rustup default stable
rustup component add rustfmt clippy
```

Verify the environment:

```sh
rustc --version                  # must be 1.92 or newer
pkg-config --modversion gtk4     # 4.20 or newer
pkg-config --modversion libadwaita-1
niri --version
wl-mirror --version
```

The repository also ships `rust-toolchain.toml`, so cargo uses the stable
channel automatically.

## 2. Build

```sh
cargo build --release
```

The binary is produced at `target/release/niri-display-manager`.

Run the quality gates before installing:

```sh
make check    # fmt-check + clippy (-D warnings) + tests
```

Live-session tests (read-only, safe to run inside niri):

```sh
cargo test --test live_environment -- --ignored
```

## 3. Install

```sh
make install
```

This builds the release binary and installs, by default into `~/.local`:

| File | Destination |
|---|---|
| Binary | `~/.local/bin/niri-display-manager` |
| Launcher entry | `~/.local/share/applications/niri-display-manager.desktop` |
| Scalable icon | `~/.local/share/icons/hicolor/scalable/apps/niri-display-manager.svg` |
| Raster icon | `~/.local/share/icons/hicolor/128x128/apps/niri-display-manager.png` |

The 128x128 PNG is rendered from the SVG at install time with `rsvg-convert`,
`magick`, or `convert` (first available); when no renderer is present, the
pre-rendered PNG committed in `data/icons` is used. `make icons` refreshes that
committed asset after icon changes.

The script refreshes the desktop database and the icon cache when the required
tools are available, so the entry appears in Rofi, Fuzzel, and application
menus immediately.

Options:

```sh
PREFIX=/usr/local make install          # custom prefix
./install.sh --skip-build              # install an existing release build
./install.sh --uninstall               # remove installed files
```

If `~/.local/bin` is not on your `PATH`, add it to your shell profile.

## 4. Manual installation

Without the script:

```sh
install -Dm755 target/release/niri-display-manager ~/.local/bin/niri-display-manager
install -Dm644 data/niri-display-manager.desktop ~/.local/share/applications/niri-display-manager.desktop
install -Dm644 data/icons/hicolor/scalable/apps/niri-display-manager.svg \
    ~/.local/share/icons/hicolor/scalable/apps/niri-display-manager.svg
install -Dm644 data/icons/hicolor/128x128/apps/niri-display-manager.png \
    ~/.local/share/icons/hicolor/128x128/apps/niri-display-manager.png
update-desktop-database ~/.local/share/applications 2>/dev/null || true
gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor 2>/dev/null || true
```

## 5. Verify

```sh
niri-display-manager
```

The window opens with the status row, five profile rows, the current outputs,
and the DRM port list. A read-only smoke test:

```sh
timeout 5 niri-display-manager   # the window should appear; timeout closes it
```

Verbose logs for diagnostics:

```sh
RUST_LOG=debug niri-display-manager
```

## 6. Uninstall

```sh
./install.sh --uninstall
```

The manager never deletes your configuration. It only manages a fenced section
inside `~/.config/niri/cfg/display.kdl` plus the one-time backup
`display.kdl.bak`, which you may remove manually once you are sure the pristine
copy is no longer needed.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `cargo: command not found` | Install `rustup` and run `rustup default stable` |
| `error: expected edition 2024` | Toolchain too old; `rustup update stable` |
| `pkg-config` cannot find `gtk4` | Install the `gtk4` package (development files) |
| `pkg-config` cannot find `libadwaita-1` | Install the `libadwaita` package |
| Build fails on `glib-sys` | Ensure `base-devel` and pkg-config are installed |
| App does not appear in the launcher | Re-run `update-desktop-database ~/.local/share/applications` |
| Icon missing in menus | Re-run `gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor`; both the scalable SVG and the 128x128 PNG are installed |
| Mirror profile reports wl-mirror missing | `sudo pacman -S wl-mirror` |
| HDMI output on the NVIDIA card is not detected | Ensure `nvidia_drm.modeset=1` is set in your kernel parameters and reboot |
| `niri validate` reports the generated file | Open an issue with the content of `display.kdl`; the manager rolls back automatically and never leaves an invalid file active |
