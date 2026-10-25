# agnostic-core

Architecture-independent actor-table and placement state. Each actor has a
stable `ActorId`, a rendezvous home and a current host; placement changes bump
the actor epoch and are rejected for unknown or departed nodes.

This is the first host-testable core state. The Wasm scheduler, namespace and
service graph build on it; the native shim remains responsible for the actual
instruction context.
