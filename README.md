# Villain

Villain is the compositor and window manager for the independent Knave Desktop
Environment. It owns Wayland protocol handling, window layout, focus,
workspaces, input policy, rendering, and direct seat/output access.

Knave owns session startup, persistent configuration, public desktop commands,
and the shell. Villain does not integrate with GNOME, KDE, Qt, or another host
desktop. Winit is a nested development backend; the direct TTY backend is the
independent desktop runtime.

## Runtime backends

    villain --tty
    villain --winit

With no option, backend selection follows the repository's safe environment
policy. The TTY path requires a real local VT/seat and performs DRM/KMS setup.
The Winit path runs nested inside an existing Wayland or X11 desktop for
development and tests. Compilation does not prove either path can acquire a
seat, GPU, or display.

## Build and install

Villain is Cargo-first:

    cargo fmt --all -- --check
    cargo test --workspace
    cargo clippy --workspace --all-targets -- -D warnings
    cargo build --release --locked

Install the compositor to the user prefix, system prefix, or an explicit
staging prefix:

    scripts/install.sh --user
    scripts/install.sh --system
    scripts/install.sh --prefix "$PWD/stage"

The installer installs villain only. knavectl is installed by the Knave
repository because it is the public control CLI for the complete desktop.

## Knave desktop IPC

Villain serves Knave's versioned JSON-lines desktop contract at:

    $XDG_RUNTIME_DIR/knave/desktop-$WAYLAND_DISPLAY.sock

The socket is mode 0600 and the containing directory is mode 0700. KNAVE_SOCKET
overrides the derived path for isolated tests. The wire types and client live
in Knave's knave-desktop-api crate; Villain must not create a competing public
protocol.

Control commands are issued through knavectl:

    knavectl version
    knavectl snapshot
    knavectl windows
    knavectl workspaces
    knavectl dispatch workspace 2
    knavectl dispatch focus-window 1
    knavectl dispatch minimize
    knavectl dispatch exec kitty

villainctl is removed. New commands belong to knavectl and are reviewed as
Knave desktop API changes.

## Configuration

The target persistent configuration is
~/.config/knave/config.toml, owned and schema-versioned by Knave. The
compositor settings live under [compositor] with modkey, input,
environment_file, and bind entries. Knave projects the old root-level
modkey, environment_file, [input], and [[bind]] keys when that table is
absent, preserving the source document for rollback. Villain never writes a
second configuration store. Villain receives the validated projection during
session startup and reload; legacy compatibility is migration-only and must
not silently delete old values.

## Repository boundaries

| Area | Owner |
| --- | --- |
| Session lifecycle and process supervision | Knave |
| Persistent settings and public desktop API | Knave |
| Wayland compositor, focus, input, layout, and rendering | Villain |
| Shell presentation and interaction | Knave Shell |

Every change to a shared ID, IPC message, configuration key, or lifecycle
path must inspect all consumers, version behavior, migration, rollback, and
the direct/nested live-test coverage.

## Documentation

- [Architecture index](docs/architecture/README.md)
- [Component boundaries](docs/architecture/component-boundaries.md)
- [Performance policy](docs/architecture/performance.md)
- [Change-impact checklist](docs/architecture/change-impact.md)
- [Agent instructions](AGENTS.md)
