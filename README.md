# Villain

Villain is the Rust/Smithay Wayland compositor and window manager for the Knave
Desktop Environment. It owns compositor mechanism and window-management policy:
outputs, rendering, input, workspaces, layout, focus, activation, XWayland,
and layer-shell.

Villain is experimental. Its internal and public interfaces can change.

## Runtime boundary

Villain provides the compositor. Knave Shell provides desktop-facing UI.
They run as separate processes:

    Knave Shell
        | Wayland + private IPC
        v
    Villain
        |
        +-- native Wayland clients
        +-- XWayland clients

Villain owns window state and compositor decisions. Shell code must consume
explicit contracts and must not duplicate focus, workspace, or layout policy.

## Build

Requirements depend on the selected backend, but a development build needs
Rust/Cargo and the Smithay backend development libraries for Wayland, DRM/KMS,
GBM, libinput, libseat, and XWayland as applicable.

    cargo build --workspace --locked

Run the repository checks:

    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

Use an explicit runtime and configuration directory for development tests.
Do not install implicitly into /usr/local.

## Run

Villain accepts one backend option:

    villain --help
    villain --winit   # nested inside an existing Wayland session
    villain --tty     # direct DRM/KMS from a real local VT and seat

With no option, backend selection follows the current environment. Nested
operation must not replace the host compositor's activation environment.
Direct mode requires a real VT, seat, GPU, input session, and appropriate
permissions. Compilation does not prove direct startup.

Villain publishes a user-only IPC socket:

    $XDG_RUNTIME_DIR/villain-$WAYLAND_DISPLAY.sock

Set VILLAIN_SOCKET when a client needs an explicit override. Wayland clients
use the selected WAYLAND_DISPLAY.

## Configuration

Configuration ownership is migrating to Knave. The current implementation
still reads the legacy Villain file:

1. VILLAIN_CONFIG, when set;
2. XDG_CONFIG_HOME/villain/config.toml; or
3. ~/.config/villain/config.toml.

Use [config.example.toml](config.example.toml) as the current format reference.
The optional environment file is selected by environment_file and accepts
literal KEY=VALUE or export KEY=VALUE lines; it is data, not a shell script.

If any bind entries are present, they replace the complete default binding set.
A configuration reload parses and validates the replacement before applying it.
The reload command is currently:

    villainctl reload

Do not add a new user-facing configuration store. The target canonical file is
~/.config/knave/config.toml; the legacy reader remains until migration is
implemented and verified.

## IPC and villainctl

The IPC protocol is a private JSON Lines transport for Knave components and
developer tools. It is not the public Knave desktop API.

The workspace contains:

| Package | Responsibility |
| --- | --- |
| villain | Compositor, window state, dispatch, and IPC server |
| villain-ipc | Smithay-independent request, response, and client types |
| villainctl | Transitional command-line IPC client |

Examples:

    cargo run -p villainctl -- version
    cargo run -p villainctl -- windows
    cargo run -p villainctl -- workspaces
    cargo run -p villainctl -- dispatch workspace 2
    cargo run -p villainctl -- dispatch focus-window 1
    cargo run -p villainctl -- dispatch minimize
    cargo run -p villainctl -- dispatch restore-minimized
    cargo run -p villainctl -- dispatch exec kitty

The IPC boundary uses one-based workspace numbers. Window IDs are monotonic
for the compositor lifetime and are not reused. Protocol changes require
sender/receiver review and compatibility handling.

## Tests and live verification

Run the ordinary workspace checks before a change is complete. For ignored
Wayland, layer-shell, XWayland, or frame-callback tests:

    cargo test -p villain -- --ignored --nocapture --test-threads=1

The frame-callback helper is documented in
[tests/README.md](tests/README.md). Live DRM/KMS, VT, GPU, Wayland socket,
XWayland, portal, and installed-startup behavior must be reported separately
from unit-test results.

## Architecture and documentation

- [Architecture index](docs/architecture/README.md)
- [Component boundaries](docs/architecture/component-boundaries.md)
- [Configuration boundary](docs/architecture/configuration.md)
- [Performance policy](docs/architecture/performance.md)
- [Change-impact checklist](docs/architecture/change-impact.md)
- [Agent instructions](AGENTS.md)

Keep compositor mechanism separate from window-management policy, IPC
serialization separate from state mutation, and focus state separate from
pointer delivery. Use Conventional Commits for focused changes.
