# Villain versioning and compatibility

Villain independently versions:

- the compositor binary and libraries;
- the private compositor protocol;
- any public desktop contract it directly implements; and
- configuration compatibility adapters.

Private does not mean undocumented: every protocol change needs sender and
receiver discovery, a compatibility range, additive/breaking behavior, and a
rollback path. A compositor-only test run does not prove shell compatibility.

Legacy commands such as `villainctl` remain transitional until all consumers
use the replacement contract and the removal is a separate reviewed change.
