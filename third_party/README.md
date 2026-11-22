# third_party

Vendored, patched copies of upstream crates. Each entry names the patch and when to drop it.

- `xattr/` — xattr 0.2.3 (MIT/Apache-2.0, pulled in by gpui → zed-async-tar). libc ≥ 0.2.190 removed
  `ENOATTR` on Linux, so the crate no longer compiles there; the patch maps it to `ENODATA` on
  Linux/Android only (`src/sys/mod.rs`, marked `RELAY PATCH`). macOS is unchanged. Drop it when
  gpui stops depending on xattr 0.2.
