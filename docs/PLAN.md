# niri-display-manager — Approved Implementation Plan

Status: approved and under implementation.
Scope: production-grade GTK4/libadwaita display and mirroring manager for the niri
compositor on CachyOS/Arch Linux, targeting hybrid-GPU laptops (Intel iGPU + NVIDIA
dGPU) with HDMI presentation workflows.

This document records the design approved for implementation. It is the
authoritative plan for the repository. The architecture summary lives in
`docs/ARCHITECTURE.md`; this file tracks intent, decisions, and progress.

---

## 1. Verified environment (design input)

| Item | Finding |
|---|---|
| Niri | `26.04` (CLI and compositor versions matched), live session accessible |
| Niri IPC | `output <NAME> off/on/mode/position/scale` (temporary), `move-window-to-monitor <OUTPUT> --id`, `focus-monitor <OUTPUT>`, `load-config-file`, `validate -c FILE`, `--json outputs/windows/workspaces` |
| wl-mirror | `0.18.5`, supports `--fullscreen-output O` |
| GTK stack | `gtk4 4.22.5`, `libadwaita 1.9.4` (development packages present) |
| Rust | `1.99.0` stable, clippy and rustfmt installed; edition 2024 supported |
| GPUs | Intel iGPU `card1` (`eDP-1` connected; `HDMI-A-1..4`), NVIDIA dGPU `card0` (`HDMI-A-5`) |
| HDMI today | All HDMI ports disconnected; cable-dependent paths verified by mocks + manual checklist |
| Config wiring | `~/.config/niri/config.kdl` includes `./cfg/display.kdl` |
| Icons | Adwaita symbolic icons verified: `video-single-display-symbolic`, `video-joined-displays-symbolic`, `display-projector-symbolic`, `video-display-symbolic`, `object-select-symbolic`, `view-refresh-symbolic`, `dialog-warning-symbolic`, `media-playback-stop-symbolic` |
| gtk-rs reality | `glib::MainContext::channel()` was removed from current glib; the modern equivalent is `async-channel` + `glib::spawn_future_local()` |

## 2. Approach decision

Profile application is **file-backed with IPC reload and verification**:

1. `~/.config/niri/cfg/display.kdl` is the single source of truth and survives
   reloads and reboots.
2. Every apply triggers `niri msg action load-config-file` and then verifies the
   real output state, with automatic rollback on mismatch.

Alternatives rejected:

- IPC-temporary only: changes are forgotten on any config edit or restart and
  cannot express mirroring.
- File-only with niri's file watcher: no deterministic verification point and
  no rollback trigger.

Mirroring spawns `wl-mirror <source> --fullscreen-output <target>` (the client
requests fullscreen on the target output), verifies the window via
`niri msg --json windows/workspaces`, and falls back to IPC actions
(`move-window-to-monitor --id`, `focus-window --id`, `fullscreen-window`) when
the window is not where it should be.

## 3. Architecture

```
src/
├── main.rs                  # anyhow boundary; logging init; presentation::run()
├── lib.rs                   # module root, lint policy, APP_ID
├── domain/
│   ├── display.rs           # OutputId, DisplayOutput, LogicalGeometry, ConnectorKind, ConnectorStatus
│   └── profile.rs           # ProfileKind(5), LayoutPlan/OutputPlan/MirrorPlan, pure position math
├── service/
│   ├── display_service.rs   # apply/reset/stop_mirror orchestration + rollback + verification
│   └── backup_service.rs    # ConfigStore trait + FileConfigStore (snapshot, atomic write, restore)
├── infrastructure/
│   ├── niri_ipc.rs          # NiriClient trait + NiriCliClient (niri msg --json/action)
│   ├── kdl_parser.rs        # managed-section render/splice/remove; include registration check
│   ├── drm_sysfs.rs         # HardwareProbe trait + SysfsProbe (/sys/class/drm connectors)
│   └── process_runner.rs    # CommandRunner + ProcessSupervisor + MirrorPidFile
└── presentation/
    ├── view_model.rs        # AppState/AppAction/AppEvent, pure row and text builders
    └── views/main_window.rs # AdwApplicationWindow, profile rows, outputs, toasts, worker thread
tests/
├── common/mod.rs            # shared fakes (NiriClient, HardwareProbe, ProcessSupervisor)
├── fixtures/                # captured/synthetic niri JSON, sysfs layouts
├── service_flow.rs          # integration: apply/rollback/mirror flows
├── kdl_roundtrip.rs         # integration: managed section splice/remove preservation
└── live_environment.rs      # #[ignore] live test: real niri validate, outputs, sysfs
data/
├── niri-display-manager.desktop
└── icons/hicolor/scalable/apps/niri-display-manager.svg
docs/                        # ARCHITECTURE.md, INSTALLATION.md, USAGE.md, PLAN.md
AGENTS.md, GEMINI.md, CLAUDE.md, README.md, Makefile, install.sh
```

Dependency rule: `presentation -> service -> domain`; `service` depends on traits
only; `infrastructure` implements traits; `domain` performs no I/O.

Traits: `NiriClient`, `HardwareProbe`, `ConfigStore`, `ProcessSupervisor`,
`CommandRunner` — all mockable for tests.

## 4. Core flows

### Apply profile (example: Extend Right)

1. Probe `niri msg --json outputs` and `/sys/class/drm/*/status`.
2. Preconditions: Extend persists and warns when the cable is absent; External
   Only and Mirror refuse without a connected external display (safety: never
   turn off the only display).
3. If leaving mirror mode, terminate the wl-mirror child (SIGTERM, 2 s grace,
   SIGKILL fallback) and clear the pid file.
4. Ensure the one-time pristine snapshot `display.kdl.bak` exists.
5. Render the managed section and splice it into `display.kdl`; write atomically
   (temp file + fsync + rename, original permissions preserved).
6. Gate with `niri validate -c ~/.config/niri/config.kdl`; on failure restore the
   snapshot, reload, and report a structured error.
7. `niri msg action load-config-file`, then poll outputs up to 3 s for the
   expected layout; on mismatch restore, reload, and report.
8. External Only focuses the external output (`focus-monitor`).
9. Return an `ApplyReport` (profile, warnings) to the UI.

### Internal Display Only (reset)

Stop mirror, restore `display.kdl.bak` when present (otherwise remove the
managed section), reload, re-enable the internal output, and verify. Fully
idempotent: `RestoreOutcome::{RestoredFromBackup, RemovedManagedSection, NoChange}`.

### Mirror start / stop

Start: apply the mirror layout (both outputs on), spawn `wl-mirror` with
`--fullscreen-output`, write the pid file, verify the window on the target
(IPC fallback move/focus/fullscreen when needed). Stop: SIGTERM with grace, kill
fallback, remove the pid file. Startup cleans up an orphaned wl-mirror recorded
in the pid file.

### Managed KDL section

```kdl
// >>> niri-display-manager: managed section - manual edits will be overwritten
// profile: extend-right
output "eDP-1" {
    position x=0 y=0
}
output "HDMI-A-1" {
    position x=1920 y=0
}
// <<< niri-display-manager: end of managed section
```

User content outside the markers is never parsed or rewritten.

## 5. Key technical decisions

1. Async state flow: `async-channel` + `glib::spawn_future_local()` replaces the
   removed `MainContext::channel`; all I/O runs on a worker thread, the UI
   thread never blocks.
2. No user-KDL rewriting: only the fenced section is spliced; `niri validate`
   is the authority before any reload.
3. Fail-safe ordering: backup -> write -> validate -> reload -> verify ->
   rollback, fully automatic.
4. Safety/lints: `#![forbid(unsafe_code)]`;
   `clippy::unwrap_used/expect_used/panic/todo/unimplemented` denied in
   non-test code. `thiserror` per layer; `anyhow` only in `main.rs`.
5. Zero emoji: UI labels, tooltips, status, and log messages are ASCII
   technical text; a unit test asserts view-model strings are ASCII.
6. Crates: `gtk4 0.11 (v4_22)`, `libadwaita 0.9 (v1_9, gtk_v4_22)`,
   `glib (futures)`, `serde`/`serde_json`, `thiserror 2`, `anyhow`,
   `async-channel 2`, `nix (signal)`, `log`/`env_logger`; dev: `tempfile`.
   Edition 2024, `rust-version = 1.92`.
7. Config path: `$XDG_CONFIG_HOME/niri/cfg/display.kdl` (default
   `~/.config/niri/cfg/display.kdl`), environment override for tests.
8. Orphan safety: the mirror child pid is tracked in
   `$XDG_RUNTIME_DIR`; startup cleans a stale wl-mirror we spawned.
9. App ID: `io.github.zyy.NiriDisplayManager` (single constant, easily changed).
10. License: MIT (recommended default; changeable on request).

## 6. UI design

`AdwApplicationWindow` -> `AdwToolbarView` + `AdwHeaderBar` (title "Niri Display
Manager", refresh button) -> `AdwPreferencesPage`:

- Display Profiles group: five `AdwActionRow`s (Internal Display Only, Extend
  Right, Extend Left, Mirror / Presentation, External Only), symbolic icon,
  technical subtitle, active marker via `object-select-symbolic`.
- Outputs group: one row per niri output (resolution, refresh, position, scale).
- Hardware Ports group: sysfs connectors with Connected/Disconnected state.
- Mirror group (while active): source -> target and a Stop Mirroring button.
- `AdwToastOverlay` for results and errors; busy state disables rows and shows
  a spinner.

## 7. Testing and QA

| Level | Coverage |
|---|---|
| Unit (domain) | connector classification, external selection, scale-aware position math, all five plans, mirror argv, ASCII string test |
| Unit (infrastructure) | KDL splice idempotency and preservation; sysfs fixture parsing; niri JSON fixtures (real, dual-output, external-off); command timeouts; supervisor SIGTERM on a real child |
| Unit (service) | with fakes: happy paths, validation failure rollback, mirror start/stop, reset idempotency |
| Integration | full flows with shared fakes plus temp directories |
| Live-gated | real `niri validate -c` on generated configs, real outputs parse, real sysfs probe |
| Manual | GUI launch in the live session; HDMI Extend/Mirror checklist when a monitor is attached |

Commands: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`, `cargo build --release`. Coverage target: 80 percent on
domain/service/infrastructure (presentation covered manually).

## 8. Deliverables and execution phases

1. **Phase 0** — bootstrap: `cargo init`, `Cargo.toml`, `rust-toolchain.toml`,
   `.gitignore`, lint skeleton, first commit.
2. **Phase 1** — domain layer and unit tests.
3. **Phase 2** — infrastructure layer, fixtures, and unit tests.
4. **Phase 3** — service layer and unit/integration tests.
5. **Phase 4** — presentation layer (view model, window, worker threading).
6. **Phase 5** — data assets (desktop entry, icon), `Makefile`, `install.sh`.
7. **Phase 6** — documentation: `README.md`, `docs/ARCHITECTURE.md`,
   `docs/INSTALLATION.md`, `docs/USAGE.md`, `AGENTS.md`, `GEMINI.md`, `CLAUDE.md`.
8. **Phase 7** — QA pass: fmt, clippy, tests, live verification of available
   paths, release build, coverage report, final commit.

## 9. Implementation status

- [x] Phase 0 — bootstrap
- [x] Phase 1 — domain
- [x] Phase 2 — infrastructure
- [x] Phase 3 — services
- [x] Phase 4 — presentation
- [x] Phase 5 — data assets and scripts
- [x] Phase 6 — documentation
- [x] Phase 7 — QA and release build

Verification record (final run):

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` are green.
- Test suite: 63 unit tests, 8 integration tests, and 3 live-session tests
  (read-only, run with `cargo test --test live_environment -- --ignored`).
- Coverage (`cargo llvm-cov`, `main.rs` and the presentation layer excluded):
  90.86 percent lines and 91.45 percent regions, with every measured file at or
  above 80 percent lines.
- Live checks: real `niri msg --json` outputs and windows parsed, real DRM sysfs
  probed, generated configurations accepted by the real `niri validate`, and
  the GUI window confirmed on the running compositor as
  `io.github.zyy.NiriDisplayManager`.
- Installer verified with a temporary prefix, including desktop entry
  validation and clean uninstall.
