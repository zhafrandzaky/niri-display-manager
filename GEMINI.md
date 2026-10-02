# GEMINI.md — context anchor for Gemini agents

## What this repository is

`niri-display-manager` is a Rust GTK4/libadwaita desktop application that
switches display profiles on the niri Wayland compositor and performs HDMI
mirroring through wl-mirror. It is not a library; it ships a single binary, a
desktop entry, and an icon, and it manages a fenced block inside the user's
niri display configuration.

When you are asked to change something here, first read:

1. `docs/ARCHITECTURE.md` — layers, traits, data flows, safety model.
2. `AGENTS.md` — non-negotiable engineering rules (code style, tests, safety).
3. The module you are about to touch; the layout is
   `domain -> service -> infrastructure -> presentation`.

## Key facts to keep in context

- Target platform: CachyOS/Arch Linux, niri 26.04, GTK 4.22, libadwaita 1.9,
  wl-mirror 0.18.5, Rust 1.99 (edition 2024, MSRV 1.92).
- Hardware profile: Intel iGPU `card1` (panel `eDP-1`, `HDMI-A-1..4`) plus
  NVIDIA dGPU `card0` (`HDMI-A-5`).
- niri output names equal DRM connector names.
- IPC is driven through the `niri` CLI, never through a private socket:
  - `niri msg --json outputs` (map keyed by output name)
  - `niri msg --json windows`, `niri msg --json workspaces`
  - `niri msg action load-config-file`
  - `niri msg action move-window-to-monitor --id <id> <output>`
  - `niri msg action focus-window --id <id>`, `niri msg action focus-monitor <output>`
  - `niri msg output <output> on`
  - `niri validate -c <path>`
- Output JSON fields used: `name`, `modes[]`, `current_mode`, `logical`
  (`x`, `y`, `width`, `height`, `scale`), `vrr_supported`.
- Window JSON has no fullscreen flag; mirror verification checks placement via
  workspace -> output mapping instead.
- The managed KDL fence is delimited by `// >>> niri-display-manager: managed
  section` and `// <<< niri-display-manager: end of managed section`; a
  `// profile: <id>` line records the active profile.

## Conventions

- Zero emoji in UI strings and logs; ASCII only. Use Adwaita symbolic icons.
- No `unwrap`/`expect`/`panic`/`todo` in non-test code; `thiserror` for domain
  errors, `anyhow` only in `main.rs`.
- All external effects go through traits so tests can fake them.
- Keep files below 800 lines; test modules may live in submodule files.
- Every change must keep `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, and `cargo test` green.

## How to retrieve context efficiently

- Use `docs/ARCHITECTURE.md` for flow questions instead of reverse-engineering
  the whole tree.
- `tests/fixtures/niri_outputs_*.json` are real/synthetic niri payloads; reuse
  them for new parsing tests.
- `tests/common/mod.rs` contains the shared fakes; extend them rather than
  writing new one-off mocks.
- The live test suite (`cargo test --test live_environment -- --ignored`) is the
  fastest way to confirm real niri compatibility.

## Guardrails

- Never touch the user's real `~/.config/niri/**` files from tests or scripts.
  Live tests must stay read-only and use temporary directories.
- Never weaken the validate/verify/rollback pipeline.
- Never turn off all connected outputs; External Only must refuse when no
  external display is active.
- Do not add GPL-licensed dependencies to this MIT project.
