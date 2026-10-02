# Portability and Configuration Discovery

This document describes how niri-display-manager finds, chooses, and manages
configuration files across different Linux distributions and niri setups. It
complements `docs/ARCHITECTURE.md`, which covers the apply pipeline itself.

## Dual-mode management

The manager can reach niri in two ways. The mode is detected at startup; no
manual configuration is required.

| Mode | Managed file | When it is used |
|---|---|---|
| Modular | A dedicated display file (default `<main config dir>/cfg/display.kdl`) | The file is already included by the main configuration, or `--display-config` was passed |
| Inline (portable) | A fenced section appended to the main `config.kdl` itself | No dedicated display file is wired into niri (vanilla single-file setups) |

In both modes the managed file is:

- snapshotted once before the first change (`.bak` next to the file),
- written atomically (temp file, fsync, rename, permissions preserved),
- gated through `niri validate` before reload,
- verified against the real output state, with automatic rollback on failure.

The fence markers are identical in both modes:

```kdl
// >>> niri-display-manager: managed section - manual edits will be overwritten
// profile: extend-right
output "eDP-1" {
    position x=0 y=0
}
// <<< niri-display-manager: end of managed section
```

Because niri treats `output` sections as multipart and positional, a section
appended at the end of `config.kdl` overrides earlier output blocks for the
same outputs. User content outside the fence is never rewritten.

## Main configuration resolution

Highest precedence first:

| # | Source | Notes |
|---|---|---|
| 1 | `--config <PATH>` | CLI override; relative paths resolve against the working directory |
| 2 | `$NIRI_CONFIG` | niri's own configuration variable, honored by the manager |
| 3 | `$NDM_CONFIG_DIR/niri/config.kdl` | Base-directory override for tests and advanced setups |
| 4 | `$XDG_CONFIG_HOME/niri/config.kdl` | XDG standard |
| 5 | `$HOME/.config/niri/config.kdl` | Fallback |

If none of these produce a path, the manager fails with a typed error
(`NoConfigBase`) instead of guessing a temporary directory.

The display target defaults to `<main config directory>/cfg/display.kdl`. An
explicit `--display-config <PATH>` always selects modular mode with that file,
even when the include is not (yet) present; in that state profiles fail fast
with a setup message instead of writing to a file niri never reads.

## Include detection

Detection uses full-path comparison, not basename matching:

- `include "cfg/display.kdl"`, `include "./cfg/display.kdl"`, absolute paths,
  and `~/`-relative paths (niri 26.04+) are resolved relative to the including
  file and compared against the target path.
- `include optional=true "..."` is understood.
- One level of indirection is followed: if the main config includes
  `modular.kdl`, and `modular.kdl` includes the display file, the layout is
  treated as modular.
- `include "old/display.kdl"` does **not** count as including
  `cfg/display.kdl`; same-basename false positives are prevented.
- Only top-level include lines are considered, matching niri's requirement.

## Compatibility matrix

| Setup | Result |
|---|---|
| CachyOS / modular `cfg/*.kdl` layout | Modular mode; existing behavior preserved |
| Vanilla Arch / Fedora / Void with a single `config.kdl` | Inline mode; profiles are applied through a fenced section in `config.kdl` |
| `$NIRI_CONFIG` pointing elsewhere | The manager resolves and manages the same file niri uses |
| `niri -c <file>` without `NIRI_CONFIG` | Pass `--config <file>`; otherwise the default path is used |
| niri older than 25.11 (no `include` support) | Inline mode works; modular mode is never selected automatically |
| Missing `HOME`, `XDG_CONFIG_HOME`, `NDM_CONFIG_DIR`, `NIRI_CONFIG` | Typed `NoConfigBase` error; no files are touched |
| Read-only configuration | Clear `ConfigError`; no partial writes (atomic rename) |

## Failure and cleanup semantics

- A managed file that does not exist yet produces no empty `.bak` artifact;
  rollback in that case simply removes the managed section.
- In modular mode with an unregistered display file (for example after
  `--display-config` for a file whose include was removed), write profiles fail
  fast with `setup required: ...` before any snapshot or write. The reset
  profile (`Internal Display Only`) still works because it only restores.
- The inline fence never deletes user content: rollback restores the snapshot
  or removes only the fenced lines.
- Known nuance: niri flags cannot be unset by a later block, so if a user
  manually disables an output in their own config (`output "X" { off }`), the
  manager cannot reverse that from an appended fence. This is identical to the
  behavior of modular includes.

## CLI reference

```text
niri-display-manager [OPTIONS]

  --config <PATH>          Main niri configuration file
  --display-config <PATH>  Managed display file (forces modular mode)
  --print-paths            Print resolved paths and mode, then exit
  -h, --help               Print help and exit
```

Environment:

| Variable | Purpose |
|---|---|
| `NIRI_CONFIG` | Main niri configuration path (niri's own variable) |
| `NDM_CONFIG_DIR` | Configuration base directory override (tests, advanced setups) |
| `RUST_LOG` | Log filter, for example `RUST_LOG=debug` |

Example:

```text
$ niri-display-manager --print-paths
mode:          modular (display file included by the main configuration)
main config:   /home/user/.config/niri/config.kdl
managed file:  /home/user/.config/niri/cfg/display.kdl
backup:        /home/user/.config/niri/cfg/display.kdl.bak
```

## Testing

Portability behavior is covered by:

- `service::config_paths` unit tests: resolution precedence, typed error,
  explicit display override, relative path handling.
- `infrastructure::kdl_parser` unit tests: top-level include collection,
  `optional=true`, path resolution and normalization, false-positive refusal.
- `service::backup_service` unit tests: mode discovery, one-level indirection,
  inline snapshot/restore, absent-file snapshots without artifacts.
- `service::display_service` unit tests: setup-required fail-fast and reset in
  that state.
- Integration tests (`tests/service_flow.rs`): modular apply/rollback and
  vanilla single-file apply/restore against the real filesystem store.
- Live tests (`--ignored`): real niri outputs, sysfs, and `niri validate` on
  generated configurations.
