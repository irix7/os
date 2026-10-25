# mesh-service

Host-testable membership and failure-detection state for the IRIX 7 logical
computer. It owns the local view of node identity, incarnation, liveness and
membership epoch; it does not implement networking, gossip or threshold
signatures.

The single-node view is the base case. Joining a node bumps the epoch; a
suspect node remains routable until the failure detector confirms it failed;
leaving or failing removes it from the membership snapshot. Routing is then
the deterministic rendezvous computation in `mesh-core`.

The real service will feed this table from SWIM probes and authority-signed
membership epochs. A partitioned node keeps a local view; it cannot invent a
higher global epoch without the authority proof.
