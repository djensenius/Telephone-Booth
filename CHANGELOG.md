# Changelog

## [0.7.0](https://github.com/djensenius/Telephone-Booth/compare/v0.6.3...v0.7.0) (2026-07-18)


### Features

* **pi:** set ALSA mixer levels from config at startup ([#108](https://github.com/djensenius/Telephone-Booth/issues/108)) ([6fac73d](https://github.com/djensenius/Telephone-Booth/commit/6fac73d3a7dc496a36faca36f9df27ec98b1a8f5))

## [0.6.3](https://github.com/djensenius/Telephone-Booth/compare/v0.6.2...v0.6.3) (2026-07-18)


### Bug Fixes

* durably spill buffered telemetry on shutdown so CallEnded survives restarts ([#106](https://github.com/djensenius/Telephone-Booth/issues/106)) ([3e2f8d6](https://github.com/djensenius/Telephone-Booth/commit/3e2f8d6f304244554dbb69cf103f9ae52184ad15)), closes [#104](https://github.com/djensenius/Telephone-Booth/issues/104)

## [0.6.2](https://github.com/djensenius/Telephone-Booth/compare/v0.6.1...v0.6.2) (2026-07-18)


### Bug Fixes

* poll GPIO levels + workflow permissions + RUSTSEC-2026-0204 ([#102](https://github.com/djensenius/Telephone-Booth/issues/102)) ([afdb136](https://github.com/djensenius/Telephone-Booth/commit/afdb136f064724664921d650803266430bab34ee))

## [0.6.1](https://github.com/djensenius/Telephone-Booth/compare/v0.6.0...v0.6.1) (2026-07-17)


### Bug Fixes

* **bin:** self-heal GPIO adapter after edge-stream loss ([#100](https://github.com/djensenius/Telephone-Booth/issues/100)) ([0260922](https://github.com/djensenius/Telephone-Booth/commit/02609227740dc738aa983d6b89a673f458a15aa6))

## [0.6.0](https://github.com/djensenius/Telephone-Booth/compare/v0.5.0...v0.6.0) (2026-07-16)


### Features

* **core:** log the decoded digit when a number is dialed ([#96](https://github.com/djensenius/Telephone-Booth/issues/96)) ([c0ec7e8](https://github.com/djensenius/Telephone-Booth/commit/c0ec7e89a7e2cf21c8073a11eafff75359a585e1))

## [0.5.0](https://github.com/djensenius/Telephone-Booth/compare/v0.4.3...v0.5.0) (2026-07-15)


### Features

* systemd watchdog keepalives + read-only hardware monitor TUI ([#95](https://github.com/djensenius/Telephone-Booth/issues/95)) ([e016d44](https://github.com/djensenius/Telephone-Booth/commit/e016d4496632f1989956d91439d61d420ce36a4f))


### Documentation

* add artist statement and bio for St. Anne's installation ([#92](https://github.com/djensenius/Telephone-Booth/issues/92)) ([8cbd51b](https://github.com/djensenius/Telephone-Booth/commit/8cbd51bfbb09ffaa28e95ee1de3449beff23d1e5))
* GPIO screw-terminal HAT wiring, --tui monitor guide, watchdog behaviour. ([e016d44](https://github.com/djensenius/Telephone-Booth/commit/e016d4496632f1989956d91439d61d420ce36a4f))

## [0.4.3](https://github.com/djensenius/Telephone-Booth/compare/v0.4.2...v0.4.3) (2026-06-29)


### Bug Fixes

* **deps:** bump quinn-proto and anyhow to patch RUSTSEC advisories ([#90](https://github.com/djensenius/Telephone-Booth/issues/90)) ([e37c76c](https://github.com/djensenius/Telephone-Booth/commit/e37c76cea204c866761c84978c129dcca5e6bcd0))

## [0.4.2](https://github.com/djensenius/Telephone-Booth/compare/v0.4.1...v0.4.2) (2026-06-09)


### Documentation

* add payphone hardware reference + handset/wiring detail ([#83](https://github.com/djensenius/Telephone-Booth/issues/83)) ([453828a](https://github.com/djensenius/Telephone-Booth/commit/453828abb032d903f8fa9bbc68d9668cd3e10add))

## [0.4.1](https://github.com/djensenius/Telephone-Booth/compare/v0.4.0...v0.4.1) (2026-06-05)


### Bug Fixes

* **packaging:** wait for tailscaled ready before tailscale serve ([#79](https://github.com/djensenius/Telephone-Booth/issues/79)) ([edfec52](https://github.com/djensenius/Telephone-Booth/commit/edfec52691f9a5529aa90d2fc51f6d52ee351888))

## [0.4.0](https://github.com/djensenius/Telephone-Booth/compare/v0.3.3...v0.4.0) (2026-05-31)


### Features

* expose per-interface IP addresses in system snapshots ([#77](https://github.com/djensenius/Telephone-Booth/issues/77)) ([f588f7c](https://github.com/djensenius/Telephone-Booth/commit/f588f7c9d0d66f09b30952c18c38dead13c1fdd7))

## [0.3.3](https://github.com/djensenius/Telephone-Booth/compare/v0.3.2...v0.3.3) (2026-05-27)


### Bug Fixes

* stamp client version on telemetry events and system snapshots ([#75](https://github.com/djensenius/Telephone-Booth/issues/75)) ([6792847](https://github.com/djensenius/Telephone-Booth/commit/6792847c37b1bc5436b1cde502e56cd740d07d4a))

## [0.3.2](https://github.com/djensenius/Telephone-Booth/compare/v0.3.1...v0.3.2) (2026-05-27)


### Bug Fixes

* **release:** drop component/package-name so release-please can tag merged PRs ([#72](https://github.com/djensenius/Telephone-Booth/issues/72)) ([3c7259e](https://github.com/djensenius/Telephone-Booth/commit/3c7259e14a3695c27788cc8ecb3f7ab7e9fa7a22))
* **release:** explicit publish→publish-apt dispatch chain ([#74](https://github.com/djensenius/Telephone-Booth/issues/74)) ([74ef222](https://github.com/djensenius/Telephone-Booth/commit/74ef222e33b00dcffce3d747b584ee4a8b3f08e1))

## [0.3.1](https://github.com/djensenius/Telephone-Booth/compare/v0.3.0...v0.3.1) (2026-05-26)


### Bug Fixes

* **simulator:** pair the TUI with a working web simulator and harden the headless guard ([#70](https://github.com/djensenius/Telephone-Booth/issues/70)) ([f4eed7d](https://github.com/djensenius/Telephone-Booth/commit/f4eed7db1e6012ae8157573894e0ce85704c7a34))

## [0.3.0](https://github.com/djensenius/Telephone-Booth/compare/v0.2.0...v0.3.0) (2026-05-26)


### Features

* operator sync resilience — heartbeat, reconnect burst, durable event spool ([#68](https://github.com/djensenius/Telephone-Booth/issues/68)) ([2a436ca](https://github.com/djensenius/Telephone-Booth/commit/2a436ca305b2b649a8e7decb7a7ebb9f5ad5f3cf))

## [0.2.0](https://github.com/djensenius/Telephone-Booth/compare/v0.1.1...v0.2.0) (2026-05-26)


### Features

* **debug:** add simulator remote access via tmux and web UI ([#66](https://github.com/djensenius/Telephone-Booth/issues/66)) ([5074a85](https://github.com/djensenius/Telephone-Booth/commit/5074a8505d1c8b672fd0da65ff338afed7222e61))


### Documentation

* expand Tailscale setup and environment variable reference ([#62](https://github.com/djensenius/Telephone-Booth/issues/62)) ([3f07c07](https://github.com/djensenius/Telephone-Booth/commit/3f07c07e2153a80373091405960e726a8fddc480))
* fix hostname consistency and clarify .local vs .lan ([#65](https://github.com/djensenius/Telephone-Booth/issues/65)) ([f19b616](https://github.com/djensenius/Telephone-Booth/commit/f19b616c1b2c38c961f1fe470f65e3cc465c5eab))
* **tailscale:** fix incorrect SSH behavior documentation ([#64](https://github.com/djensenius/Telephone-Booth/issues/64)) ([594bd23](https://github.com/djensenius/Telephone-Booth/commit/594bd2341549b99aaab4c29b5cd6dd221c700b19))

## [0.1.1](https://github.com/djensenius/Telephone-Booth/compare/v0.1.0...v0.1.1) (2026-05-26)


### Bug Fixes

* **ci:** resolve --pubkey-output to absolute path before cd ([#53](https://github.com/djensenius/Telephone-Booth/issues/53)) ([6678da2](https://github.com/djensenius/Telephone-Booth/commit/6678da2a44f72041241506f189200c3c39e9c495))
* **ci:** tolerate gpg --import exit=2 in publish-apt ([#52](https://github.com/djensenius/Telephone-Booth/issues/52)) ([b4db6e8](https://github.com/djensenius/Telephone-Booth/commit/b4db6e884563bb2d573455bb9e5360162c6f2735))
* **ci:** unblock cargo-deny audit on main ([#59](https://github.com/djensenius/Telephone-Booth/issues/59)) ([fdcfa11](https://github.com/djensenius/Telephone-Booth/commit/fdcfa1185db1b3a3c8a416aa93ff0a0b8265542c))
* **observability:** include runtime mode in booth status and system snapshots ([#57](https://github.com/djensenius/Telephone-Booth/issues/57)) ([9321c4b](https://github.com/djensenius/Telephone-Booth/commit/9321c4b5b715a3d58e590cea336a12dabbd5441f))
* **packaging:** ship simulator+mock in .deb and add [runtime] autostart config ([#56](https://github.com/djensenius/Telephone-Booth/issues/56)) ([b1271ad](https://github.com/djensenius/Telephone-Booth/commit/b1271ad764445390f13f4208787a4f87aae48586))


### Documentation

* **adr:** note gh-pages signed-commits ruleset exclusion ([#54](https://github.com/djensenius/Telephone-Booth/issues/54)) ([26b755d](https://github.com/djensenius/Telephone-Booth/commit/26b755dfd30e9c7997cec946e1470671087e08a3))
* **observability:** document Prometheus deployment topology ([#55](https://github.com/djensenius/Telephone-Booth/issues/55)) ([ae40a01](https://github.com/djensenius/Telephone-Booth/commit/ae40a014902b86a95eab407f49220bc3e06a81f5))

## [0.1.0](https://github.com/djensenius/Telephone-Booth/compare/v0.1.0...v0.1.0) (2026-05-26)


### Features

* **audio:** prefer local operator recordings ([bdd6be8](https://github.com/djensenius/Telephone-Booth/commit/bdd6be8f36f169fe66061d922c03fb133f27d320))
* **booth-bin:** add --simulator TUI; make audio/operator cross-platform ([#1](https://github.com/djensenius/Telephone-Booth/issues/1)) ([98a1894](https://github.com/djensenius/Telephone-Booth/commit/98a1894ff919fc8463dcea4dcffc19d433b78366))
* **metrics:** add booth-metrics crate and observability telemetry variants ([#3](https://github.com/djensenius/Telephone-Booth/issues/3)) ([c8e4782](https://github.com/djensenius/Telephone-Booth/commit/c8e47821eaa5d9cfe77687ad41075ee40b8d2598))
* **observability:** /metrics endpoint, vmagent packaging, ADR 0006, dashboards ([#5](https://github.com/djensenius/Telephone-Booth/issues/5)) ([e1c8277](https://github.com/djensenius/Telephone-Booth/commit/e1c82778873b348dcc497c97aceaeca94bc401de))
* **runtime:** wire observability event forwarder, system pusher, and /v1/system route ([#4](https://github.com/djensenius/Telephone-Booth/issues/4)) ([e41d3cb](https://github.com/djensenius/Telephone-Booth/commit/e41d3cb05b31552fe369f5eebf35f8b3aa1b3fda))
* signed APT repo + automated release pipeline ([#47](https://github.com/djensenius/Telephone-Booth/issues/47)) ([4472260](https://github.com/djensenius/Telephone-Booth/commit/4472260e3b7efe268e03c0746dfaa72d5babef7d))


### Bug Fixes

* **bin:** make recording metadata durable and crash-recoverable ([#30](https://github.com/djensenius/Telephone-Booth/issues/30)) ([eace670](https://github.com/djensenius/Telephone-Booth/commit/eace670b78264de72abc1f5c621acf30fb1778bb))
* **bin:** split effect_task so operator work cannot block audio/pulse effects ([#37](https://github.com/djensenius/Telephone-Booth/issues/37)) ([c60cd14](https://github.com/djensenius/Telephone-Booth/commit/c60cd146e29db1629351e6f55719be65f3b83231))
* **bin:** upgrade ratatui 0.29→0.30 to remove unmaintained paste crate ([#40](https://github.com/djensenius/Telephone-Booth/issues/40)) ([5dd98cc](https://github.com/djensenius/Telephone-Booth/commit/5dd98cc4722fb421ea838255850b4879f260cfb5)), closes [#9](https://github.com/djensenius/Telephone-Booth/issues/9)
* **bin:** validate timeout/backoff/observability bounds at startup ([#29](https://github.com/djensenius/Telephone-Booth/issues/29)) ([9c76a1a](https://github.com/djensenius/Telephone-Booth/commit/9c76a1a6b4554616feef01046b9b732ec6b7b3d9))
* **ci:** make release-please work with workspace.package.version ([#49](https://github.com/djensenius/Telephone-Booth/issues/49)) ([ab298ab](https://github.com/djensenius/Telephone-Booth/commit/ab298abbe77c4aa04d7c582887f0887da368c827))
* **ci:** pin all GitHub Actions to commit SHAs and tighten publish permissions ([#39](https://github.com/djensenius/Telephone-Booth/issues/39)) ([8456857](https://github.com/djensenius/Telephone-Booth/commit/84568576e9e671c6fc5b403943fef3cb726e87f4))
* **debug:** ensure listener tasks shut down with the runtime ([#27](https://github.com/djensenius/Telephone-Booth/issues/27)) ([8d693bf](https://github.com/djensenius/Telephone-Booth/commit/8d693bffeab513d12f89caad7612e31326864a37))
* **debug:** fail closed when no bearer token is configured ([#41](https://github.com/djensenius/Telephone-Booth/issues/41)) ([efceb7e](https://github.com/djensenius/Telephone-Booth/commit/efceb7e52a35f05633cd6189dedb4de25d1e470f))
* **debug:** make LAN listener opt-in and require strong token for external binds ([#36](https://github.com/djensenius/Telephone-Booth/issues/36)) ([3183774](https://github.com/djensenius/Telephone-Booth/commit/3183774249d1ace84c8082a2e67600d74c19d10d))
* **hal:** pass real upload metadata instead of dummy values ([#34](https://github.com/djensenius/Telephone-Booth/issues/34)) ([7392645](https://github.com/djensenius/Telephone-Booth/commit/73926457899f53543c6bfe663ac786adf20f6009)), closes [#17](https://github.com/djensenius/Telephone-Booth/issues/17)
* **hal:** redact sensitive URLs in audio/operator error messages ([#35](https://github.com/djensenius/Telephone-Booth/issues/35)) ([0c3c7fa](https://github.com/djensenius/Telephone-Booth/commit/0c3c7fa0363ca657e20b8b8fdc670300a1a703f8))
* **observability:** prevent duplicate forwarding of synthetic call events ([#28](https://github.com/djensenius/Telephone-Booth/issues/28)) ([ce45154](https://github.com/djensenius/Telephone-Booth/commit/ce451541be3afca2f34821be553384240bcbd8ef)), closes [#23](https://github.com/djensenius/Telephone-Booth/issues/23)
* **pi:** add bounded cleanup for PiAudioSource.finished metadata ([#32](https://github.com/djensenius/Telephone-Booth/issues/32)) ([3e88798](https://github.com/djensenius/Telephone-Booth/commit/3e88798446191c9c07b1f1297b885bd33eb6c49e))
* **pi:** align uploads with messages API ([#46](https://github.com/djensenius/Telephone-Booth/issues/46)) ([9696816](https://github.com/djensenius/Telephone-Booth/commit/9696816df24d299a7e9e742d7845a3a36a8d7220))
* **pi:** bound audio download memory and stream recording uploads ([#38](https://github.com/djensenius/Telephone-Booth/issues/38)) ([b9e3a5f](https://github.com/djensenius/Telephone-Booth/commit/b9e3a5f0367d92648d3c9ec318d2aa8bd9de1253))
* **pi:** correct multi-channel recording duration calculation ([#31](https://github.com/djensenius/Telephone-Booth/issues/31)) ([504531d](https://github.com/djensenius/Telephone-Booth/commit/504531d318ba04afdae71c8fe338cb7c5edd63cf))
* **pi:** harden remote audio URL fetches against SSRF ([#43](https://github.com/djensenius/Telephone-Booth/issues/43)) ([11e2439](https://github.com/djensenius/Telephone-Booth/commit/11e2439e2540282a241d8bac8d7069ae26b6bd59)), closes [#12](https://github.com/djensenius/Telephone-Booth/issues/12)
* **pi:** replace GPIO unbounded channels with bounded/coalescing queues ([#33](https://github.com/djensenius/Telephone-Booth/issues/33)) ([b93626d](https://github.com/djensenius/Telephone-Booth/commit/b93626dc990918bdd28c835191d6aa073857f4f7))
* **pi:** validate presigned upload URLs before PUT ([#42](https://github.com/djensenius/Telephone-Booth/issues/42)) ([bfdfb6e](https://github.com/djensenius/Telephone-Booth/commit/bfdfb6e2263351c53af3c1e8eb56d91354d555fd))


### Documentation

* add Raspberry Pi from-scratch setup guide ([#45](https://github.com/djensenius/Telephone-Booth/issues/45)) ([07be355](https://github.com/djensenius/Telephone-Booth/commit/07be35510dca402e1017674eca1b3460cfebb17c))
* add related repositories section to README ([#7](https://github.com/djensenius/Telephone-Booth/issues/7)) ([a26ce5f](https://github.com/djensenius/Telephone-Booth/commit/a26ce5f6b8f04e870532bac49bb68446293acaef))
* correct observability env var names to single underscore ([#6](https://github.com/djensenius/Telephone-Booth/issues/6)) ([d14b1ae](https://github.com/djensenius/Telephone-Booth/commit/d14b1aeb810e7ee0163f8bb4b3f92dcae006f0cd))


### Miscellaneous Chores

* **packaging:** install production APT signing pubkey ([#48](https://github.com/djensenius/Telephone-Booth/issues/48)) ([706199e](https://github.com/djensenius/Telephone-Booth/commit/706199ea3296bf634529c2ba91689a7d7f7594d1))

## Changelog

All notable changes to this project are documented in this file. The format
is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Entries below this point are maintained automatically by
[release-please](https://github.com/googleapis/release-please) from
Conventional Commits landing on `main`.
