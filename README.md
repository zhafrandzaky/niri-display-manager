# niri-display-manager

A production-grade GTK4/libadwaita display manager for the [niri](https://github.com/YaLTeR/niri)
Wayland compositor. It switches between laptop, extended, mirrored, and
projector-only display setups, persists them to the niri configuration, and
rolls back automatically if the compositor rejects a change.

The project targets CachyOS/Arch Linux on hybrid-GPU laptops (tested on an
Intel iGPU + NVIDIA dGPU machine) where the internal panel is `eDP-1` and the
HDMI ports are split across `card0` and `card1`.

## Features

| Feature | Description |
|---|---|
| Five display profiles | Internal only, Extend right, Extend left, Mirror / Presentation, External only |
| Persisted layouts | Profiles are written to `~/.config/niri/cfg/display.kdl` and survive restarts |
| Instant application | Each apply reloads niri through `niri msg action load-config-file` |
| Verified apply | Output state is polled after reload; failures trigger an automatic rollback |
| Pristine backup | `display.kdl.bak` is created once before the first change and restores idempotently |
| Validate before load | Generated configuration passes `niri validate` before niri ever sees it |
| HDMI mirroring | `wl-mirror` is spawned with `--fullscreen-output`, supervised, and stopped with SIGTERM |
| Multi-GPU aware | Reads `/sys/class/drm` so a cable on either GPU is detected |
| Fail-safe external-only | The internal panel is only turned off when an external display is active |
| Zero-emoji UI | Technical ASCII labels and freedesktop symbolic icons only |
| Launcher entry | Ships a `.desktop` file plus scalable SVG and 128x128 PNG icons for Rofi, Fuzzel, and application menus |

## System requirements

| Component | Requirement |
|---|---|
| Linux | Wayland session running niri (tested with niri 26.04) |
| GTK | `gtk4` >= 4.20 (tested with 4.22.5) |
| libadwaita | >= 1.6 (tested with 1.9.4) |
| Rust | >= 1.92 (edition 2024; tested with 1.99.0) |
| wl-mirror | Required only for the Mirror / Presentation profile (tested with 0.18.5) |
| jq | Optional, useful for manual `niri msg --json` inspection |

## Quick start

```sh
# Dependencies (CachyOS / Arch Linux)
sudo pacman -S --needed rustup gtk4 libadwaita wl-mirror jq librsvg base-devel

# Build and install for the current user (~/.local by default)
make install

# Run
niri-display-manager
```

See `docs/INSTALLATION.md` for a complete installation guide, `docs/USAGE.md`
for operating instructions and diagnostics, and `docs/ARCHITECTURE.md` for the
internal design.

## How it works

Profile application follows a fail-safe pipeline:

```text
probe niri + DRM sysfs
        |
        v
plan layout (pure domain logic)
        |
        v
stop mirror child -> snapshot config -> write managed section
        |
        v
niri validate -- gate
        |
        v
niri msg action load-config-file -> verify observed layout
        |
        v
success, or automatic rollback to the pristine snapshot
```

The manager never rewrites user configuration. It owns a fenced section inside
`~/.config/niri/cfg/display.kdl`:

```kdl
// >>> niri-display-manager: managed section - manual edits will be overwritten
// profile: extend-right
output "eDP-1" {
    position x=0 y=0
}
output "HDMI-A-1" {
    position x=1920 y=0
    scale 1
}
// <<< niri-display-manager: end of managed section
```

Everything outside the fence is preserved byte for byte.

## Project layout

```text
src/
├── main.rs                  entry point and logging
├── lib.rs                   module root and lint policy
├── domain/                  pure entities and layout rules (no I/O)
├── service/                 application use cases and rollback pipeline
├── infrastructure/          niri IPC, DRM sysfs, KDL fence, process supervision
└── presentation/            GTK4 / libadwaita UI and worker wiring
data/                        desktop entry, scalable SVG, and 128x128 PNG icon
tests/                       integration and live-environment tests
docs/                        architecture, installation, usage, plan
```

## Development

```sh
make fmt-check      # rustfmt check
make clippy         # clippy with -D warnings
make test           # unit and integration tests
make check          # all of the above
cargo test --test live_environment -- --ignored   # live niri session tests
```

The live tests are read-only: they parse real niri output, probe real DRM
sysfs, and validate generated configurations in a temporary directory.

## Safety model

- The pristine snapshot is created once and is never overwritten by the manager.
- Every write is atomic (temp file, fsync, rename) and preserves file permissions.
- Generated KDL is validated by niri itself before any reload.
- Post-reload verification compares real output geometry against the plan.
- Any failure restores the snapshot and reloads the previous configuration.
- The mirror helper is tracked by pid file; stale wl-mirror processes from a
  crashed session are cleaned up on the next start.
- The crate forbids `unsafe` code and denies `unwrap`, `expect`, `panic`,
  `todo`, and `unimplemented` outside tests.

## Limitations

- Only niri is supported; there is no X11 or other-compositor backend.
- Mirroring requires an external display already active in niri. If the cable
  is absent, Extend profiles still persist a layout that applies on hotplug,
  but Mirror and External Only are refused to protect the panel-only setup.
- Positions are managed globally: applying a profile may override manual output
  positions, which is the intended contract of the managed section.

## License

MIT. See `LICENSE`.
