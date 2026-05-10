# Rift Crawler

[![License](https://img.shields.io/badge/license-Proprietary-red.svg)](LICENSE)
[![Build](https://img.shields.io/badge/build-Rust%201.85%2B-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-pre--launch-yellow.svg)](BEFORE_PUBLISHING.md)

A Vulkan-rendered, server-authoritative multiplayer ARPG dungeon
crawler written in Rust. See [`ARCHITECTURE.md`](ARCHITECTURE.md)
for the high-level design and crate layout.

## License

**Proprietary — All Rights Reserved.** See [`LICENSE`](LICENSE)
for the full terms.

In short: you may view the source and build it locally for
personal study, but redistribution, hosting for third parties,
and any commercial use require a separate written licence
from the author. Third-party Rust dependencies are listed in
[`THIRD_PARTY.txt`](dist/THIRD_PARTY.txt) (regenerate with
`./scripts/gen-third-party.sh`) and remain governed by their
own permissive open-source licences.

## Building

```sh
cargo build --release
```

Run the client:

```sh
./scripts/rift.sh client run --release
# or on Windows:
.\scripts\rift.ps1 client run -Release
```

Run the dedicated server:

```sh
./scripts/rift.sh server run
```

## Documentation

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — crate layout, tech
  stack, and rendering pipeline.
- [`BEFORE_PUBLISHING.md`](BEFORE_PUBLISHING.md) — launch
  checklist (showstoppers, polish, ops).
- [`DEPLOYMENT.md`](DEPLOYMENT.md) — operator notes for
  running the dedicated server.

## Contact

Licensing enquiries (commercial use, redistribution, etc.):
emilohlund@hotmail.com
