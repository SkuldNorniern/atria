# Atria

Atria is a portable display server for Artery OS and ArteryTV. This repository currently
contains its wire protocol, backend-independent state machine, and first output backend:

- `atria` is the user-facing facade.
- `atria-protocol` owns message framing, object and opcode vocabulary, payload codecs,
  version/capability negotiation primitives, and typed errors.
- `atria-compositor` owns connection objects, surfaces, scene placement, focus, capabilities,
  sessions, seats, and frame/buffer lifecycle state.
- `atria-software-output` CPU-composites validated software buffers and presents complete
  frames to headless or file-backed sinks.
- `atria-drm` owns the audited ioctl/mmap boundary and scans software-composited frames out
  through atomic KMS or the reported legacy page-flip fallback.

The protocol, compositor, and facade packages are `#![no_std]`. The software output package
uses `std` for owned pixel storage and portable file I/O on Linux and FreeBSD. The DRM sink
uses `std` plus the platform `libc` syscall boundary on those same targets.
