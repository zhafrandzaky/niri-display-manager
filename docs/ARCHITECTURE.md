# Architecture

This document describes the structure of niri-display-manager: layers, trait
contracts, data flows, the fail-safe rollback model, and the multi-GPU
handling that the manager relies on.

## Goals and constraints

- Manage display profiles for a niri Wayland session without owning the user's
  configuration file.
- Never leave the compositor in a broken or black-screen state.
- Keep all external interaction (IPC, sysfs, files, processes) behind traits so
  the whole application is testable without a live session.
- Keep the UI layer free of blocking I/O.
- Zero `unsafe`, zero panics in library code, zero emoji in user-visible text.

## Layer map

```text
+---------------------------+        +--------------------------------------+
| presentation              |        | infrastructure                       |
|  view_model, main_window  |        |  niri_ipc   drm_sysfs                |
|  GTK4 / libadwaita        |        |  kdl_parser process_runner           |
+-------------+-------------+        +------------------+-------------------+
              |  AppAction / AppEvent                    | implements
              v                                          v
+-------------+-------------+        +------------------+-------------------+
| service                   |        | traits                               |
|  display_service          |<------>|  NiriClient  HardwareProbe           |
|  backup_service           |        |  ConfigStore CommandRunner           |
|  apply / verify / rollback|        |  ProcessSupervisor                   |
+-------------+-------------+        +--------------------------------------+
              |  pure calls
              v
+-------------+-------------+
| domain                    |
|  display, profile         |
|  entities, planning rules |
+---------------------------+
```

Dependency rule: `presentation -> service -> domain`, services depend on
traits only, and infrastructure implements those traits. The domain layer has
no knowledge of niri, GTK, sysfs, or the filesystem.

## Module responsibilities

| Module | Responsibility |
|---|---|
| `domain::display` | `DisplayOutput`, `DisplayMode`, `LogicalGeometry`, `ConnectorStatus`, connector classification (`eDP`/`LVDS`/`DSI` are internal) |
| `domain::profile` | `ProfileKind`, `LayoutPlan`, `MirrorPlan`, pure position math, plan verification, planning errors |
| `service::display_service` | Apply pipeline, verification polling, rollback, mirror start/stop, orphan cleanup, `SystemState` for the UI |
| `service::backup_service` | `ConfigStore` trait and `FileConfigStore`: snapshot-once, atomic writes, restore, include detection |
| `infrastructure::niri_ipc` | `NiriClient` trait and `NiriCliClient` over `niri msg --json` / `niri msg action` |
| `infrastructure::drm_sysfs` | `HardwareProbe` trait and `SysfsProbe` reading `/sys/class/drm/*/status` |
| `infrastructure::kdl_parser` | Managed-section render/splice/remove, profile comment parsing, include check |
| `infrastructure::process_runner` | `CommandRunner` with timeouts, `ProcessSupervisor` with SIGTERM/SIGKILL, `MirrorPidFile` |
| `presentation::view_model` | `AppState`, `AppAction`, `AppEvent`, pure row and status-text builders |
| `presentation::views::main_window` | Window construction, widget rebuilds, worker thread, `glib::spawn_future_local` event loop |

## Trait contracts

| Trait | Operations | Production adapter |
|---|---|---|
| `NiriClient` | `outputs`, `windows`, `workspaces`, `load_config`, `move_window_to_output`, `focus_window`, `focus_output`, `enable_output`, `validate_config` | `NiriCliClient` (`niri` binary) |
| `HardwareProbe` | `connectors` | `SysfsProbe` (`/sys/class/drm`) |
| `ConfigStore` | `read_display_config`, `snapshot`, `write_managed_section`, `restore_snapshot`, path accessors, `is_registered_in_main_config` | `FileConfigStore` |
| `CommandRunner` | `run` with timeout | `SystemCommandRunner` |
| `ProcessSupervisor` | `spawn`, `is_running`, `terminate`, `terminate_pid` | `SystemSupervisor` |

## Apply pipeline

`DisplayService::apply_profile(profile)` executes the following steps:

1. Probe `niri msg --json outputs` and `/sys/class/drm` connectors.
2. Plan the layout with pure domain logic. Refusals happen here:
   - Mirror without an enabled external display is rejected.
   - External only without a connected external display is rejected, so the
     internal panel can never be the only disabled output.
   - Extend without a cable is allowed: the block is persisted with
     `expect_present = false`, verification skips the absent output, and a
     warning is reported.
3. Stop any supervised mirror child (SIGTERM with a two-second grace, then
   SIGKILL) and remove the pid file.
4. Create the pristine `display.kdl.bak` snapshot if it does not exist yet.
5. Internal only: restore the snapshot (or remove the fence, or no-op) and
   re-enable the internal output if needed. Otherwise: render the managed KDL
   section and splice it into `display.kdl`.
6. Gate the new file through `niri validate -c <config.kdl>`.
7. Reload with `niri msg action load-config-file`.
8. Poll outputs (default 50 attempts x 100 ms, 5 s) and compare against the plan
   with `verify_plan`. Mismatches describe the exact output and coordinates.
9. Apply focus (`focus-monitor`), and for Mirror spawn `wl-mirror` with
   `--fullscreen-output <target> <source>`, record the pid, then verify the
   mirror window is on the target output through `windows` + `workspaces`.

Any failure in steps 6-9 triggers `rollback()`: restore the snapshot, reload,
and log secondary failures without masking the original error.

## Mirror lifecycle

```text
apply Mirror
   |
   +-- ensure both outputs are enabled and positioned (layout plan)
   +-- spawn wl-mirror --fullscreen-output <target> <source>
   +-- write pid to $XDG_RUNTIME_DIR/niri-display-manager.mirror.pid
   +-- poll niri windows/workspaces until the wl-mirror window is on <target>
   |      the window is matched by child pid, then by the app id
   |      `at.yrlf.wl_mirror` (or `wl-mirror` on older releases)
   |      fallback placement: move-window-to-monitor --id + focus-window --id
   +-- on failure: terminate child, remove pid file, rollback layout

stop / switch away / reset
   +-- SIGTERM (2 s grace) -> SIGKILL fallback -> reap -> remove pid file

startup
   +-- if a pid file exists and /proc/<pid>/comm is wl-mirror: SIGTERM it
```

niri's `Window` structure (niri-ipc 26.4) does not expose a fullscreen flag, so
the manager does not blindly toggle fullscreen. It relies on wl-mirror's own
`--fullscreen-output` request and verifies placement instead, which avoids
accidentally unfullscreening an already correct window. wl-mirror output is
captured to `$XDG_RUNTIME_DIR/niri-display-manager-mirror.log`, and failure
messages reference that file.

## Managed KDL section

The manager appends or replaces exactly one fenced section. The fence markers
are stable constants; the profile id comment is parsed back to display the
active profile on startup. User content outside the fence is never parsed or
rewritten.

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

Properties used: `off`, `position x= y=`, `scale`. Modes are intentionally left
unset so niri keeps the user's preferred mode; external outputs receive an
explicit `scale 1` so left-position math is deterministic.

## Fail-safe table

| Failure point | Designed behavior |
|---|---|
| Plan refuses profile | Nothing is written; the user gets a specific reason |
| Snapshot cannot be written | Apply aborts before touching the configuration |
| Validation fails | Restore snapshot, reload, report `ValidationFailed` |
| Reload command fails | Restore snapshot, reload, report the niri error |
| Verification times out | Restore snapshot, reload, report the mismatch |
| Mirror window missing or misplaced | Terminate child, remove pid file, restore snapshot, reload |
| Application exits normally | Worker sends `Shutdown`, mirror is SIGTERMed, window closes |
| Application crashes | Stale pid file is detected and cleaned on next start |

## Multi-GPU KMS handling

The target hardware exposes:

| Card | GPU | Connectors |
|---|---|---|
| `card1` | Intel iGPU (PCI 0000:00:02.0) | `eDP-1` (panel), `HDMI-A-1..4`, `DP-1..2` |
| `card0` | NVIDIA dGPU (PCI 0000:00:01.0) | `HDMI-A-5`, `DP-3`, `eDP-2` |

Rules:

- Internal displays are detected by connector prefix: `eDP`, `LVDS`, or `DSI`.
- niri output names match connector names, so `niri msg --json outputs` and
  sysfs entries map one to one.
- External selection is deterministic: HDMI before DisplayPort, then already
  enabled outputs, then lexicographic name. On this hardware that prefers the
  Intel HDMI ports over the NVIDIA ones when both are connected.
- Sysfs is the ground truth for cable state; niri only lists outputs it has
  adopted. Extend profiles therefore persist a block for a disconnected HDMI
  port from sysfs data alone.
- Outputs on the NVIDIA card require kernel modesetting (`nvidia_drm.modeset=1`);
  the manager surfaces diagnostics rather than changing kernel parameters.

## Threading model

```text
UI thread (GTK main context)                 worker thread
  |                                             |
  | AppAction (async-channel, unbounded) ------>|
  |                                             | DisplayService owns
  |                                             |   NiriClient, probe, store,
  |                                             |   supervisor, pid file
  |<----- AppEvent (async-channel) -------------|
  |  glib::spawn_future_local(event loop)       |
```

`glib::MainContext::channel()` was removed from current gtk-rs. The modern
equivalent is used instead: `async-channel` for transport and
`glib::spawn_future_local` to consume events on the main context. The contract
is unchanged: no blocking I/O runs on the UI thread.

Long operations are bounded: niri commands have a 5 s timeout, layout
verification polls for at most 5 s, and mirror termination allows 2 s before
SIGKILL.

## Error model

- `domain::profile::PlanError`: precondition failures with user-facing messages.
- `infrastructure::*` errors: `NiriError`, `HardwareError`, `ProcessError`.
- `service::backup_service::ConfigError`: file and permissions errors.
- `service::display_service::ServiceError`: aggregates the above plus
  `ValidationFailed`, `VerificationFailed`, and `MirrorFailed`.
- `anyhow` appears only at the binary boundary (`main.rs`); library code never
  panics on recoverable conditions.

Lint policy (`lib.rs`): `#![forbid(unsafe_code)]` plus denied
`clippy::unwrap_used`, `expect_used`, `panic`, `todo`, and `unimplemented`
(allowed inside `#[cfg(test)]`).

## Testing strategy

| Level | What is covered |
|---|---|
| Domain unit tests | Connector classification, external selection, scale-aware position math, all five plans, mirror argv, verification mismatches |
| Infrastructure unit tests | Real captured niri JSON fixtures, sysfs fixture trees, KDL splice/remove idempotency, command timeouts, real SIGTERM supervision, pid file lifecycle |
| Service unit tests | Full pipeline with fakes: happy path, validation rollback, verification rollback, reset idempotency, mirror start/stop, orphan cleanup |
| Integration tests | Real `FileConfigStore` on temporary directories: managed section on disk, pristine backup bytes, persist-without-cable, refusals |
| Live tests (`--ignored`) | Real niri outputs, real sysfs, real `niri validate` on generated configurations in a temporary directory |
| Manual | GUI launch and HDMI-dependent flows with a physical monitor |

The presentation layer is covered by pure view-model tests plus a manual UI
checklist; GTK widget construction is intentionally thin.

Measured with `cargo llvm-cov` (presentation and `main.rs` excluded): 90.86
percent line coverage and 91.45 percent region coverage, with every measured
file at or above 80 percent lines.
