# Changelog

## [0.3.0](https://github.com/vymalo/another-agentic-platform/compare/v0.2.0...v0.3.0) (2026-11-28)


### ⚠ BREAKING CHANGES

* **api:** the Platform API takes everything from its file (ANR-970) ([#196](https://github.com/vymalo/another-agentic-platform/issues/196))

### Features

* **api:** a read-only owner permission for the staff service (ANR-1118, AD-072) ([#153](https://github.com/vymalo/another-agentic-platform/issues/153)) ([427bffc](https://github.com/vymalo/another-agentic-platform/commit/427bffcc5eaa1699e9b5dd4afa2ac58a57069ca2))
* **api:** a workload certificate lives about a day, and a longer one is warned about (ANR-1374) ([#203](https://github.com/vymalo/another-agentic-platform/issues/203)) ([e2c6aaf](https://github.com/vymalo/another-agentic-platform/commit/e2c6aaf082e6eb6fd4ec71978b248cfd9f0b618d))
* **api:** the Platform API takes everything from its file (ANR-970) ([#196](https://github.com/vymalo/another-agentic-platform/issues/196)) ([e718426](https://github.com/vymalo/another-agentic-platform/commit/e718426e4edd91b7e43b1fb495035f6e0a195748))
* **controller:** a missing referent has its own reason (ANR-1159) ([b73f1ad](https://github.com/vymalo/another-agentic-platform/commit/b73f1ad2df68e47d0081d91a2191e22595dfae72))
* **controller:** the operator wake path, sleep stored in status.sleep (ANR-1067, AD-058 P3a-1) ([#138](https://github.com/vymalo/another-agentic-platform/issues/138)) ([02f6210](https://github.com/vymalo/another-agentic-platform/commit/02f621096638343a8f1eb9fb303931a3e00efba1))
* **controller:** the Service switch around a sleeping agent (ANR-1182, AD-058 P3a-2) ([#219](https://github.com/vymalo/another-agentic-platform/issues/219)) ([187bc6b](https://github.com/vymalo/another-agentic-platform/commit/187bc6b7081a7682557f60c97685b0a75bc47206))
* **crd:** release channels on AgentService, and status.revisions (ANR-965) ([f8638a9](https://github.com/vymalo/another-agentic-platform/commit/f8638a906e3504ac7b5bfb313721bc37ca696e32))
* **dashboards:** the operator and API dashboards as code (ANR-1423) ([#218](https://github.com/vymalo/another-agentic-platform/issues/218)) ([eeac8b8](https://github.com/vymalo/another-agentic-platform/commit/eeac8b87a5b61d36e9bd3b1847c962b737896146))
* **domain:** render an adam agent's activity listener, reachable by the operator only (ANR-1030a, AD-058) ([#141](https://github.com/vymalo/another-agentic-platform/issues/141)) ([39e911e](https://github.com/vymalo/another-agentic-platform/commit/39e911eb70c625a5a30f9ce3e9707c2a3757ef76))
* **e2e:** a kind end-to-end of the pod-minute meter (ANR-1447) ([#229](https://github.com/vymalo/another-agentic-platform/issues/229)) ([4967ddc](https://github.com/vymalo/another-agentic-platform/commit/4967ddccb9bc6bfbd3724f6cce6afac432ddd8fa))
* **forms:** leave the activator's transient wake-at out of the YAML export (ANR-1352) ([#179](https://github.com/vymalo/another-agentic-platform/issues/179)) ([5247617](https://github.com/vymalo/another-agentic-platform/commit/5247617a0b8642e21aba3518e763239ad2c27b5b))
* **meter:** attribute an agent pod by its owner chain ([9f6730e](https://github.com/vymalo/another-agentic-platform/commit/9f6730e9477a33494ec748b2d894a6fa1edcdd26))
* **metering:** export the sink's counters and let the chart trust a private CA (ANR-1194) ([#217](https://github.com/vymalo/another-agentic-platform/issues/217)) ([0710f92](https://github.com/vymalo/another-agentic-platform/commit/0710f92119707bf91a7ad4a09f2f3767f49f51fe))
* **meter:** persist a vanished pod's facts as tombstones in a ConfigMap (ANR-1198) ([#140](https://github.com/vymalo/another-agentic-platform/issues/140)) ([59666cd](https://github.com/vymalo/another-agentic-platform/commit/59666cdee709b6e1c2716acfb0815ae0b759d869))
* **meter:** track the collector's acknowledgements of pod-minute events (ANR-1151, part 1) ([#139](https://github.com/vymalo/another-agentic-platform/issues/139)) ([275a01c](https://github.com/vymalo/another-agentic-platform/commit/275a01c8289cd9107605e278104ef3e15389eada))
* **operator:** wire the activity source so an idle adam agent can sleep (ANR-1030b, AD-058) ([#145](https://github.com/vymalo/another-agentic-platform/issues/145)) ([7488199](https://github.com/vymalo/another-agentic-platform/commit/74881995a7948b1a29429c984562c8b7f1074643))
* **operator:** wire the pod-minute meter into operator run, with the broker's account source and the chart (ANR-1103) ([#137](https://github.com/vymalo/another-agentic-platform/issues/137)) ([cb66d28](https://github.com/vymalo/another-agentic-platform/commit/cb66d28786a9b788eacfcbfdb31c76917a9e8d75))
* **release:** publish aapctl's signed binaries and its Homebrew formula ([51e31bc](https://github.com/vymalo/another-agentic-platform/commit/51e31bc9b4382a628c2387d1c6586d1feb05ef4b))
* **run-pods:** move the parity goldens and the pins to adam-rs b5d1c7e ([b75b2e6](https://github.com/vymalo/another-agentic-platform/commit/b75b2e6f97ce8da0d5d40809e200946984a67ab8))
* **tokens:** re-read a rotated token instead of restarting the operator (ANR-1192) ([#223](https://github.com/vymalo/another-agentic-platform/issues/223)) ([61afba3](https://github.com/vymalo/another-agentic-platform/commit/61afba3113204d3f7f0d3cf6b5d56318739b42e5))
* **usage:** an agent pod's account from its AgentService (ANR-1100) ([#152](https://github.com/vymalo/another-agentic-platform/issues/152)) ([25fe854](https://github.com/vymalo/another-agentic-platform/commit/25fe854e04396952f6b1bb9124304d0a454613e0))


### Bug Fixes

* **activity:** a typed refusal and a closed cause for the poll warn (ANR-1303) ([#180](https://github.com/vymalo/another-agentic-platform/issues/180)) ([433e6b4](https://github.com/vymalo/another-agentic-platform/commit/433e6b4b5ab649ea48018460c8b863800230db44))
* **auth-workload:** the lifetime test names the fixture's identity (ANR-1374 follow-up) ([#226](https://github.com/vymalo/another-agentic-platform/issues/226)) ([f0b29bf](https://github.com/vymalo/another-agentic-platform/commit/f0b29bf957449ca259c3929b5a271355228bb741))
* **chart:** name the values that make the tool probe fit the pass (ANR-1080) ([#149](https://github.com/vymalo/another-agentic-platform/issues/149)) ([3f36186](https://github.com/vymalo/another-agentic-platform/commit/3f3618613a90dccdd9e06ca2919cd9b3689930d5))
* **chart:** one rule for every list of NetworkPolicy peers (ANR-1339) ([#186](https://github.com/vymalo/another-agentic-platform/issues/186)) ([0dd4968](https://github.com/vymalo/another-agentic-platform/commit/0dd4968d4edab55081ef95300667ecbc29034324))
* **chart:** refuse a namespaceSelector that selects every namespace (ANR-1191) ([#174](https://github.com/vymalo/another-agentic-platform/issues/174)) ([1a36acd](https://github.com/vymalo/another-agentic-platform/commit/1a36acd18eab55715cc15b288d67425324fe088a))
* **chart:** refuse a tuning or leaderElection value the file cannot hold ([176fa33](https://github.com/vymalo/another-agentic-platform/commit/176fa338ea5d9f7a33a3bd92eb9b84eb21cd93d8))
* **ci:** quote the docs-site step name YAML read as a mapping ([#172](https://github.com/vymalo/another-agentic-platform/issues/172)) ([6d3a2d3](https://github.com/vymalo/another-agentic-platform/commit/6d3a2d343367ef5fa8c3b0693b5b120e3790f938))
* **ci:** read each skip guard as one construct, folds included (ANR-1323 review) ([#175](https://github.com/vymalo/another-agentic-platform/issues/175)) ([f5f9e99](https://github.com/vymalo/another-agentic-platform/commit/f5f9e99b78e91b29043ab523fc241fa5377bb6c1))
* **ci:** the guard tests unset CDPATH instead of SC1007's false positive (ANR-1323, ANR-1326) ([#185](https://github.com/vymalo/another-agentic-platform/issues/185)) ([88f9e86](https://github.com/vymalo/another-agentic-platform/commit/88f9e8673327880c5ee731a895efa7d965e65da7))
* **ci:** the skip-guard check scans .yaml workflows too (ANR-1323 review) ([#173](https://github.com/vymalo/another-agentic-platform/issues/173)) ([3132796](https://github.com/vymalo/another-agentic-platform/commit/3132796554b5872891fcc38c72d0f2b210cd6a97))
* **controller:** a mark dropped before its claim writes no record (ANR-1199) ([#178](https://github.com/vymalo/another-agentic-platform/issues/178)) ([0919986](https://github.com/vymalo/another-agentic-platform/commit/09199864062104e41d6fefb5a28bd4b31d656129))
* **controller:** the meter knows a run pod by its run-id annotation, not its hash label (ANR-1160) ([#135](https://github.com/vymalo/another-agentic-platform/issues/135)) ([d483ab6](https://github.com/vymalo/another-agentic-platform/commit/d483ab6718b400020c8162059ef9c668a549c155))
* **docs-check:** a tilde fence is code, and a URL is not a path:line (ANR-1362) ([#194](https://github.com/vymalo/another-agentic-platform/issues/194)) ([b6b177d](https://github.com/vymalo/another-agentic-platform/commit/b6b177df04a9a11e16e778d05a50e5c9ac65ae8c))
* **docs:** config.ts reads DEFAULT_BASE instead of writing the base twice (ANR-1389) ([#209](https://github.com/vymalo/another-agentic-platform/issues/209)) ([882c14f](https://github.com/vymalo/another-agentic-platform/commit/882c14f77292b24f29ecfa4b5197d7e55acbf1e9))
* **docs:** escape a CRD table cell once and walk a map beside fixed fields (ANR-1391) ([#210](https://github.com/vymalo/another-agentic-platform/issues/210)) ([5ff85fc](https://github.com/vymalo/another-agentic-platform/commit/5ff85fcd75944a9668d9cee6170dc46005b44a7e))
* **docs:** indented code is code to the checker, the renderer retries, and STRUCTURE.md says what this site has (ANR-1085 review) ([#197](https://github.com/vymalo/another-agentic-platform/issues/197)) ([fa00367](https://github.com/vymalo/another-agentic-platform/commit/fa00367cc509da15e37bb45794b3b4e2d8aaa882))
* **docs:** the checker's tab stop is CommonMark's, so its column matches the renderer's (ANR-1085 review) ([#201](https://github.com/vymalo/another-agentic-platform/issues/201)) ([f86b61d](https://github.com/vymalo/another-agentic-platform/commit/f86b61dada88e61fe6f73ebb05bba2ee7dc238cc))
* **domain:** a key a toleration or a nodeAffinity does not have is refused (ANR-1357) ([#191](https://github.com/vymalo/another-agentic-platform/issues/191)) ([782c889](https://github.com/vymalo/another-agentic-platform/commit/782c88977a0cb9993d8af4a5b582976b4fed42d6))
* **domain:** a key an egress rule does not have is refused, not dropped (ANR-1356) ([#187](https://github.com/vymalo/another-agentic-platform/issues/187)) ([bbe69e8](https://github.com/vymalo/another-agentic-platform/commit/bbe69e844e6bf85b5be04ac3e102a6f558340354))
* **domain:** a run pod's egress refuses a selector that selects everything, and a port entry with no port (ANR-1355) ([#202](https://github.com/vymalo/another-agentic-platform/issues/202)) ([ecc2cc2](https://github.com/vymalo/another-agentic-platform/commit/ecc2cc2dd0ca7b71babfcac34506e48e581567a5))
* **domain:** an egress rule needs a to, and no range may be a /0 (ANR-1353) ([#182](https://github.com/vymalo/another-agentic-platform/issues/182)) ([f04f04c](https://github.com/vymalo/another-agentic-platform/commit/f04f04c93c11a15508a69143ca1bbc0bdb6e3cea))
* **e2e:** assert the card's own name, not the old pin's Coder ([ddd360c](https://github.com/vymalo/another-agentic-platform/commit/ddd360c8f660ed8df4b7d4fbb2d581c772fae3e9))
* **forms:** the export suite passes and can run on its own (ANR-1352 review) ([#206](https://github.com/vymalo/another-agentic-platform/issues/206)) ([47798b9](https://github.com/vymalo/another-agentic-platform/commit/47798b9568565cd8574637dbb4a7dad0b9f6e72f))
* **meter:** read the AgentService's uid before billing it ([79e7ff0](https://github.com/vymalo/another-agentic-platform/commit/79e7ff0c7cb04445e215855d60337a566c0f8d76))
* **objects:** a wake-at written during a save is not a change (ANR-1183) ([#176](https://github.com/vymalo/another-agentic-platform/issues/176)) ([e2ed4c7](https://github.com/vymalo/another-agentic-platform/commit/e2ed4c7062294346bda7ba94aede1056eb494ece))
* **operator, trace:** clear the warnings that make clippy -D warnings red (ANR-1192 follow-up) ([#228](https://github.com/vymalo/another-agentic-platform/issues/228)) ([32d6fe6](https://github.com/vymalo/another-agentic-platform/commit/32d6fe6003678b858469c37951f4356c2ba33179))
* **tokens:** stamp the inode, and the last read-once sentence (ANR-1192 review follow-up) ([#227](https://github.com/vymalo/another-agentic-platform/issues/227)) ([d3a7bdf](https://github.com/vymalo/another-agentic-platform/commit/d3a7bdff191d6e95edaa6b8499108f6273104fc3))
* **usage:** drop the duplicate import block that broke cargo test on main (ANR-1190 follow-up) ([#221](https://github.com/vymalo/another-agentic-platform/issues/221)) ([b2fa501](https://github.com/vymalo/another-agentic-platform/commit/b2fa5019278423a2c712e68f48e8ad13eb20ec18))


### Documentation

* a Concepts page first under Architecture, and an honest README status (ANR-934) ([#168](https://github.com/vymalo/another-agentic-platform/issues/168)) ([3a2c7e0](https://github.com/vymalo/another-agentic-platform/commit/3a2c7e09a8ee7277cb96d7f2a2f615d41d97ca36))
* a Writing docs page, and Contributing links the roadmap and the docs rules (ANR-936) ([#170](https://github.com/vymalo/another-agentic-platform/issues/170)) ([59b7f7c](https://github.com/vymalo/another-agentic-platform/commit/59b7f7ce4160db21c8e0197be201293fe02d9cd8))
* AD-059 accepted, with §10a's four questions answered (ANR-960) ([#156](https://github.com/vymalo/another-agentic-platform/issues/156)) ([628c5a7](https://github.com/vymalo/another-agentic-platform/commit/628c5a7ff781b0f48e675501bacd5cccab656dd7))
* **adr:** AD-063 gains the run-certificate names, contract v1 and the issuer choice (ANR-1307) ([#159](https://github.com/vymalo/another-agentic-platform/issues/159)) ([c22cd06](https://github.com/vymalo/another-agentic-platform/commit/c22cd06e57207c3d70081744d690257a61973d01))
* **api:** a null for an optional field is pruned, and must not be guarded ([4150b93](https://github.com/vymalo/another-agentic-platform/commit/4150b93a8b128de2cbe9f83d399ab039ac027da7))
* **architecture:** §59b's cluster facts move to the agent page (ANR-1380) ([#208](https://github.com/vymalo/another-agentic-platform/issues/208)) ([d3b2cc7](https://github.com/vymalo/another-agentic-platform/commit/d3b2cc74f3ca3f891fd6370773d6e94182cd6a39))
* **architecture:** §59c's increment records move to the agent page (ANR-1379) ([#205](https://github.com/vymalo/another-agentic-platform/issues/205)) ([d48bc00](https://github.com/vymalo/another-agentic-platform/commit/d48bc00ac9bef57a985a141b0438375b7f9ee672))
* **architecture:** §60a's K8 note says the Events step has since passed (ANR-1393) ([#211](https://github.com/vymalo/another-agentic-platform/issues/211)) ([e41d6d5](https://github.com/vymalo/another-agentic-platform/commit/e41d6d5528dae35babacee201b633be35ad46d28))
* **architecture:** §60a's slice records move to the agent page (ANR-1378) ([#204](https://github.com/vymalo/another-agentic-platform/issues/204)) ([cbcfc6e](https://github.com/vymalo/another-agentic-platform/commit/cbcfc6e2a009ef81788f15ce51615e44848ba1d6))
* **architecture:** §60b carries the three RFC verifications the split left unmarked (ANR-1381 review) ([#215](https://github.com/vymalo/another-agentic-platform/issues/215)) ([af8f057](https://github.com/vymalo/another-agentic-platform/commit/af8f057754175fe9221bf6019d560b62d90bfb4c))
* **architecture:** §60b's proofs move to the agent page (ANR-1381) ([#207](https://github.com/vymalo/another-agentic-platform/issues/207)) ([d62caca](https://github.com/vymalo/another-agentic-platform/commit/d62caca4560195cb6ea6bd11567b5e65e02b1ab8))
* **architecture:** the build records and inline proofs of §59a move to its agent page (ANR-1343) ([#193](https://github.com/vymalo/another-agentic-platform/issues/193)) ([14e8390](https://github.com/vymalo/another-agentic-platform/commit/14e8390bb58dc01027d74291d49ea6f32834221c))
* **architecture:** the control-plane page leaves stillMixed (ANR-1367) ([#212](https://github.com/vymalo/another-agentic-platform/issues/212)) ([c034cc0](https://github.com/vymalo/another-agentic-platform/commit/c034cc0bed0b44ae8e0682bb2e41009a068ade9e))
* **architecture:** the facts §60a stands on move to its agent page (ANR-1366) ([#195](https://github.com/vymalo/another-agentic-platform/issues/195)) ([388397c](https://github.com/vymalo/another-agentic-platform/commit/388397c475e1cdf88b068737d8ae309f506db0f0))
* **chart:** the workload permissions comment names both grantable permissions (ANR-1118 review) ([#177](https://github.com/vymalo/another-agentic-platform/issues/177)) ([389bd64](https://github.com/vymalo/another-agentic-platform/commit/389bd641c6026292fff3794b537d1190d98c90dc))
* **crds:** regenerate the AgentService reference for spec.release and status.revisions (ANR-965 follow-up) ([#222](https://github.com/vymalo/another-agentic-platform/issues/222)) ([6cf8958](https://github.com/vymalo/another-agentic-platform/commit/6cf895862214e7c4049afd0cbbea5ea6a56bce22))
* **deploy:** generate the activity bearer in the cluster and push it (ANR-1318) ([#166](https://github.com/vymalo/another-agentic-platform/issues/166)) ([965d7b4](https://github.com/vymalo/another-agentic-platform/commit/965d7b4d8c86f9e3877131e37aa950db28caf10e))
* **deploy:** the install section follows the chart, and the run-pod facts match the code (ANR-1073) ([#163](https://github.com/vymalo/another-agentic-platform/issues/163)) ([2284765](https://github.com/vymalo/another-agentic-platform/commit/2284765f2b4b31fa8e36edda8c51892d78d772d2))
* **deploy:** the probe settings are keys of the operator's file, not variables (ANR-1325) ([#164](https://github.com/vymalo/another-agentic-platform/issues/164)) ([baadfe2](https://github.com/vymalo/another-agentic-platform/commit/baadfe21abae3c1d9561a1c11325f3ef92b1761a))
* **extensions:** the activity/v1 contract and AD-075 (ANR-1417) ([#216](https://github.com/vymalo/another-agentic-platform/issues/216)) ([46ade54](https://github.com/vymalo/another-agentic-platform/commit/46ade549b6405857020e6abdb3b2ef2f010274c3))
* **get-started:** four more walk-throughs, from a folder agent to a tool server (ANR-1202) ([#142](https://github.com/vymalo/another-agentic-platform/issues/142)) ([1ecba53](https://github.com/vymalo/another-agentic-platform/commit/1ecba5304db699ed8533eca7234ea11c8244c24a))
* **guides:** the run records leave the pages for people, and the checker refuses a dated mark (ANR-1361) ([#199](https://github.com/vymalo/another-agentic-platform/issues/199)) ([5800766](https://github.com/vymalo/another-agentic-platform/commit/58007664b91efdfbb79be8afd92fb8cc8e883408))
* **meter:** a chain that cannot be read is held, not counted ([18d4a8f](https://github.com/vymalo/another-agentic-platform/commit/18d4a8f10f21c66864fb4500b90c0bab8336d909))
* **observability:** the metrics transport is a collector scrape (ANR-1479) ([#232](https://github.com/vymalo/another-agentic-platform/issues/232)) ([21a1114](https://github.com/vymalo/another-agentic-platform/commit/21a1114acb712ad1240c07b16a5aa4e58a55e317))
* one revision-history table again, in date order (ANR-1354) ([#181](https://github.com/vymalo/another-agentic-platform/issues/181)) ([1fe5485](https://github.com/vymalo/another-agentic-platform/commit/1fe5485f49a5a55405fa7b9e28a203e0dae13464))
* **security:** workload identity in three modes, with OpenBao as the issuer (ANR-1082, AD-077) ([#231](https://github.com/vymalo/another-agentic-platform/issues/231)) ([8124bb1](https://github.com/vymalo/another-agentic-platform/commit/8124bb1c2dbd527494ede9e821a5c70a2d2727d2))
* separate the docs for people from the docs for agents (ANR-1002, AD-074) ([#190](https://github.com/vymalo/another-agentic-platform/issues/190)) ([c54ff9d](https://github.com/vymalo/another-agentic-platform/commit/c54ff9d212c5a10cb6a6cea9fe8118d9c6569943))
* **site:** one page per CRD kind and the aapctl reference (ANR-935) ([#171](https://github.com/vymalo/another-agentic-platform/issues/171)) ([3cc2543](https://github.com/vymalo/another-agentic-platform/commit/3cc2543af02d17b454c8db7af60f7b1bb77108ba))
* **site:** render the Platform API with vitepress-openapi (ANR-930) ([#167](https://github.com/vymalo/another-agentic-platform/issues/167)) ([0460eb1](https://github.com/vymalo/another-agentic-platform/commit/0460eb1dbece91bc2fcf56e67207aba06ad77861))
* **site:** STRUCTURE.md v3, the wider sidebar, the in-depth block and the status icons (ANR-1085) ([#160](https://github.com/vymalo/another-agentic-platform/issues/160)) ([7f05efe](https://github.com/vymalo/another-agentic-platform/commit/7f05efef1fda40c2874bfa0f2f40c5c62ff1a424))
* the §60a links name AD-073's anchor, not AD-072's (ANR-1341) ([#165](https://github.com/vymalo/another-agentic-platform/issues/165)) ([3ef9637](https://github.com/vymalo/another-agentic-platform/commit/3ef96377bb36929599e9a0efdf96a1ada11b4236))
* the operator's telemetry is its file, and the controller lists all fifteen series (ANR-1077) ([#157](https://github.com/vymalo/another-agentic-platform/issues/157)) ([4ee3527](https://github.com/vymalo/another-agentic-platform/commit/4ee3527653241bcfa1c0ca419a56b6c67235417c))


### Tests

* **api:** reserve three distinct ports, and say why a start failed (ANR-1065) ([#189](https://github.com/vymalo/another-agentic-platform/issues/189)) ([2f8159c](https://github.com/vymalo/another-agentic-platform/commit/2f8159cac5160467f9fd5c3ca2776f9587b44e6b))
* **cli:** the two-command refresh test starts the second after the first wrote (ANR-1064) ([#188](https://github.com/vymalo/another-agentic-platform/issues/188)) ([7bf5279](https://github.com/vymalo/another-agentic-platform/commit/7bf52795783a3e7af001db8bde36ee6c204a8fe5))
* **operator:** compose the meter and flush its sink on the way out (ANR-1193) ([#183](https://github.com/vymalo/another-agentic-platform/issues/183)) ([a6c248e](https://github.com/vymalo/another-agentic-platform/commit/a6c248e432b6b7228e15085762691d3a97b00e0b))
* **operator:** serialise the cluster suite's start and retry only a taken port (ANR-1363) ([#200](https://github.com/vymalo/another-agentic-platform/issues/200)) ([d0eb095](https://github.com/vymalo/another-agentic-platform/commit/d0eb095c5b550fa3f1ee30d9c65b059c453d5ade))
* **operator:** three distinct ports for the operator child (ANR-1358) ([#192](https://github.com/vymalo/another-agentic-platform/issues/192)) ([45be7b2](https://github.com/vymalo/another-agentic-platform/commit/45be7b28bc84ce7f3cc15b38baa4e3d46bc7e3e6))
* **platform-api:** the 403 matrix compares permissions whole, not as prefixes (ANR-1118) ([#184](https://github.com/vymalo/another-agentic-platform/issues/184)) ([f02f36f](https://github.com/vymalo/another-agentic-platform/commit/f02f36fe9d15efadfb8b7d6f515d10c6ca75dab1))
* **usage:** the AccountSource conformance suite, and the broker and the fake pass it (ANR-1190) ([#214](https://github.com/vymalo/another-agentic-platform/issues/214)) ([d432aab](https://github.com/vymalo/another-agentic-platform/commit/d432aab6c5b0da89704f65357f3b889c70a0209f))


### Build System

* **docker:** static musl on distroless/static for the operator and the API (ANR-1174) ([#225](https://github.com/vymalo/another-agentic-platform/issues/225)) ([ffd14a7](https://github.com/vymalo/another-agentic-platform/commit/ffd14a75140f43b4b3e65b339da7a0b57a3d9d3b))


### Continuous Integration

* cancel stale runs on a newer push, a merge or a deleted branch ([#155](https://github.com/vymalo/another-agentic-platform/issues/155)) ([7117d46](https://github.com/vymalo/another-agentic-platform/commit/7117d4690ee87f36db07f1b9ef14814f43435002))
* fail when a release-PR skip guard drifts from the other 25 (ANR-1323) ([#151](https://github.com/vymalo/another-agentic-platform/issues/151)) ([37dc2e7](https://github.com/vymalo/another-agentic-platform/commit/37dc2e7cfb227b198b1d6d12dcea426862f8a3bd))
* **release:** give the release commit's operator-image run a group of its own (ANR-1196, AD-070) ([#144](https://github.com/vymalo/another-agentic-platform/issues/144)) ([db85dcd](https://github.com/vymalo/another-agentic-platform/commit/db85dcd741bdcb2701086aeec0997c6cc025b4b1))
* **release:** refuse a release branch whose top commit is not release-please's (ANR-1326) ([#148](https://github.com/vymalo/another-agentic-platform/issues/148)) ([5441bb6](https://github.com/vymalo/another-agentic-platform/commit/5441bb69ffb8ffa90ea6e23215f9350e0ddbc4a8))
* skip the release PR's jobs only for the release App's bot (ANR-1302) ([#147](https://github.com/vymalo/another-agentic-platform/issues/147)) ([249c85b](https://github.com/vymalo/another-agentic-platform/commit/249c85b3ff2a35ac4132f07584c857cc17cef801))
* the release-please PR skips the heavy jobs, and governance cancels its older run (ANR-1206) ([#143](https://github.com/vymalo/another-agentic-platform/issues/143)) ([2e367cf](https://github.com/vymalo/another-agentic-platform/commit/2e367cf247c2afdfebcb8d3429be3b79f283b9fc))


### Chores

* **deploy:** bump operator and api to sha-d264559 ([05cf2eb](https://github.com/vymalo/another-agentic-platform/commit/05cf2eb0701af6bf399c770aca3d1a822a9d4ad0))

## [0.2.0](https://github.com/vymalo/another-agentic-platform/compare/v0.1.0...v0.2.0) (2026-11-27)


### Features

* **usage:** aap-usage-otlp, the sink that sends pod-minutes to the collector as OTLP logs ([#127](https://github.com/vymalo/another-agentic-platform/issues/127)) ([fa4815e](https://github.com/vymalo/another-agentic-platform/commit/fa4815e4e048a8cbf48aa2b047ae0bdfbdc3f227))


### Tests

* **e2e:** an empty code from a failed exec is no answer either, so the registry wait keeps waiting ([#133](https://github.com/vymalo/another-agentic-platform/issues/133)) ([34d5eef](https://github.com/vymalo/another-agentic-platform/commit/34d5eef3fab8d8826f194ee8a5f3ef8bc44acd67))
* **e2e:** wait for the registry to answer before the first probe, and print the code a check got ([#130](https://github.com/vymalo/another-agentic-platform/issues/130)) ([016598d](https://github.com/vymalo/another-agentic-platform/commit/016598d6f24650bca3e01b771693b9cafaaaac5d))
* **usage-otlp:** wait for the sink's counters; docs: CLAUDE.md names release-pin.sh ([#132](https://github.com/vymalo/another-agentic-platform/issues/132)) ([d3cca03](https://github.com/vymalo/another-agentic-platform/commit/d3cca033962eaed2f7dc6d201524fa5b76cbad9b))


### Continuous Integration

* **release:** release PRs carry the governance template's sections (AD-070) ([#134](https://github.com/vymalo/another-agentic-platform/issues/134)) ([15a63e3](https://github.com/vymalo/another-agentic-platform/commit/15a63e3dedbab878005d2a195bce898ca5bc53a9))
* **release:** release-please, and signed versioned operator and API images and OCI charts on a v* tag (AD-070) ([#129](https://github.com/vymalo/another-agentic-platform/issues/129)) ([798ea14](https://github.com/vymalo/another-agentic-platform/commit/798ea149a4b433a262ee14d68b6d79c882ff1b3f))


### Chores

* **deploy:** bump operator and api to sha-016598d ([386c962](https://github.com/vymalo/another-agentic-platform/commit/386c962670f46338819535bdc5c6ea36fa6c1fca))
* **deploy:** bump operator and api to sha-798ea14 ([ea2ea69](https://github.com/vymalo/another-agentic-platform/commit/ea2ea69fcb56500e6fe0d7feea18da1e2f3f974e))
* **deploy:** bump operator and api to sha-fa4815e ([a2dd4f8](https://github.com/vymalo/another-agentic-platform/commit/a2dd4f80dafd376cc217a8818efcd45b71719f3b))
