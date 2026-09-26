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

The public desktop contract is owned by Knave's knave-desktop-api crate.
Villain is its versioned server implementation. knavectl is the public control
client; villainctl and villain-ipc are removed.

Version and test changes to:

- the pinned Knave API revision and protocol messages;
- socket naming, permissions, and readiness behavior;
- workspace and window IDs and fields;
- focus and activation semantics;
- keybinding dispatch;
- configuration projection and reload;
- layer-shell lifecycle;
- nested/direct backend selection;
- environment and portal activation.

For each change, find every sender and receiver, define compatibility or
migration behavior, update tests, and document user-visible effects.

Villain must not create a second user-facing configuration store. It consumes
validated projections from ~/.config/knave/config.toml and keeps source
configuration readable only as an explicit migration path.

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

Do not perform opportunistic refactors or unrelated cleanup. Review error paths,
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

## Performance and resource usage

Review resource behavior before adding compositor watchers, timers, IPC
subscriptions, event-loop work, background tasks, threads, caches, buffers, or
parallel work.

- Keep the compositor event loop event-driven. Avoid busy loops, high-frequency
  polling, duplicate subscriptions, and retry storms.
- Bound per-client and per-surface work, queues, retries, memory, and
  concurrency. Do not create unbounded workers for clients, outputs, or events.
- Give every activity explicit cancellation and cleanup on disconnect, reload,
  client destruction, backend failure, and shutdown.
- Measure CPU, resident memory, threads, file descriptors, wakeups, and
  relevant latency under idle, normal, and stress workloads.

Compilation, unit tests, and a successful compositor start do not establish
acceptable resource behavior.
