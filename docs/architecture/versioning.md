# Villain versioning and compatibility

Villain independently versions:

- the compositor binary and libraries;
- private compositor implementation protocols; and
- the compatibility range of the Knave desktop API it implements.

Knave owns the public desktop API version. Villain pins a concrete Knave API
revision, validates requests, and must not silently accept incompatible
messages. Private protocol changes still require sender/receiver discovery,
additive or breaking behavior, tests, and rollback.

The public control client is knavectl, owned by Knave. villainctl is removed;
new control commands are Knave desktop API changes, not Villain-local CLI
features.
