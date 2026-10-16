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
shell's background. Rounded pane masks follow the card interior; shell controls are composited above
them. The shell layer continues to own all input. Inactive
workspace windows in visible panes receive paced frame callbacks; their commits
request a repaint. Pane composition uses no capture readback or PNG encoding.

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
windows remain visible in their workspace panes. The shell uses transparent clearing, and the compositor places a black translucent
dimmer behind the cards so wallpaper remains visible around them, including while searching. No image reload, worker or timer is added.
Deploy the matching Shell and Villain builds together; rollback restores both.

Workspace panes fill their card interior with centered cover scaling and a
22-pixel rounded mask. The shell draws its border and controls above the pane;
empty workspaces have no empty minimized shelf or placeholder covering wallpaper.
Overview's dimmer is compositor-owned and sits below the panes. Its full-output
backdrop excludes the bar, so wallpaper reaches all four screen edges.

Rounded pane textures use an LRU capped at three entries and 36 Mi pixels,
separate from the scene-effects cache. The key includes workspace, geometry and
source IDs, commits, crop, transform and alpha. Static repaint reuses textures;
window commits and layout changes replace the affected entry. Eviction, reload
and compositor shutdown release textures. Two transient pane targets are bounded
by the same per-target pixel limit. This adds no worker, timer or watcher.

Verification on 2026-10-16 checked pixel samples along all four card edges
against centered cover scaling (mean channel error below 0.1/255), plus wallpaper
at the screen's top edge instead of the bar. Screenshots show empty cards and
minimized controls above wallpaper. Settled three-second samples had no CPU-tick
or main-thread context-switch growth. After twenty Overview toggles, compositor
RSS was 243580 KiB, with 16 threads and 60 FDs; Shell RSS was 237940 KiB, with 16
threads and 54 FDs. The preceding implementation measured about 225892/231240 KiB
respectively in the same software-rendered harness, with the same thread/FD counts;
these short, separate runs are not a controlled GPU-memory or long-run comparison.
Direct-TTY, hardware GPU, output hotplug and prolonged stress remain unverified.
