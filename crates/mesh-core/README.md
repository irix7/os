# mesh-core

The host-testable policy for the IRIX 7 logical computer:

- rendezvous home computation for objects and actors;
- a bounded route cache with home and object epochs;
- leases with fencing epochs;
- placement summaries and GPU-aware host choice.

The single-node path is the base case: `MembershipView::single` makes every
home local and the mesh envelope is not constructed. At 128 nodes the same
policy shards homes without a central directory.

This is policy only. Membership gossip, authority manifests, transport and
lease persistence live in the native mesh service.
