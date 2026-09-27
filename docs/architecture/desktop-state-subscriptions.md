# Desktop state subscriptions

Villain implements Knave desktop API 1.2 `subscribe` on a dedicated connection.
The first response is a complete snapshot, registered atomically on the compositor
event loop. Later snapshots replace prior state and may coalesce intermediate
changes. Existing request/response clients are unaffected; no unsolicited frames
are written to query or action connections.

Relevant mutations mark desktop metadata dirty. After an event batch, Villain
compares workspace/window summaries and advances a session-local generation only
when they differ. Wayland title/app-ID callbacks and XWayland property notifications
are included. Cursor movement and pixel-only commits do not broadcast snapshots.
This does not implement live preview damage notifications.

A nonblocking listener accepts up to 16 clients. Each client uses one worker;
subscription workers block on socket readiness and a wake socket instead of a
timer. Each subscriber retains at most one pending frame, and serialized snapshots
are shared and capped at 16 MiB. A write or event-loop handoff times out after two
seconds. Completion notifications reap workers and close control descriptors;
shutdown removes sources, closes all client sockets and joins workers. The
compositor never waits for subscriber writes.

Deploy the pinned Knave API revision, then this compositor, then the subscribing
shell. API 1.0/1.1 query/action clients retain support. Roll back the shell before
the server. No user configuration, socket path or permissions change occurs.
See Knave's ADR 0002 for wire semantics and compatibility. Binary version stays
unreleased 0.1.0; the public desktop API advances from 1.1 to 1.2.

## Measurement

An isolated nested compositor probe on 2026-09-27 observed no subscription data
through three idle seconds. Across 40 workspace switches, action-acknowledgement
to snapshot receipt was 0.031 ms median, 0.112 ms p95 and 0.263 ms maximum; revisions
increased throughout. This measures IPC delivery, not visible frame latency.
Forty subscribe/disconnect cycles returned to the same 9 threads and 40 file
descriptors. Compositor shutdown took 16 ms. Whole-process idle CPU rose four
scheduler ticks during that interval, including nested rendering; zero CPU
wakeups are not claimed. The previous shell requested state every 500 ms; the
new healthy connection has no state refresh timer. Direct-TTY behavior and
long-running desktop resource usage were not measured in this probe.
