# Changelog

All notable changes to this project will be documented in this file.

## [0.0.2] - 2026-09-30

### Added

- Sd verbs on quipu, storage modes (repo-local pendant, remote, sync), wasm core (#2) ([ed5133c](https://github.com/scbrown/seeds/commit/ed5133cd3386fe399ae99811d889e97fd9f68f43))
- *(remote)* Allow_plain_http_hosts, a user-only opt-in to send the token over plain http (#3) ([7b70a60](https://github.com/scbrown/seeds/commit/7b70a6089c0814d0931dc32d5c9bbf4d49ce13d8))

### CI/CD

- Release pipeline with prebuilt sd binaries and crates.io trusted publishing ([835d71f](https://github.com/scbrown/seeds/commit/835d71f387b3946d982e4df0d0b41f19af39c9ed))
- *(release)* Run checksums when build succeeded on dispatch and tag runs ([e0f6783](https://github.com/scbrown/seeds/commit/e0f67839f2088e11f0211b5ad45481c0c1756193))
- *(release)* Release-plz, git-cliff changelog and the book on GitHub Pages (#7) ([8ce9a21](https://github.com/scbrown/seeds/commit/8ce9a2195ad38ac4fe301ff9610fc35d5244117c))

### Documentation

- Use the sd command, document install and the chmln/sd collision ([2d024fd](https://github.com/scbrown/seeds/commit/2d024fd8034e51020f8b16cf0cc5db869c9c5f7e))
- Seeds is the beads replacement and a quipu-stack member (#4) ([64214c8](https://github.com/scbrown/seeds/commit/64214c8a0480f28204b2a8a2b6c5d684592734f6))
- *(changelog)* Seed CHANGELOG.md with cliff's exact header (#9) ([6b1bb4b](https://github.com/scbrown/seeds/commit/6b1bb4b9e634827b24901d67ae55203c21f2125d))

### Fixed

- *(validate)* A blocker that has its own blocker can be added (#6) ([44bef05](https://github.com/scbrown/seeds/commit/44bef055258567d13b59910605d5cb3864255bae))

### Miscellaneous

- Seeds tracks its own backlog in a repo-local pendant (#5) ([7cd7804](https://github.com/scbrown/seeds/commit/7cd7804f79c41d30de0e52c73f8f171cacb8053e))
