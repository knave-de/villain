# Villain configuration boundary

The canonical user configuration is
`~/.config/knave/config.toml`, owned by Knave settings.

Villain may retain a compatibility reader for legacy configuration during an
explicit migration, but it must not create a competing user-facing config
store. New compositor settings need a typed projection or versioned contract
from Knave, with validation, defaults, precedence, migration, and rollback
defined before adoption.

Runtime state, socket paths, seat/VT discovery, and process bookkeeping belong
to the runtime/session boundary rather than persistent user configuration.
Legacy data must not be silently deleted.
