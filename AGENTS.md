# Villain WM Agent Instructions

Villain is the Wayland compositor and window manager for the Knave Desktop
Environment. It owns compositor mechanism and window-management policy:
outputs, rendering, input, workspaces, layout, focus, activation, XWayland,
and layer-shell handling.

Knave owns the user-facing configuration, settings, session lifecycle, public
desktop API, and cross-component compatibility policy. Preserve that boundary.

## Startup and investigation

Before editing:

1. Read this file and the umbrella `knave/AGENTS.md` when working on a
   cross-repository change.
2. Read the relevant README, architecture, protocol, and build documentation.
3. Inspect the complete owning module and search all consumers of changed state,
   protocol messages, configuration keys, or commands.
4. Check `git status` and preserve unrelated dirty changes.
5. Classify the change as private, additive, behavioral, configuration,
   migratory, protocol, or breaking.
6. Write a short impact summary before changing a shared contract.

Do not attribute input, focus, or client behavior to a shell or application
until the compositor event path has been inspected.

## Contracts and compatibility

The current `villain-ipc` protocol is an internal compositor transport. It is
not automatically the public Knave desktop API. `villainctl` is a transitional
developer client and must not be removed until Knave has command and API
parity.

Version and test changes to:

- `villain-ipc` request/response/event types;
- socket naming and readiness behavior;
- workspace and window fields;
- focus and activation semantics;
- keybinding dispatch;
- configuration parsing and reload;
- layer-shell lifecycle;
- nested/direct backend selection;
- environment and portal activation.

For each such change, find every sender and receiver, define compatibility or
migration behavior, update tests, and document user-visible effects.

Villain must not create new user-facing configuration ownership during the
migration to `~/.config/knave/config.toml`. Existing Villain configuration must
remain readable until an explicit Knave migration is complete.

## Runtime safety

Keep nested and direct-TTY behavior distinct:

- nested mode must not replace the host compositor's activation environment;
- direct mode requires a real VT, seat, GPU, and input session;
- inherited `DISPLAY` or `WAYLAND_DISPLAY` must not override real VT detection;
- compositor-intercepted keys must not be forwarded as stuck client modifiers;
- explicit focus transitions take precedence over stale pointer hit-testing;
- layer-shell surfaces remain separate from ordinary workspace windows.

Do not claim DRM/KMS, GPU, VT, portal, XWayland, or live focus behavior from
unit tests or compilation alone.

Runtime code must have explicit ownership, cleanup, permissions, process
reaping, shutdown, stale-socket handling, and typed errors. Avoid panics for
recoverable configuration, IPC, device, or process failures.

## Code quality and comments

Keep compositor mechanism separate from window-management policy. Keep IPC
serialization separate from state mutation. Keep focus state separate from
pointer delivery and layer-shell focus.

Do not perform opportunistic rewrites or unrelated cleanup. Review error paths,
event ordering, modifier state, workspace transitions, client destruction,
XWayland stacking, output changes, and frame pacing for every relevant change.

Comments should explain invariants, protocol sequencing, safety assumptions, or
non-obvious reasons. Do not restate obvious Rust code or write long comments.

## Build and verification

Villain is a Cargo workspace. Use:

```console
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use isolated runtime/config directories for protocol tests. Run ignored
Wayland, layer-shell, XWayland, and frame-callback tests when the change
affects those paths.

Update README and examples when build, run, configuration, protocol, or
behavioral contracts change. Keep documentation concise and do not document
commands or behavior that the code does not provide.

## Git and GitHub

- Preserve unrelated dirty changes.
- Create a new focused branch for every task.
- Use Conventional Commits.
- Commit every coherent implementation slice.
- Link dependent cross-repository pull requests.
- Describe affected contracts, consumers, compatibility, verification,
  rollout, and rollback in cross-repository pull requests.
- Always squash and merge.

Before completion, inspect the final diff, run `git diff --check`, check
generated files, and clearly report live behavior that remains unverified.
