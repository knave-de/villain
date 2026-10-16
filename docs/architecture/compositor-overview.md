# Compositor-rendered overview panes

Villain implements overview visibility and `set_overview_panes`. Knave Session
starts one hidden Overview shell process.
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

Knave, Villain, and Shell use the same visibility and pane message shapes.
Update the three packages together.
The selected hardware output currently has scale 1. Live nested and direct-TTY
rendering, damage, input, frame pacing, CPU, memory, and wakeups need measurements
on the target session before performance is considered verified.

Overview panes and PNG previews include only the background-layer surface named
`knave-shell-wallpaper`, beneath workspace windows. Other shell layers remain
excluded, and workspace-only screen sharing still excludes every shell layer.
The scene cache distinguishes desktop, wallpaper preview and capture selection.
During Overview the full-output scene omits ordinary and unmanaged windows;
windows remain visible in their workspace panes. The shell uses transparent
clearing and a black translucent backdrop so wallpaper remains visible around
the cards, including while searching. No image reload, worker or timer is added.
Deploy the matching Shell and Villain builds together; rollback restores both.
