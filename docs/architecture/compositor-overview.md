# Compositor-rendered overview panes

Villain implements Knave desktop API 1.3 `set_overview_panes`. The shell sends
up to three workspace IDs and rectangles in logical output coordinates. Villain
rejects duplicate IDs, invalid workspaces, zero or oversized rectangles, and
requests without a mapped overview layer surface with a retryable unavailable
response. Replacing the list requests a repaint. Unmapping or destroying
`knave-shell-overview` clears the list; output resize invalidates its geometry. There is no persistent state.

Both nested Winit and direct TTY use the same render-element construction.
Existing window surface textures are transformed to fit the output aspect
ratio inside each pane, cropped to its rectangle, and composited above the
shell's background. The pane rectangles avoid the shell-drawn controls and
rounded card corners. The shell layer continues to own all input. Inactive
workspace windows in visible panes receive paced frame callbacks; their commits
request a repaint. This adds no capture, readback, PNG encode, or image cache.

The request is additive. API 1.2 clients and the old PNG query still work with
this compositor. Install Villain before a new shell; rollback the shell first.
The selected hardware output currently has scale 1. Live nested and direct-TTY
rendering, damage, input, frame pacing, CPU, memory, and wakeups need measurements
on the target session before performance is considered verified.

Desktop API 1.4 adds a point-selection command. Villain checks that the point is
inside the active pane, maps it through the render fit transform, and selects the
topmost visible window at that point. Empty space selects the pane's workspace.
The default standalone Super binding toggles one overview surface; combinations
continue to use their separate shortcuts. A pending launch is bounded to one
child and a second Super release cancels it when its surface appears.
The exact legacy `MOD` binding that executes `knave-shell overview` is
interpreted as the toggle for existing user configuration; other custom exec
bindings keep their configured commands.
