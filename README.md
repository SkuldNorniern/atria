# Atria

Atria is a portable display server for Artery OS and ArteryTV. This repository currently
contains its allocation-free wire protocol foundation:

- `atria` is the user-facing facade.
- `atria-protocol` owns message framing, object and opcode vocabulary, payload codecs,
  version/capability negotiation primitives, and typed errors.

Both packages are `#![no_std]`; only the test suite uses `std`.
