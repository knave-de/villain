# Villain build and packaging

Villain is Rust/Cargo-first. Cargo owns its libraries, binaries, tests, and
workspace orchestration. Build changes must verify debug and release artifact
paths, generated files, dependency discovery, clean-checkout reproducibility,
and staged installation.

Compilation and unit tests do not validate DRM/KMS, a real local VT/seat,
Wayland socket behavior, GPU presentation, process reaping, or installed
startup. Those paths require explicit integration or live smoke checks.

Packaging and installation must use an explicit prefix and must not implicitly
write to `/usr/local`. Keep build-system cleanup separate from compositor
behavior, configuration migration, and legacy-binary removal.
