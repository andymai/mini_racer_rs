# Changelog

## [1.3.0](https://github.com/andymai/mini_racer_rs/compare/v1.2.0...v1.3.0) (2026-10-23)


### Features

* add MiniRacerRs.counters for per-context activity counts ([#126](https://github.com/andymai/mini_racer_rs/issues/126)) ([d482a3f](https://github.com/andymai/mini_racer_rs/commit/d482a3fb9fb4ac5ba3c077e6d2deffe25402ef3d))

## [1.2.0](https://github.com/andymai/mini_racer_rs/compare/v1.1.1...v1.2.0) (2026-10-22)


### Features

* **platforms:** native gems and CI for musl Linux ([#117](https://github.com/andymai/mini_racer_rs/issues/117)) ([5b565f2](https://github.com/andymai/mini_racer_rs/commit/5b565f2e7e5f2ef5f502c2ddf7d38fcbdcd68e35))

## [1.1.1](https://github.com/andymai/mini_racer_rs/compare/v1.1.0...v1.1.1) (2026-10-21)


### Performance

* **ext:** park at once for contexts whose answers outlast the spin budget ([#105](https://github.com/andymai/mini_racer_rs/issues/105)) ([66af89b](https://github.com/andymai/mini_racer_rs/commit/66af89b8d22ebe0c4dd28c877155131e3d7e5738))
* **wire:** add criterion benches for decoding, writing and transcoding ([#106](https://github.com/andymai/mini_racer_rs/issues/106)) ([42fc2b5](https://github.com/andymai/mini_racer_rs/commit/42fc2b50dba2a4a6437456293fbe61aa5d29aef8))

## [1.1.0](https://github.com/andymai/mini_racer_rs/compare/v1.0.6...v1.1.0) (2026-10-19)


### Features

* **source-maps:** map runtime error frames back to original sources ([#97](https://github.com/andymai/mini_racer_rs/issues/97)) ([d92bb05](https://github.com/andymai/mini_racer_rs/commit/d92bb0505ecd02deb1a0b62aa952268a5bdfb689))

## [1.0.6](https://github.com/andymai/mini_racer_rs/compare/v1.0.5...v1.0.6) (2026-10-19)


### Bug Fixes

* **decode:** raise SystemStackError when a decode runs out of stack ([#95](https://github.com/andymai/mini_racer_rs/issues/95)) ([7d21103](https://github.com/andymai/mini_racer_rs/commit/7d2110338352c0259eef2e4f22f439a4875765eb))

## [1.0.5](https://github.com/andymai/mini_racer_rs/compare/v1.0.4...v1.0.5) (2026-10-19)


### Performance

* **encode:** send Ruby strings to V8 in the form V8 stores them ([#93](https://github.com/andymai/mini_racer_rs/issues/93)) ([62f9c85](https://github.com/andymai/mini_racer_rs/commit/62f9c851917cb0ce7d760b5fc545aa014df0a53b))

## [1.0.4](https://github.com/andymai/mini_racer_rs/compare/v1.0.3...v1.0.4) (2026-10-19)


### Performance

* **engine:** share a large source from its first evaluation ([#91](https://github.com/andymai/mini_racer_rs/issues/91)) ([31425e1](https://github.com/andymai/mini_racer_rs/commit/31425e1fc72083fd1c6564d5f50d30b8e2d746aa))

## [1.0.3](https://github.com/andymai/mini_racer_rs/compare/v1.0.2...v1.0.3) (2026-10-18)


### Performance

* **decode:** intern hash keys, build each Hash at once, and transcode strings in one pass ([#87](https://github.com/andymai/mini_racer_rs/issues/87)) ([220bfe7](https://github.com/andymai/mini_racer_rs/commit/220bfe7e6a9c8fa5a38befefcb4995756c377944))

## [1.0.2](https://github.com/andymai/mini_racer_rs/compare/v1.0.1...v1.0.2) (2026-10-18)


### Performance

* **engine:** share one copy of a large repeated source across contexts ([#85](https://github.com/andymai/mini_racer_rs/issues/85)) ([9fe050a](https://github.com/andymai/mini_racer_rs/commit/9fe050a7a612314264e255f9bcc40ca146e93571))

## [1.0.1](https://github.com/andymai/mini_racer_rs/compare/v1.0.0...v1.0.1) (2026-10-18)


### Performance

* **engine:** build small call arguments in the runner and write primitive results directly ([#83](https://github.com/andymai/mini_racer_rs/issues/83)) ([76afcbd](https://github.com/andymai/mini_racer_rs/commit/76afcbd7bb5df36da6797b95d8dae7049ee80f53))

## 1.0.0 (2026-10-18)


### Features

* **engine:** add async promise dispatch and V8 task queues ([#37](https://github.com/andymai/mini_racer_rs/issues/37)) ([c23986a](https://github.com/andymai/mini_racer_rs/commit/c23986a407e94864dfa35343cd7579e2e99c905c))
* **engine:** add heap stats and snapshot support ([#39](https://github.com/andymai/mini_racer_rs/issues/39)) ([38072fb](https://github.com/andymai/mini_racer_rs/commit/38072fba06c249a9a7e72d39b97a3f4fb979cb62))
* **engine:** carry idle single-threaded contexts across fork ([#27](https://github.com/andymai/mini_racer_rs/issues/27)) ([c1251bc](https://github.com/andymai/mini_racer_rs/commit/c1251bcce60eb05627d616564cae73df90a622d7))
* **engine:** dispatch through the channel and termination cell ([#14](https://github.com/andymai/mini_racer_rs/issues/14)) ([77c05bd](https://github.com/andymai/mini_racer_rs/commit/77c05bd77c29c063561078edd11903b54fb3c6db))
* **engine:** enforce max_memory after garbage collection ([#41](https://github.com/andymai/mini_racer_rs/issues/41)) ([ba3dced](https://github.com/andymai/mini_racer_rs/commit/ba3dced1e862b0964d6d6a3a02d448f6a3de1347))
* **engine:** support garbage collection after idle ([#43](https://github.com/andymai/mini_racer_rs/issues/43)) ([a835159](https://github.com/andymai/mini_racer_rs/commit/a835159799e3cc918d486f4a9f0288a67f96f6fb))
* **engine:** support verbose exceptions ([#45](https://github.com/andymai/mini_racer_rs/issues/45)) ([5adf6b6](https://github.com/andymai/mini_racer_rs/commit/5adf6b64b52235822657359aeb0545f674759c2a))
* **ext:** attach ruby callables as javascript functions with context#attach ([#20](https://github.com/andymai/mini_racer_rs/issues/20)) ([1c84836](https://github.com/andymai/mini_racer_rs/commit/1c8483680518f8346aa8ee58f285b32d10d7c9f7))
* **ext:** build, load and warm up startup snapshots with mini_racer::snapshot ([#21](https://github.com/andymai/mini_racer_rs/issues/21)) ([441a8f5](https://github.com/andymai/mini_racer_rs/commit/441a8f58f856099b2c2771cbd5b3937b804d0ed2))
* **ext:** call javascript functions from ruby with context#call ([#18](https://github.com/andymai/mini_racer_rs/issues/18)) ([bf144c6](https://github.com/andymai/mini_racer_rs/commit/bf144c6e38c2848b4e69267f89b0c804189708a0))
* **ext:** context low_memory_notification, answered after the collection as in mini_racer ([#26](https://github.com/andymai/mini_racer_rs/issues/26)) ([d9b11e2](https://github.com/andymai/mini_racer_rs/commit/d9b11e28837e9727f85ebe95d023355c8d55f258))
* **ext:** park only the waiting fiber under a fiber scheduler ([#25](https://github.com/andymai/mini_racer_rs/issues/25)) ([6d16553](https://github.com/andymai/mini_racer_rs/commit/6d16553a13b2100951e2dcc41324d32bd4ff57c1))
* **ext:** return every javascript value from eval as mini_racer does ([#17](https://github.com/andymai/mini_racer_rs/issues/17)) ([8c59e07](https://github.com/andymai/mini_racer_rs/commit/8c59e07cb2abbfe92d9197bf64d653419fdbc8f6))
* **sync:** turn-taking channel and termination cell with loom models ([#9](https://github.com/andymai/mini_racer_rs/issues/9)) ([353abb7](https://github.com/andymai/mini_racer_rs/commit/353abb763b7e3407c6da869db5d04a63950d91cb))
* **wire:** decode every V8 wire tag and write mini_racer's encoding ([#16](https://github.com/andymai/mini_racer_rs/issues/16)) ([24d8a85](https://github.com/andymai/mini_racer_rs/commit/24d8a85243c9c4acf515ec12fa03f96211f39a8c))


### Bug Fixes

* **engine:** attach under a pending stop raises mini_racer's internal error ([#47](https://github.com/andymai/mini_racer_rs/issues/47)) ([e68d8ce](https://github.com/andymai/mini_racer_rs/commit/e68d8ced83faa122e77e667c754bfb872d2436e4))
* **engine:** model mini_racer's one-shot stop interrupt ([#49](https://github.com/andymai/mini_racer_rs/issues/49)) ([649ff1a](https://github.com/andymai/mini_racer_rs/commit/649ff1a40b94564bd5d07141e1fa526c23006fa2))
* **engine:** preserve callback exceptions during termination ([#42](https://github.com/andymai/mini_racer_rs/issues/42)) ([3a92918](https://github.com/andymai/mini_racer_rs/commit/3a92918460b71cc5cf564221c40b839442e34721))
* **engine:** preserve results after late timeouts ([#38](https://github.com/andymai/mini_racer_rs/issues/38)) ([1d8038d](https://github.com/andymai/mini_racer_rs/commit/1d8038d4ba13859552c1c8dac7bf2595ef28b069))
* **engine:** raise disposed error for in-flight work ([#40](https://github.com/andymai/mini_racer_rs/issues/40)) ([fbd37f1](https://github.com/andymai/mini_racer_rs/commit/fbd37f1dde7288658e190bb2c6d5976942980016))
* **fork:** hold the single-threaded platform's worker across a fork ([#70](https://github.com/andymai/mini_racer_rs/issues/70)) ([78c94e3](https://github.com/andymai/mini_racer_rs/commit/78c94e3685b35a5b90f37c02e336de2e25feccd4))
* **fork:** quiesce runners and hold V8's workers across a multi-threaded fork ([#68](https://github.com/andymai/mini_racer_rs/issues/68)) ([95b69bc](https://github.com/andymai/mini_racer_rs/commit/95b69bc625c248e0af263353b3e413540726672c))


### Performance

* **decode:** decode arrays of numbers in runs ([#72](https://github.com/andymai/mini_racer_rs/issues/72)) ([2513a48](https://github.com/andymai/mini_racer_rs/commit/2513a481f3c84a026091b1f77dad41b299cdf8c2))
* **engine:** compile ExecJS calls and react-rails renders once per context ([#77](https://github.com/andymai/mini_racer_rs/issues/77)) ([774be16](https://github.com/andymai/mini_racer_rs/commit/774be1609816d7027986578a594f03fb00e0de61))
* **engine:** compile large scripts from a process-wide code cache ([#69](https://github.com/andymai/mini_racer_rs/issues/69)) ([ca3ce70](https://github.com/andymai/mini_racer_rs/commit/ca3ce705febb3b92c5f62879faffbd5e725fee8b))
* **engine:** hand freed malloc memory back after a large eval ([#67](https://github.com/andymai/mini_racer_rs/issues/67)) ([81b21cc](https://github.com/andymai/mini_racer_rs/commit/81b21ccb68d9f0d3c69c7f9594d1f3a027d39bbf))
* **engine:** run V8's worker tasks on a pool each process owns ([#75](https://github.com/andymai/mini_racer_rs/issues/75)) ([a090a9f](https://github.com/andymai/mini_racer_rs/commit/a090a9fb6dfe3ae9e42e864e8a084bd67aaa7485))
* **engine:** share each snapshot's blob across its contexts and refuse blobs V8 would abort on ([#79](https://github.com/andymai/mini_racer_rs/issues/79)) ([470da4a](https://github.com/andymai/mini_racer_rs/commit/470da4a2f73791c0f0610a0c41b102dc7e31fcfd))
* **ext:** decode arrays and objects in batches under one rb_protect each ([#23](https://github.com/andymai/mini_racer_rs/issues/23)) ([77f44ab](https://github.com/andymai/mini_racer_rs/commit/77f44ab1d09b49944dd480bdc6e9dff9781ad366))
* **wire:** copy ascii spans in bulk when transcoding latin-1 text ([#19](https://github.com/andymai/mini_racer_rs/issues/19)) ([c7e4a7a](https://github.com/andymai/mini_racer_rs/commit/c7e4a7acbeac56af80422ce8774c6e43035232a9))
