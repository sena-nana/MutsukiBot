---
name: resource-state-providers
description: Implement or change standard memory, shared-memory, database, state, blob, lease, or ResourceRef provider plugins and their persistence semantics.
---

# Resource And State Providers

- Keep real bytes, mappings and connections inside the provider; expose only descriptors across runtime boundaries.
- Enforce resource generation, version, lifetime, sealing and lease rules from Core contracts.
- Route persistent state changes through declared command/commit tasks instead of direct plugin mutation.
- Make provider loss, stale refs and invalid leases structured failures.
- The persistent `mutsuki.std.resource.sqlite` provider stores blob/COW/capability
  bytes and versions in a product-supplied SQLite file; reopening the same path must
  restore descriptors, versions and bytes, and stale writes keep failing with
  `resource.generation_mismatch`. One-shot outputs are cleaned through the
  capability `delete` command, not by silent eviction.

Test create/read/update, sealing, lease expiry, restart persistence and invalid descriptor behavior.
