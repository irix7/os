# mesh-migrate

M2 L0 stackless-actor migration policy.

The crate owns the portable actor state, its canonical little-endian codec and
state hash, the root-signed `ActorMove` directive, the `I7M1` migration wire
bodies, and the `PREPARE/READY/STATE/COMMIT/ABORT` lease state machine.

It deliberately does **not** own threads, stacks, TLS sessions, sockets, file
descriptors, a general actor scheduler or a persistent directory. A migrated
actor resumes at a message boundary; node-local resources are not migrated.

See `docs/adr/m2-0009-actor-migration.md` and `docs/plan/m2-rig.md`.
