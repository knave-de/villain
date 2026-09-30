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

Villain handles SIGINT and SIGTERM through its calloop event loop, allowing
the Wayland socket and lock to be removed on normal shutdown and initialization
failure. The session supervisor still owns process termination and restart
policy. Direct-session environment activation uses one bounded worker and is
cancellable during shutdown; nested Winit mode does not modify the host
activation environment.

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
session startup and reload; root-level compatibility is migration-only and must
not silently delete old values.

The tracked `config.example.toml` is a root-level migration/test fixture only; it
is not loaded or installed by Villain. Create and edit the canonical Knave
configuration instead.

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

## Window focus

Clicks on empty space or noninteractive panels preserve the focused visible
window. If focus is missing, the most recently focused visible window is
selected. Workspace switches, minimization, and closure also restore visible
window focus. Keyboard-interactive shell surfaces such as overview and launchers
can take focus; empty workspaces, fully minimized workspaces, host focus loss,
and suspended TTY sessions can have no focused window. Shortcut-driven focus
changes wait until compositor-intercepted keys are released.

## Maximize and restore

`MOD+F` toggles maximization in the default binding set. Maximized windows fill
usable workspace space, keeping layer-shell panels and application chrome.
Fullscreen is a separate state and takes precedence while active. Unmaximizing
restores tiled placement or saved floating geometry; minimize/restore retains
the maximized state. Explicitly focusing an unrelated hidden window restores
the normal layout. Child dialogs remain visible above their maximized parent.

    knavectl dispatch maximize
    knavectl dispatch unmaximize
    knavectl dispatch toggle-maximize

Custom binding sets replace the defaults. Add this to the existing canonical
Knave configuration when using custom bindings:

```toml
[[compositor.bind]]
keys = "MOD+F"
dispatch = "toggle-maximize"
```

These commands require desktop API 1.1. Existing Shell clients may remain on
API 1.0. See [maximization policy](docs/architecture/maximization.md).

## Master/stack resizing

Edit the existing
`[compositor]` table in `~/.config/knave/config.toml`:

```toml
[compositor]
master_percent = 60 # 60% master, 40% stack; default 50, valid range 10..90
```

Left-drag within five logical pixels of the main vertical divider. The cursor
changes to a horizontal resize cursor. Default shortcuts are `MOD+Ctrl+Left`
(shrink master by 5 percentage points), `MOD+Ctrl+Right` (grow by 5), and
`MOD+Ctrl+R` (reset to the configured default). Each key press makes one step.
`MOD` is the configured modkey, normally Super.

Custom binding sets replace defaults. Append these entries to your existing
configuration if you use custom bindings; change the keys or signed step size
as desired:

```toml
[[compositor.bind]]
keys = "MOD+CTRL+LEFT"
dispatch = "resize-master"
args = ["-5"]

[[compositor.bind]]
keys = "MOD+CTRL+RIGHT"
dispatch = "resize-master"
args = ["5"]

[[compositor.bind]]
keys = "MOD+CTRL+R"
dispatch = "reset-master"
```

Run `knavectl reload` after editing. Resizing is per workspace and lasts
until reset or restart; reload preserves manual overrides. The percentage is
clamped to 10–90%. Resize actions do nothing with fewer than two tiled windows
or while a maximized/fullscreen window covers the layout. Floating windows and
individual stack dividers are unchanged. These are Villain keybinding actions,
not new desktop IPC commands.
