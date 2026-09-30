# Compositor-rendered overview panes

Villain implements Knave desktop API 1.4 overview visibility and API 1.3
`set_overview_panes`. Knave Session starts one hidden Overview shell process.
The default Super binding toggles the session-local `overview_visible` state;
Escape and acknowledged shell actions set it false. The shell maps or unmaps
its layer surface from the subscribed snapshot. Destroying the Overview surface
resets visibility, so a supervised restart begins hidden. Visibility is not
persistent. The shell sends up to three workspace IDs and rectangles in logical
output coordinates. Villain
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
