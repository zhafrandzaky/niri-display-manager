# AGENTS.md — engineering rules for AI coding agents

This file is authoritative for any AI agent (or human contributor) changing
this repository. Read it before editing code.

## Project summary

`niri-display-manager` is a GTK4/libadwaita Rust application that manages
display profiles for the niri Wayland compositor: internal only, extend
left/right, HDMI mirroring through wl-mirror, and external-only. It runs on any
distribution with niri, managing a fenced section either in a dedicated
`cfg/display.kdl` that the main configuration includes (modular mode) or
directly in the main `config.kdl` (portable inline mode), always with automatic
backup, validation, and rollback. Path discovery and mode detection are
documented in `docs/PORTABILITY.md`.

## Required commands

Run these before claiming any change is complete:

```sh
cargo fmt                      # or: make fmt
cargo fmt --check              # or: make fmt-check
cargo clippy --all-targets -- -D warnings   # or: make clippy
cargo test                     # or: make test
make check                     # all three gates
```

Live niri session tests (read-only, temporary files only):

```sh
cargo test --test live_environment -- --ignored
```

Never run `cargo test -- --ignored` on a machine where the user has not agreed
to the live tests; they only read state and validate files in `/tmp`, but they
do require a niri session.

## Architecture rules (non-negotiable)

- `presentation -> service -> domain`; infrastructure implements traits used by
  services. Never add a reverse dependency.
- The domain layer (`src/domain/`) performs no I/O and must not import
  `gtk4`, `adw`, filesystem, or process types.
- All external interaction goes through the traits in
  `infrastructure/` (`NiriClient`, `HardwareProbe`, `CommandRunner`,
  `ProcessSupervisor`) or `service::backup_service::ConfigStore`. If new
  external behaviour is needed, extend the trait and its fakes; do not call
  `std::process` or `std::fs` from services or UI.
- The GTK main context must never block. Long operations run on the worker
  thread in `presentation::views::main_window`; UI updates arrive through
  `async-channel` and `glib::spawn_future_local`.
- `glib::MainContext::channel` does not exist in current gtk-rs. Do not
  reintroduce it; use the async-channel pattern already in place.

## Rust standards

- Edition 2024; `rust-version = "1.92"`.
- `#![forbid(unsafe_code)]` stays. Do not add `unsafe` blocks anywhere,
  including tests.
- Library code must not call `unwrap()`, `expect()`, `panic!()`,
  `todo!()`, or `unimplemented!()`; these are denied by lint attributes in
  `lib.rs` and only relaxed under `#[cfg(test)]`.
- Use `thiserror` for domain/infrastructure/service errors and `anyhow` only at
  the `main.rs` boundary.
- Prefer let-chains (`if let Some(x) = y && condition`) over nested `if`s;
  clippy is configured to fail on collapsible blocks.
- Keep functions small and single-purpose; keep files under 800 lines
  (production code target 200-400). Test modules live in submodule files when a
  file would exceed the limit (see `src/service/display_service/`).
- No new dependencies without a clear need; the dependency set is deliberately
  small and license-clean (no GPL crates in this MIT project).

## UI sanitation rules

- Zero emoji or non-ASCII emoticons in labels, headers, tooltips, status text,
  or log messages. UI strings must be ASCII; this is enforced by a unit test in
  `presentation::view_model`.
- Use freedesktop/Adwaita symbolic icon names only, and verify they exist on
  the target system before using a new one.
- Error messages shown to users must be specific and actionable, in English.

## Safety rules for the user's system

- The manager may only write inside the managed KDL fence. Never rewrite,
  reorder, or reformat user content outside the fence markers.
- Configuration writes go through `ConfigStore` in both modes (modular
  `cfg/display.kdl` or inline `config.kdl`); never auto-inject `include`
  directives. An explicitly configured but unregistered display file must fail
  fast, not write silently.
- Never delete `display.kdl.bak`; it is the user's pristine copy.
- Never disable or bypass the `niri validate` gate, the post-reload
  verification, or the rollback path.
- Never spawn commands through a shell with interpolated strings; always pass
  argument vectors to `CommandSpec` so output names cannot inject commands.
- Mirror cleanup must remain SIGTERM-first with a bounded grace period and a
  pid-file orphan guard based on `/proc/<pid>/comm`.
- Do not add features that turn off every connected display. External Only must
  keep refusing when no external display is active.

## Testing requirements

- Every behaviour change needs a test at the right layer:
  - pure logic -> domain unit tests;
  - adapters -> infrastructure unit tests with fixtures (real niri JSON is in
    `tests/fixtures/`);
  - workflows -> service tests with the fakes in the same module or
    `tests/common/`;
  - on-disk effects -> integration tests with `FileConfigStore` and
    `tempfile`.
- Keep the fakes honest: they must record calls and simulate reloads, not just
  return success.
- Coverage target: 80 percent on domain/service/infrastructure. The
  presentation layer is covered by view-model tests plus a manual checklist.
- Never weaken an existing test to make a change pass; fix the behaviour or the
  test's premise, and say why in the commit message.

## Commit conventions

`<type>: <description>` with types `feat`, `fix`, `refactor`, `docs`, `test`,
`chore`, `perf`, `ci`. Keep commits scoped; mention rollback or safety-relevant
changes explicitly.
