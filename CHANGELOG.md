# Changelog

All notable changes to this project will be documented in this file.

## [0.0.4] - 2026-10-01

### Added

- *(cli)* Br's --session as a claimed write attribution; --labels/--for visible (w3k75d.13) (#62) ([93c453f](https://github.com/scbrown/seeds/commit/93c453f6746c3fd28237bc275bf23eb39d2292a4))
- *(sync)* --dry-run and --status previews that write nothing (#66) ([5d4d50d](https://github.com/scbrown/seeds/commit/5d4d50df79d3ff70dbfee82ab3dd0da6352829aa))
- *(doctor)* --quick and --robot-triage (#67) ([f0254ca](https://github.com/scbrown/seeds/commit/f0254ca2467ed3f140a1f48bfea03842b6c544f2))
- *(info)* --schema, --whats-new and --thanks (#68) ([9f85fda](https://github.com/scbrown/seeds/commit/9f85fda103b05a22eca6f39f118221e833209968))
- *(version)* --check against the latest published release (#69) ([182303c](https://github.com/scbrown/seeds/commit/182303c04e56c85cca3e916dca5e2d197b426f1d))
- *(orphans)* --fix, br's interactive close (#70) ([2616f00](https://github.com/scbrown/seeds/commit/2616f00b4566e0ca350d46c524d50f183df6e216))
- *(update)* Refuse replacing existing text without --force, like br (#71) ([0578b98](https://github.com/scbrown/seeds/commit/0578b9839d2a242be7eb3c7db912f76e7c3a2973))
- *(create)* --file, br's markdown bulk import, in one transaction (#72) ([b34f5f6](https://github.com/scbrown/seeds/commit/b34f5f6083348bd47d5a4f5e74e4f81527037d17))

### Documentation

- *(readme)* Add seeds to the stack table (#65) ([70aafbe](https://github.com/scbrown/seeds/commit/70aafbea91827f28456379b1780e7301c69d7257))

### Fixed

- *(model)* Carry facts this sd does not model through import, sync and renumber (#73) ([5f5b32b](https://github.com/scbrown/seeds/commit/5f5b32b9fdbb6780ebb2c317de79bbdf2bbe52a2))

### Miscellaneous

- *(parity)* 20 br-engine flags not applicable, with reasons (w3k75d.13) (#63) ([0db0541](https://github.com/scbrown/seeds/commit/0db054146a06a0362fcfd033fa73b6243c983dbf))

## [0.0.3] - 2026-10-01

### Added

- *(cli)* Sd label add/remove/list/list-all/rename, in br's shapes (#13) ([68000ec](https://github.com/scbrown/seeds/commit/68000ecedb48f0f5b6ed42712b3b5888447217a0))
- *(cli)* Sd blocked, in br's shape and semantics (#14) ([2ad0ddb](https://github.com/scbrown/seeds/commit/2ad0ddba48b9ff628b11e75461e29b9af4b2b593))
- *(cli)* Sd reopen, defer and undefer, in br's shapes (#15) ([4dad71f](https://github.com/scbrown/seeds/commit/4dad71f1da694adf527889bbce2393116555ba83))
- *(json)* Every write reports its tx, so a script can pin with --at (sd-non.2) (#16) ([cfceb1e](https://github.com/scbrown/seeds/commit/cfceb1e2f24a6d7a3b8aa6130d23208700f0ebe6))
- *(cli)* Sd search, over br's fields with closed hits counted (#19) ([80caa11](https://github.com/scbrown/seeds/commit/80caa11c11feea2f61cb5d8c621838d613f6449d))
- *(cli)* Sd stale, br's untouched-seeds list (#20) ([4cd042e](https://github.com/scbrown/seeds/commit/4cd042e10d915fbe827099871aea1b63cf6bd96d))
- *(create)* --step/--visit make a workflow step's create idempotent (#17) ([083a40f](https://github.com/scbrown/seeds/commit/083a40fde7173ee077d93ec226eee94d897ea244))
- *(close)* --outcome done|abandoned|superseded|failed (#18) ([7248c28](https://github.com/scbrown/seeds/commit/7248c2805a712b43d98fbb00bac84f3afa12e8fd))
- *(cli)* Sd stats, br's summary and breakdowns (#21) ([ca38c8c](https://github.com/scbrown/seeds/commit/ca38c8c1cc31b57a9b841aef8b8ca97ca26ef6a2))
- *(cli)* Sd epic status and close-eligible, as br (#22) ([9439908](https://github.com/scbrown/seeds/commit/94399084029e98c00a6240f3b65fd9661a98e2d9))
- *(cli)* Sd version, where and info, in br's shapes (#23) ([5c88b11](https://github.com/scbrown/seeds/commit/5c88b114bd019f83cc546ea345772b8acda73d2c))
- *(cli)* Sd completions for bash/zsh/fish/powershell/elvish (#24) ([e51bb33](https://github.com/scbrown/seeds/commit/e51bb337fef1f8a43ffcc07be1561ad239fdb658))
- *(cli)* Sd init, which can never re-point a project's ledger (#25) ([31ee8f2](https://github.com/scbrown/seeds/commit/31ee8f25e5d7ae8e4b0013e03825d7d441e27d2e))
- *(json)* List/search/blocked issues carry br's dependent_count (#26) ([4b53a91](https://github.com/scbrown/seeds/commit/4b53a91c9191440a91b8810acb0764d26e1cc7f9))
- *(model)* Owner, a real field with --owner on create and update (#27) ([7348a60](https://github.com/scbrown/seeds/commit/7348a60c25652dbcb46414673f92e4923127e932))
- *(cli)* Sd delete as a tombstone, with wu's three constraints (#28) ([4d4b8d5](https://github.com/scbrown/seeds/commit/4d4b8d5b687c210ff5aed48a7bb11f2c63735f43))
- *(cli)* Sd q, the status alias, and exit-21 pointers for br verbs that live elsewhere (#29) ([af629e1](https://github.com/scbrown/seeds/commit/af629e14dad665bf24a390fd256e668cabef6a6e))
- *(cli)* Sd graph, br's dependents/dependencies walk and components (#30) ([626a5c6](https://github.com/scbrown/seeds/commit/626a5c6c76c2ecefac709fdec965d93a4533ce56))
- *(cli)* Sd history, a seed's transaction history, which names its meaning (#31) ([6a82ebe](https://github.com/scbrown/seeds/commit/6a82ebe557d7285c7740d2267476234e7e892509))
- *(cli)* Sd changelog, closed seeds by type since a date, tag or commit (#32) ([ddf4739](https://github.com/scbrown/seeds/commit/ddf47399de14dc95da01da1084eabfc78291f882))
- *(cli)* Sd config list/get/path, read-only and token-safe (#33) ([37a10c6](https://github.com/scbrown/seeds/commit/37a10c6ef25fcdf8f487437230b3e69c0afd52fe))
- *(cli)* Sd lint, br's template-section warnings (#34) ([5130947](https://github.com/scbrown/seeds/commit/5130947ce97d10f3baf077d74ace15e3448edcbf))
- *(cli)* Sd schema, published from the same tables the contract tests pin (#35) ([c46bd0a](https://github.com/scbrown/seeds/commit/c46bd0a10cf8c0411b10540c971024ed6b2bff8d))
- *(cli)* Sd capabilities, derived from sd's own definitions (#36) ([8b74e6d](https://github.com/scbrown/seeds/commit/8b74e6db01feae61e299ebad2c8122812efcfa2c))
- *(cli)* Sd doctor, read-only ledger and config checks that gate CI (#37) ([f4e2101](https://github.com/scbrown/seeds/commit/f4e2101c2047c8d481c8df7afe9bae756c752a51))
- *(cli)* Sd orphans, open seeds that a git commit mentions (#38) ([88404ae](https://github.com/scbrown/seeds/commit/88404ae2254bf2833b25cc3809b1f57d71a796a4))
- *(cli)* --robot, alias of --json, on every verb (#39) ([1731b27](https://github.com/scbrown/seeds/commit/1731b274b244dcbeb92fa88164cde85c43316567))
- *(cli)* List/search filters, paging, and count shorthands (#41) ([6810a94](https://github.com/scbrown/seeds/commit/6810a949cfbf07654a20f6ba4b769022d98069c1))
- *(cli)* --transition-comment on close, update, defer, undefer, epic close-eligible (#42) ([ff3cfb5](https://github.com/scbrown/seeds/commit/ff3cfb563122c58e0967450cad93245e52c245fd))
- *(remote)* Signed writes to quipu, no bearer on the wire (#40) ([f97a106](https://github.com/scbrown/seeds/commit/f97a1062e6b277c5b153053a8b87c42c35a3eba6))
- *(cli)* Update -t/--set-labels/--parent/--description-file, create --status/--defer (sd-non.3) (#45) ([f7492c5](https://github.com/scbrown/seeds/commit/f7492c50e4504e49e835bf76fb7164a8102afd78))
- *(cli)* Global --format text|json; toon refused, --stats n/a (#46) ([99c00ae](https://github.com/scbrown/seeds/commit/99c00aefbe423a6d54695dd25e3790eabd594cd5))
- *(ready)* --sort, --include-deferred, -r/--recursive and --epic (#48) ([83ba2e3](https://github.com/scbrown/seeds/commit/83ba2e3868a9e7c4727c16a0e792a345b7feb520))
- *(cli)* Global -q/--quiet; --no-color, --verbose, --wrap listed not applicable (#49) ([4ae5e07](https://github.com/scbrown/seeds/commit/4ae5e078f174fc26aaaef8f3cee3ec427a223b2c))
- *(list,search)* --format csv with --fields (#50) ([bf50c3f](https://github.com/scbrown/seeds/commit/bf50c3fff2336688bdeae4b5c36b88e2a1bc3ab5))
- *(attribution)* [**breaking**] --agent-name/--harness/--model recorded as declared claims on the write (#51) ([dff64bb](https://github.com/scbrown/seeds/commit/dff64bbc2351173b9c862bc54e82ed2e78f00b24))
- *(version)* Say "(seeds)" and "tool":"seeds" so sd can be told from chmln/sd (#52) ([bc5ab99](https://github.com/scbrown/seeds/commit/bc5ab99a9a641ef36f56d7d9c92d5099c41327a4))
- *(list,search)* --long, --pretty and --tree text layouts (#53) ([64c1854](https://github.com/scbrown/seeds/commit/64c1854a518768679c1f7e8620787c9605c9040f))
- *(cli)* Dep list -t/--type; comments add --content made visible (#55) ([e2f387d](https://github.com/scbrown/seeds/commit/e2f387db179da3ceec30e205dcf0c31893f50877))
- *(blocked)* --detailed lists each blocker with title, priority and status (#56) ([5c60cca](https://github.com/scbrown/seeds/commit/5c60ccaa6d23820407a09fc0923f1a1aa04e59ed))
- *(delete)* --from-file reads ids; --hard listed not applicable (#57) ([a964e66](https://github.com/scbrown/seeds/commit/a964e66e764af639e55d8d321a8b74292c109e96))
- *(close)* --suggest-next lists the seeds the close fully unblocked (#58) ([e538dcc](https://github.com/scbrown/seeds/commit/e538dcc86d48053a781f4f7b4368139d20bd4967))
- *(config)* List --project / --user show only what that file sets (#59) ([f6e59ab](https://github.com/scbrown/seeds/commit/f6e59abe4017d9ca8511ff0a65f33b81b824c458))
- *(stats)* Recent activity with --activity / --no-activity / --activity-hours (#60) ([9614449](https://github.com/scbrown/seeds/commit/96144491150cb221abccb6c9b9f00c08fc380e6d))

### Documentation

- *(install)* The release recipe names the real seeds-ai-v* tags (#10) ([a085f3c](https://github.com/scbrown/seeds/commit/a085f3c55e796d76071ca3157c1b1128828d5965))
- *(verbs)* [**breaking**] List and search hide deferred seeds by default (#44) ([0cd89cd](https://github.com/scbrown/seeds/commit/0cd89cda99f39743bc61b6fdf8b9583e398296a8))

### Fixed

- *(create)* A remote create says created, not exists (#43) ([e6be6d5](https://github.com/scbrown/seeds/commit/e6be6d5ad44e72956195709f540fcfeb17fe6618))
- *(update)* Reparent refuses an existing parent loop instead of walking it forever (#47) ([83bb7d7](https://github.com/scbrown/seeds/commit/83bb7d74c6c5e6fc2e28b8f08784ec10707dc19a))
- *(cli)* --body is a visible alias on create, update and q (#54) ([8e0387d](https://github.com/scbrown/seeds/commit/8e0387d3b609fa63b0f5948905e6d95b79f398af))
- *(store)* [**breaking**] A write keeps facts this sd does not model, so an older sd never erases newer fields (#61) ([b0962f5](https://github.com/scbrown/seeds/commit/b0962f5e2697572f464d794eccc8402d258d366d))

### Miscellaneous

- *(seeds)* Close sd-p75, release parity landed (seeds-ai v0.0.2) (#11) ([314c2d9](https://github.com/scbrown/seeds/commit/314c2d938eab550aeecba0be7a2c7aa0604ed490))

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
