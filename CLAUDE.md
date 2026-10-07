# CLAUDE.md — context anchor for Claude Code

## Mission

Maintain `niri-display-manager`: a production-grade Rust/GTK4 display manager
for the niri compositor. Correctness and safety of the user's display setup
outrank feature velocity. A bad change here can black-screen a laptop.

## Read first

1. `AGENTS.md` — binding engineering rules.
2. `docs/ARCHITECTURE.md` — layer map, trait contracts, apply pipeline,
   rollback model, multi-GPU handling.
3. `docs/USAGE.md` — what each profile promises to the user.

## Architecture idioms to preserve

- Clean Architecture with one-way dependencies:
  `presentation -> service -> domain`, infrastructure implements service-facing
  traits. Dependencies never point back.
- Domain purity: `src/domain/` has no I/O and no GTK. If you need to read
  something, it belongs in an infrastructure adapter behind a trait.
- Use cases are synchronous and run on the worker thread. The GTK main context
  only builds widgets and consumes `AppEvent`s via `glib::spawn_future_local`.
- Adapters are injected (`NiriClient`, `HardwareProbe`, `ConfigStore`,
  `ProcessSupervisor`, `CommandRunner`); tests use the fakes in
  `tests/common/` and `src/service/display_service/tests.rs`.
- Configuration targeting is dual-mode (modular included file or inline fenced
  section) and resolved through `cli` + `service::config_paths`. Keep both
  modes working when touching `ConfigStore` or the service pipeline.

## Memory-safety and failure discipline

- `#![forbid(unsafe_code)]` must remain; never add `unsafe`.
- No `unwrap`/`expect`/`panic!`/`todo!` outside `#[cfg(test)]`; convert every
  recoverable condition into a typed error.
- Prefer let-chains and early returns; clippy's `-D warnings` gate is part of
  the contract.
- Bounded operations only: 5 s command timeouts, 5 s verification polling,
  2 s mirror termination grace. Do not introduce unbounded waits.

## TDD workflow for this repo

1. Reproduce or specify the behavior as a failing test at the correct layer:
   domain (pure), infrastructure (fixture-driven), service (fakes), integration
   (real `FileConfigStore` + `tempfile`).
2. Implement the minimal change that makes it pass.
3. Run `make check` (fmt, clippy `-D warnings`, tests).
4. Run live tests when relevant:
   `cargo test --test live_environment -- --ignored`.
5. Update `docs/` when behavior, safety guarantees, or commands change.

## Definition of done

- `cargo fmt --check` clean.
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo test` green, including new tests for new behavior.
- Rollback semantics preserved: snapshot -> write -> validate -> reload ->
  verify -> rollback.
- Mirror lifecycle preserved: SIGTERM first, bounded grace, pid-file cleanup,
  orphan detection via `/proc/<pid>/comm`.
- No emoji or non-ASCII in UI strings or logs.
- Files stay under 800 lines; split test modules into submodule files when
  needed.

## Things that must never happen in a diff

- Writing outside the managed KDL fence or deleting `display.kdl.bak`.
- Writing to an explicitly configured display file that is not included by the
  main configuration; the setup-required state must fail fast.
- Skipping or weakening `niri validate`, verification polling, or rollback.
- A profile that can disable every connected display.
- Shell-string command execution with interpolated output names.
- GPL dependencies in this MIT project.
- Tests that mutate the user's real `~/.config/niri/**`.

## Useful commands

```sh
make check
make install
cargo test --test live_environment -- --ignored
RUST_LOG=debug niri-display-manager
niri msg --json outputs | jq
cat /sys/class/drm/card*-*/status
```
