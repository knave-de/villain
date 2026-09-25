# Villain performance and resource usage

Villain's event loop, surface lifecycle, input path, frame scheduling, IPC,
backend watchers, and client cleanup are performance-sensitive.

Before adding a watcher, timer, subscription, cache, buffer, task, thread, or
parallel path:

- keep the event loop event-driven rather than busy or polling;
- bound per-client, per-surface, per-output, queue, retry, memory, and
  concurrency growth;
- define cancellation and cleanup for disconnect, destruction, reload, backend
  failure, and shutdown; and
- check whether reconnects, workspaces, outputs, or surfaces multiply the work.

Measure CPU, resident memory, threads, file descriptors, wakeups, and relevant
latency under idle, normal, and stress workloads. Include nested and direct
runtime paths when they are affected. Compilation, unit tests, and compositor
startup alone do not prove acceptable DRM, Wayland, GPU, or lifecycle resource
behavior.

Record a baseline and expected delta for performance-sensitive changes. Explain
or mitigate regressions before merge; do not hide them by reducing the workload
or omitting measurements.

The desktop IPC listener admits at most 16 simultaneous client connections.
Each active connection may have one request waiting on the compositor event
loop; additional clients remain in the Unix listener backlog until a slot is
released. Disconnects return their slot, and shutdown drops the listener with
the compositor process.

Direct-session environment activation uses one worker and a one-entry request
queue. Reload bursts are coalesced instead of spawning one thread per reload;
shutdown sets a cancellation flag, terminates an active integration child, and
joins the worker.

## Nested baseline

A release nested-session sample on 2026-09-26 measured Villain in Winit mode
with the Knave supervisor and Shell bar. After two seconds of startup, six
one-second samples recorded Villain at 2.4% down to 0.8% process-lifetime CPU,
about 121 MiB RSS, and 9 threads. This is a host-specific nested baseline, not
an acceptance threshold. Direct TTY/DRM/seat/GPU behavior remains unverified.
