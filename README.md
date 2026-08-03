# Atria

Atria is a portable display server for Artery OS and ArteryTV. This repository currently
contains its wire protocol and backend-independent state machine:

- `atria` is the user-facing facade.
- `atria-protocol` owns message framing, object and opcode vocabulary, payload codecs,
  version/capability negotiation primitives, and typed errors.
- `atria-compositor` owns connection objects, surfaces, scene placement, focus, capabilities,
  sessions, seats, and frame/buffer lifecycle state.

All packages are `#![no_std]`; only the test suites use `std`.
