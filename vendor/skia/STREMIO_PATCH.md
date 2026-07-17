# Local rust-skia build patch

This directory is `skia-bindings` 0.99.0 from the upstream rust-skia project.
Only its build orchestration is changed.

The application links a static-CRT MPV SDK. rust-skia does not publish a
matching static-CRT Windows archive, so Cargo must build Skia from source. The
local patch keeps that fallback reproducible by:

- selecting the newest complete installed Windows SDK instead of the newest
  version-numbered SDK directory;
- passing the same SDK headers to bindgen; and
- temporarily mapping the crate directory to a short drive path while GN,
  Ninja, and clang-cl compile Skia.

The Skia checkout itself is downloaded by the original upstream build logic and
is not stored in this repository.
