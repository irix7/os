# mesh-sim

Host-only simulation of the single logical computer at 1, 2, 8 and 128 nodes.
It exercises the real `mesh-service` membership table and `mesh-core`
rendezvous policy; it does not implement networking, gossip, authority
signatures or a Wasm runtime.

The simulation checks that every node is routable, homes are deterministic,
load stays bounded, a single node has no peer dependency and failed nodes are
removed from the routing snapshot. It is the first executable version of the
M1/M2/M3/M4 policy gates, not evidence of a running mesh.
