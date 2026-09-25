# Villain change-impact checklist

Before changing compositor, input, focus, layout, protocol, configuration
compatibility, or lifecycle code:

1. identify every affected surface, client, shell, session, and command;
2. inspect both sides of every protocol and every focus/input transition;
3. classify the change and record configuration, version, and rollback impact;
4. test normal, failure, shutdown, and stale-runtime-file paths; and
5. distinguish compiled/unit-tested behavior from live Wayland, GPU, VT, and
   installed-binary verification.

Cross-repository changes must update the linked Knave/shell consumers and
preserve a compatibility path until the coordinated replacement is deployed.
