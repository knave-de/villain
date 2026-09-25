# Villain configuration boundary

The canonical user configuration is
`~/.config/knave/config.toml`, owned by Knave settings.

Villain consumes the typed [compositor] projection:

- modkey and bind define compositor key dispatch;
- input defines touchpad policy; and
- environment_file selects an optional environment fragment relative to the
  Knave configuration file.

If no [compositor] table exists, Knave projects legacy root-level modkey,
environment_file, [input], and [[bind]] values into this model. Villain reads
that projection through the knave-config API. New writes belong under
[compositor], and the old keys remain readable for rollback rather than being
silently deleted.

Runtime state, socket paths, seat/VT discovery, and process bookkeeping belong
to the runtime/session boundary rather than persistent user configuration.
Legacy data must not be silently deleted.
