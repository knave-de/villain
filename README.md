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

Villain serves Knave's JSON-lines desktop contract at:

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

## Configuration

The canonical configuration is `~/.config/knave/config.toml`, owned by Knave.
Compositor settings live under `[compositor]` with modkey, input,
environment_file, and bind entries. Villain receives the validated projection
during session startup and reload. The unreleased format has no schema version
or root-level compatibility projection.

The tracked `config.example.toml` demonstrates the `[compositor]` section of
Knave's canonical config. Villain does not install a separate config file.

## Repository boundaries

| Area | Owner |
| --- | --- |
| Session lifecycle and process supervision | Knave |
| Persistent settings and public desktop API | Knave |
| Wayland compositor, focus, input, layout, and rendering | Villain |
| Shell presentation and interaction | Knave Shell |

Every change to a shared ID, IPC message, configuration key, or lifecycle path
must inspect all consumers, update the coordinated packages, and include the
relevant direct/nested live-test coverage.

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
Fullscreen is a separate state and takes precedence while active; top-layer
surfaces such as the status bar are hidden until fullscreen ends. Unmaximizing
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

See [maximization policy](docs/architecture/maximization.md).

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

## Screenshots

Screenshot bindings capture Villain's composed output directly. Region capture
freezes the current frame before selection starts; drag with the left button and
press `Escape` to cancel. The compositor retains focus and intercepts selection
input, so the frozen image keeps tooltips and other layer surfaces visible.

| Shortcut | Action |
| --- | --- |
| `Print` | Drag a region and save it |
| `Shift+Print` | Save the output |
| `MOD+Print` | Save the focused window |
| `Ctrl+Print` | Drag a region and copy PNG |
| `Ctrl+Shift+Print` | Copy the output as PNG |
| `MOD+Shift+Print` | Copy the focused window as PNG |

Saved files go to `$XDG_PICTURES_DIR/Screenshots` or `~/Pictures/Screenshots`.
Custom binding sets replace defaults; these actions use
`dispatch = "screenshot"` with arguments `region|output|window` and
`save|clipboard`.

## Window appearance

Configure independent window gap edges, border widths and colors, corner radii,
focus opacity, per-side shadows and background blur through Knave's
`[compositor.appearance]` settings. Maximized effects are individually opt-in;
fullscreen always bypasses effects. Validate with `knave config check` and reload
with `knavectl reload` after deploying the matching compositor. See
[window appearance](docs/architecture/window-appearance.md) for implementation,
compatibility, rollout and resource behavior; the full TOML reference belongs to Knave.

## Selective screen sharing

Villain exposes standard foreign-toplevel enumeration and window image sources
for portal window sharing. A window source stays tied to its original window
across stacking, resizing and workspace switches; closing it stops the source.
Knave's private workspace capture protocol accepts immutable arbitrary workspace
sets. It freezes the last allowed frame on excluded workspaces without rendering
excluded content, and produces black until the first allowed frame. Sessions are
bounded to eight and canvases to eight million pixels. Shell layers and overview
panes are excluded from workspace capture. See Knave ADR 0007 for deployment and
compatibility; multiple-monitor selection remains out of scope.
