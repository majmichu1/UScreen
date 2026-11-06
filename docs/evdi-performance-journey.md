---
priority: niedrig
---

# EVDI performance development journey, 5–6 November 2026

This report records how the EVDI helper changes were developed for the merged PR #49 and its subsequent multi-tablet follow-up, including the intermediate regressions and the approaches that were rejected. Exact content comparison, shared conversion, a faster RGB row comparison, writer correctness and wakeup changes, FIFO allocation fallback, and native reference/conversion fusion were retained. The performance evidence supports specific helper CPU reductions under the synthetic workloads below. It does not establish a whole-system CPU, GPU, power, or end-to-end latency reduction.

The [CSV matrix](evdi-performance-matrix.csv) preserves **163 historical candidate comparisons across 151 distinct case groups**, derived from **1,382 recorded raw runs in 16 benchmark files**. It additionally includes **40 thread-sweep configuration summaries from 200 timed runs and 40 byte-verification records**, plus **six live phase comparisons** and **six separate final-runtime observations**: 215 matrix rows in total. `record_type` distinguishes these scopes. The extra comparisons come from the early multi-variant fusion and vectorization experiments. The [sanitized numerical evidence](evdi-performance-evidence.json) contains every recorded benchmark row, case-group mapping, original summary, source SHA-256 identity, and retained/rejected verdict used here. No recorded candidate or case group is omitted because it was slower.

The first historical series ends at native fused source `40cd003e`. The later sections add a finite thread-count sweep and a source-bound three-tablet A/B observation with correct own-window input and video markers. A later source-bound run observed the selected runtime hint and device-side ADB reconnect/application recovery. Physical cable reconnect, reboot and pen-only interaction were not tested; the limits of these bounded observations remain explicit below.

## Scope, source identity, and reading the measurements

The production code discussed here is [host/evdi/evdi_helper.c](../host/evdi/evdi_helper.c), with native regression coverage in [host/evdi/evdi_idle_test.c](../host/evdi/evdi_idle_test.c). Each measured source is identified independently below because many experimental variants were preserved snapshots rather than commits. Their hashes identify the source bytes; they are not a claim that the current checkout, an installed executable, and every historical benchmark binary are identical.

There are three distinct source comparisons that must remain separate:

1. The early release denominator is commit `edb5cc5f3a7efb066e7e3967a6306a647e0d5c7b`. Its helper source is also present at `39ca854b41915ea936d432f3854aa0b3db22ec7e` and hashes to `4d12a525…`.
2. The publication comparison uses later upstream commit `8fcfe9e83e98b33e61c7d40275a7b1686e3272da`, helper SHA-256 `80847466…`, against integrated commit `6c09add6683e063259a471e5395675ee0ba45665`, helper SHA-256 `35486c2c…`. This upstream event-handling base differs from the early release base.
3. The final native fusion experiments use the writer/FIFO snapshot `6431462a…` as their denominator and fused snapshot `40cd003e…` as their candidate. There is no isolated CPU benchmark between the publication snapshot and the writer/FIFO snapshot.

Reductions from these different denominators must not be added, chained into a claimed total, or substituted for a measured final-versus-upstream comparison. The row-specific denominator in the matrix is authoritative.

The synthetic harness simulates full-damage grabs and includes its input mutations and the process CPU used by conversion workers. It excludes real EVDI/kernel framebuffer copies, the capture FIFO, FFmpeg, GPU work, display latency, and whole-system energy use. `CPU ms/grab` means median process CPU seconds per run × 1000 ÷ simulated grabs. It is not wall-clock frame latency. A multi-threaded process can accumulate more CPU time than elapsed time.

`single` and `pool` are harness modes; the actual thread counts are preserved in every matrix row. `scale` is the stream downscaling factor, and `frames` is the number of simulated grabs per variant per repeat, including grabs that publish no frame. Case names are normalized as follows: `static` repeats identical source RGB; `unused` changes only an unused channel; `moving` changes the last pixel; `full` changes the full frame; `noise` updates a random texture; and `bar` moves a narrow edge bar. Original case labels and omitted mode/scale fields remain explicit in the CSV and JSON. Early mode/scale values were implicit in the original rows, not newly measured values.

For each repeat, the paired ratio is candidate CPU ÷ denominator CPU. Ratios below 1 mean less CPU time. The first four benchmark files originally reported the **ratio of variant medians**; subsequent files reported the **median of paired ratios**. These estimators differ and both are preserved in the CSV. The full tables below use the estimator originally reported by that file. The pair range is the observed minimum–maximum, not a confidence interval. Where an original 95% bootstrap interval exists, it is copied and labeled as historical; it was not re-bootstrapped for this report. Three, five, or seven pairs provide limited uncertainty information and no broad hardware generalization.

`history/...` values are portable logical evidence identifiers. They do not imply that the original files are hosted at those paths. Original-byte hashes bind the private preserved originals, while the public JSON is a sanitized numerical projection. Repository links resolve to actual project files. Personal filesystem paths, USB serials, private network addresses, screenshots, and raw configuration/process logs are excluded.

## Why work was needed

The original investigation observed a 1536 × 960, 60 fps H.264 VAAPI stream with NV12 input, constant-QP mode (`qp=16`) and a 44,000 kbit/s configured maximum. Hardware encoding was active. A static encoder image could still be submitted at approximately 60 fps. A configured bitrate cap did not describe the measured bitrate or explain the helper's CPU usage.

The trace followed the work from Cinnamon's presentation of the shared desktop through Xorg's full-region damage reporting to EVDI and the helper. Two separate ten-second tracing windows each contained 590 full-frame damage notifications, approximately 58.92 and 59.02 per second. Xorg already supplied a full-size rectangle, so attributing every large rectangle to an EVDI expansion would have been incorrect. A damage notification described invalidation, not proof that every source RGB pixel had changed.

The following are **source-reported historical diagnostic intervals**, not matched optimized-versus-original measurements. The source identity is `history/analyses/UScreen_CPU_GPU_Last_2026-11-05.md`; its SHA-256 is preserved in the numerical evidence. CPU percentages use one logical core as 100%. GPU counters describe the specified client's engine utilization; they are not percentages of total system power.

| Process | Initial CPU interval, % of one core | Static-window CPU interval, % of one core |
| --- | ---: | ---: |
| EVDI helper | 48.04 | 38.14 |
| FFmpeg | 17.34 | 15.26 |
| Cinnamon | 27.07 | 32.34 |
| Xorg | 5.99 | 5.61 |
| htop | 4.37 | 4.33 |
| ADB | 2.95 | 2.36 |

| GPU client | Initial render-engine % | Static render-engine % | Static video-engine %, relative to one engine |
| --- | ---: | ---: | ---: |
| FFmpeg | 6.59 | 6.35 | 5.41 |
| Cinnamon | 11.46 | 12.76 | 0 |
| Xorg | 5.95 | 6.44 | 0 |

Profiling attributed 51.43% of sampled helper CPU to `conv_worker`, 6.45% to `publish_frame`, 17.05% to `_copy_to_user`, 7.31% to `drm_clflush_virt_range`, 5.01% to `_copy_from_iter`, 3.01% to `clear_highpages_kasan_tagged`, and 2.72% to `find_vmap_area`. These are sampling locations, not exclusive independently timed pipeline stages. They motivated reducing conversion and memory movement; they cannot be added to benchmark gains.

A permitted stream-off/restart comparison recorded the following changing workload. A later reconstruction of the encoder timeline showed that the expensive regime could subside before restart and recur afterwards. Restart therefore was not established as the causal repair.

| Process or GPU counter | Before, static window | Stream off | Restart, Firefox | Restart, htop |
| --- | ---: | ---: | ---: | ---: |
| Helper CPU, % of one core | 38.14 | 0 | 4.41 | 4.88 |
| FFmpeg CPU, % of one core | 15.26 | 0 | 3.09 | 3.16 |
| Cinnamon CPU, % of one core | 32.34 | 3.07 | 10.54 | 3.35 |
| Xorg CPU, % of one core | 5.61 | 0.49 | 1.23 | 0.74 |
| FFmpeg render-engine % | 6.35 | No client | 0.98 | 0.82 |
| FFmpeg video-engine % | 5.41 | No client | 1.04 | 0.67 |
| Cinnamon render-engine % | 12.76 | 0.92 | 0.92 | 1.87 |
| Xorg render-engine % | 6.44 | 0.15 | 0.71 | 0.65 |

In a later controlled trigger test, a small visible surface was redrawn on the physical monitor, outside the tablet's desktop rectangle. The prepared optimized helper was not running in this experiment. The historical source is `history/evdi-idle-optimization-20261105/trigger-live.json`; the narrative is `history/analyses/UScreen_EVDI_Optimierung_und_Ausloeser_2026-11-05.md`.

| Trigger phase | Actual encoder frames/s |
| --- | ---: |
| Quiet baseline, no visible test window | 11.6 |
| Hidden window, 60 draw updates/s | 8.4 |
| Visible unchanged window | 8.4 |
| Visible redraw, first attempt | 52.3 |
| Window hidden afterwards | 8.7 |
| Visible redraw, second attempt | 49.5 |
| Window hidden again | 10.0 |

The helper used approximately 49–51% of one core during visible redraw and 9–11% in quiet controls; FFmpeg used approximately 17% versus 4–5%. These phases demonstrate that visible redraw outside the tablet can drive this expensive path on the observed desktop. They do not measure the optimized helper or identify the cause of every historical busy interval.

A separate natural busy observation recorded 1,374 AyuGram presentations, 492 Cinnamon presentations and 492 shared-pixmap transfers in approximately ten seconds. Every transfer reported the full 6529 × 2040 shared source region to the 1536 × 960 tablet target. A later quiet observation recorded no AyuGram presentations and 66 Cinnamon presentations/transfers. This temporal association is not an isolated AyuGram intervention. There was no established visible animation, and the specific internal repaint cause was not determined. Of 44 accepted tablet-buffer samples, seven distinct NV12 hashes occurred; six concurrent-change samples were rejected. That series does not establish an unchanged tablet throughout or a percentage of redundant frames. The attempted isolated minimization comparison failed serialization; a corrected attempt started quiet and therefore performed no further minimization test.

## Development decisions and rejected approaches

1. **Exact comparison first exposed an expensive design.** `content-v1` compared actual RGB content but copied the full reference and retained damage-based full conversion on a change. It reduced static work but made the last-pixel case about 1.401× the release CPU cost under the original estimator. It was rejected. `content-dirty-rows` then limited conversion to rows whose used RGB channels actually changed, while preserving writer/keepalive behavior. Its full-frame-change result was still about 1.207× the release cost in the early three-repeat file, so the implementation needed further conversion work.
2. **The first fusion was insufficient.** `reference-band-fusion` moved reference copying near conversion and grouped rows by chroma bands. In the fusion experiment its full-change result was about 1.188× the original release cost. This was rejected as a final design. It is a different experiment from the later native load-sharing fusion.
3. **Compiler vectorization helped native conversion.** `native-compiler-vectorized` improved native cases, but the scaled path remained a reason to continue. `shared-converter` reused the byte conversion kernel for native and scaled input, with packed B,G,R,0 scratch and the same integer averaging. The pinned repeat series retained it. All eleven groups and their unequal pair/frame counts remain below.
4. **The comparison loop needed the right fast path.** Fixed channel specialization (`specialized-channels`) did not give a reliable extra benefit. Grouped OR comparison without immediately testing the first pixel (`blocked-no-first-pixel`) regressed full-frame change in the short single-thread test. Comparing whole source-row bands (`banded-compare`) added complexity without a stable unchanged-content benefit. All three were rejected. `blockedfast-compare` retained the immediate first-RGB-pixel test, a sixteen-pixel OR reduction, and a scalar tail. It was retained after seven-pair target measurements, while its near-neutral and slower groups remain visible.
5. **Publication rebased the comparison.** `publication-integrated` combined the retained changes on the later upstream event-handling base. Its separate five-pair publication series is the valid source for claims against `8fcfe9e`, rather than the earlier comparisons against `edb5cc5`.
6. **More instruction-set machinery was not an automatic win.** AVX2 `target_clones` regressed native full-frame, texture and bar cases. SSE4.1 clones had mixed results and no reliable overall benefit. Both were rejected. Later handwritten SSE2 produced the recorded byte-identical conversion outputs but no useful overall CPU win and was also rejected. Their complete slower and faster groups are retained.
7. **Writer and FIFO changes addressed behavior and resources.** `writer-fifo` releases an obsolete writer-buffer claim before an idle skip, waits until the existing keepalive deadline, and wakes immediately on publication. FIFO allocation attempts smaller capacities if a larger growth fails. Historical regression, wakeup, mutation and pipe-capacity evidence supports these changes. The SIMD files use the pre-writer publication source as control and therefore do not quantify writer/FIFO CPU improvement. No isolated percentage gain is claimed.
8. **The later native fusion removed duplicate source loading.** `native-reference-conversion-fused` captures reference bytes in the same native load path used to convert to NV12. The scaled path keeps separate reference snapshots. The short three-pair screening run and longer seven-pair run are both retained. In the longer run the native full/bar/noise groups have ratios around 0.782–0.859, with smaller, neutral, and slightly worse results in other groups. These are comparisons against `writer-fifo`, not against upstream. The longer static case is 0.940 rather than the short run's 1.096, which illustrates why the screening result alone was insufficient.

The following table binds every path, including reference and superseded paths, to its exact helper source. `retained_then_superseded` and `superseded_partial_candidate` are historical decisions, not claims that all of their implementations remain separately enabled.

| Stage | Decision | Helper source SHA-256 |
| --- | --- | --- |
| `release-original` | reference | `4d12a525e6bddd5ca2518433a529c19ede80bf465051b0ccd656d5ece5dd28c0` |
| `content-v1` | rejected | `f152ea7220fd434088cf41b6f7e918ad9b2da858a46057b497849d0d56570125` |
| `content-dirty-rows` | retained_then_superseded | `b7ab6894eb364ef7f0ce170830baab65c3ff260b2b09f823f66b8bd92489c6a4` |
| `reference-band-fusion` | rejected_as_final | `b6cc1d3209c86aa52bf87752547eba3dea17026e9bd3110b575822811adbfc4c` |
| `native-compiler-vectorized` | superseded_partial_candidate | `8d35d6d1bb7ef5714df6f26b4d9a079bb5bea569aa7445260a7bf67836a9131c` |
| `shared-converter` | retained | `e73f7746efaf980ad75a364dd004c2a243e17407c173b8213fcc27c7dd206b6e` |
| `specialized-channels` | rejected | `40b445c36272f9079d700de5908b47e6523c064687e861314c66532115182dc9` |
| `blocked-no-first-pixel` | rejected | `f4b631d4be92383846a45d885f40ba995ef1a48ec20e34b1f9a0bbaecd6d7d2a` |
| `banded-compare` | rejected | `fa8fd05bf3cfaf1f5338a35bba844aac5764b12fd2d43d5ccb135ae964ac2108` |
| `blockedfast-compare` | retained | `964201174da2c4278ba9e0042f8fde77871af49dfa68e7446e17f7e54fca2008` |
| `publication-integrated` | retained | `35486c2c52bbe44ba683ff1b2421af4485f9a572f0d87c62053eaedb0ab43a6e` |
| `upstream-publication-base` | reference | `8084746605615e34138e8eed2dd267d3ccf21f8acf9a4aca418c84e7d980d2da` |
| `avx2-clones` | rejected | `0a25ef7f6424ffdfe62003414afe4e4137a257147d66e4601aef0018c1f12689` |
| `sse41-clones` | rejected | `9c231f55c9cb25f4f3e1d11fae793030d121970d58d74fabd588b4c627473691` |
| `writer-fifo` | retained | `6431462a2ba78a63897ac2460cb474b44530e21f6e6e4c091aa2618a3b7f6a1c` |
| `handwritten-sse2` | rejected | `954ba24d58c067a6d9def626fbba284db5ae6390906acaa59f984617501b9b5e` |
| `native-reference-conversion-fused` | retained | `40cd003e232cc96ba4070ae33c2aa96f61da550244524790fc8ec66974f4f60e` |

The first rejected source was reconstructed from its preserved patch over the original git blob and checked against its recorded SHA-256. Other uncommitted stages are bound to preserved source snapshots. Source hashes and original benchmark hashes were rechecked during this export. This establishes the identity of the preserved evidence; it is not a new build-to-binary attestation.

## Complete benchmark results

Each section names its logical evidence ID and original-byte SHA-256. Rows use the original estimator described above. `B` is the denominator CPU cost and `C` is the named candidate CPU cost, both in process CPU ms per simulated grab. The two thread counts are candidate/denominator. An interval in the last column is a copied historical 95% bootstrap interval. A dash means no such interval was recorded. The full-precision ratios, both estimators, publication counts, raw indices and original JSON pointers are in the CSV/JSON.

### evdi-content-idle-20261105/checks-and-benchmark-v1

Evidence ID: `history/evdi-content-idle-20261105/checks-and-benchmark-v1.json`. Original SHA-256: `e30b54e37a0bd50fdbe54f6da84cf3ee76980378fd84b5d0c75ce6100145e90b`. Denominator: `release-original`. 18 raw runs, 3 case groups.

Scope: 1536x960, 240 simulated full-damage grabs; one real conversion thread; no kernel copy/FFmpeg/GPU. Estimator: ratio of each variant median CPU time to denominator median CPU time.

Method: alternating before/after; 3 repeats per case.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `content-v1` | moving | single / 1 | 240 / 3 | 1/1 | 2.491061 | 3.489152 | 1.400669 | 1.400669–1.508886 | — |
| `content-v1` | static | single / 1 | 240 / 3 | 1/1 | 2.469574 | 0.417423 | 0.169026 | 0.165281–0.202307 | — |
| `content-v1` | unused | single / 1 | 240 / 3 | 1/1 | 2.862717 | 1.284193 | 0.448593 | 0.447896–0.458553 | — |

### evdi-content-idle-20261105/checks-and-benchmark

Evidence ID: `history/evdi-content-idle-20261105/checks-and-benchmark.json`. Original SHA-256: `a08273d82bab0d627ac155469a4c9d1b561fe5896adcd9f162057f69efe405b4`. Denominator: `release-original`. 24 raw runs, 4 case groups.

Scope: 1536x960, 240 simulated full-damage grabs; one real conversion thread; no kernel copy/FFmpeg/GPU. Estimator: ratio of each variant median CPU time to denominator median CPU time.

Method: alternating before/after; 3 repeats per case.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `content-dirty-rows` | full | single / 1 | 240 / 3 | 1/1 | 2.722579 | 3.285000 | 1.206576 | 1.185598–1.210785 | — |
| `content-dirty-rows` | moving | single / 1 | 240 / 3 | 1/1 | 2.470989 | 0.460590 | 0.186399 | 0.177648–0.197078 | — |
| `content-dirty-rows` | static | single / 1 | 240 / 3 | 1/1 | 2.475162 | 0.432514 | 0.174742 | 0.165496–0.176562 | — |
| `content-dirty-rows` | unused | single / 1 | 240 / 3 | 1/1 | 2.848135 | 1.278025 | 0.448723 | 0.447172–0.463847 | — |

### evdi-idle-optimization-20261105/fusion-benchmark

Evidence ID: `history/evdi-idle-optimization-20261105/fusion-benchmark.json`. Original SHA-256: `6a1bb7b31a028e0379674113bfcf991220cca27c7b3b89e2b6088e78098d4d17`. Denominator: `release-original`. 36 raw runs, 4 case groups.

Scope: same original 240-grab synthetic signal; process CPU, one converter thread; 3 rotating-order repeats. Estimator: ratio of each variant median CPU time to denominator median CPU time.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `content-dirty-rows` | full | single / 1 | 240 / 3 | 1/1 | 2.720133 | 3.286295 | 1.208138 | 1.198115–1.212420 | — |
| `reference-band-fusion` | full | single / 1 | 240 / 3 | 1/1 | 2.720133 | 3.231457 | 1.187978 | 1.180292–1.188418 | — |
| `content-dirty-rows` | moving | single / 1 | 240 / 3 | 1/1 | 2.466007 | 0.451128 | 0.182939 | 0.181713–0.211719 | — |
| `reference-band-fusion` | moving | single / 1 | 240 / 3 | 1/1 | 2.466007 | 0.497842 | 0.201882 | 0.186403–0.210297 | — |
| `content-dirty-rows` | static | single / 1 | 240 / 3 | 1/1 | 2.475066 | 0.432045 | 0.174559 | 0.171408–0.175527 | — |
| `reference-band-fusion` | static | single / 1 | 240 / 3 | 1/1 | 2.475066 | 0.464665 | 0.187738 | 0.181272–0.192962 | — |
| `content-dirty-rows` | unused | single / 1 | 240 / 3 | 1/1 | 2.870852 | 1.279067 | 0.445536 | 0.442380–0.461863 | — |
| `reference-band-fusion` | unused | single / 1 | 240 / 3 | 1/1 | 2.870852 | 1.299063 | 0.452501 | 0.442662–0.477800 | — |

### evdi-idle-optimization-20261105/vectorization-benchmark

Evidence ID: `history/evdi-idle-optimization-20261105/vectorization-benchmark.json`. Original SHA-256: `58bcace40dfef8e75f0f773844282173f5cedcfa28e47eef978773b98ef98289`. Denominator: `release-original`. 48 raw runs, 4 case groups.

Scope: same original 240-grab synthetic signal; process CPU, one converter thread; 3 rotating-order repeats. Estimator: ratio of each variant median CPU time to denominator median CPU time.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `content-dirty-rows` | full | single / 1 | 240 / 3 | 1/1 | 5.817177 | 6.566552 | 1.128821 | 1.089942–1.129692 | — |
| `reference-band-fusion` | full | single / 1 | 240 / 3 | 1/1 | 5.817177 | 6.987484 | 1.201181 | 1.065128–1.472936 | — |
| `native-compiler-vectorized` | full | single / 1 | 240 / 3 | 1/1 | 5.817177 | 4.561182 | 0.784089 | 0.730298–0.784089 | — |
| `content-dirty-rows` | moving | single / 1 | 240 / 3 | 1/1 | 5.260731 | 1.195283 | 0.227209 | 0.218790–0.262283 | — |
| `reference-band-fusion` | moving | single / 1 | 240 / 3 | 1/1 | 5.260731 | 1.035914 | 0.196914 | 0.193158–0.237783 | — |
| `native-compiler-vectorized` | moving | single / 1 | 240 / 3 | 1/1 | 5.260731 | 1.115942 | 0.212127 | 0.212032–0.235691 | — |
| `content-dirty-rows` | static | single / 1 | 240 / 3 | 1/1 | 4.629012 | 1.042941 | 0.225305 | 0.203046–0.258022 | — |
| `reference-band-fusion` | static | single / 1 | 240 / 3 | 1/1 | 4.629012 | 1.003512 | 0.216788 | 0.188884–0.248268 | — |
| `native-compiler-vectorized` | static | single / 1 | 240 / 3 | 1/1 | 4.629012 | 1.032541 | 0.223059 | 0.215569–0.232202 | — |
| `content-dirty-rows` | unused | single / 1 | 240 / 3 | 1/1 | 5.353421 | 2.507829 | 0.468454 | 0.468454–0.486231 | — |
| `reference-band-fusion` | unused | single / 1 | 240 / 3 | 1/1 | 5.353421 | 2.996076 | 0.559656 | 0.409850–0.615098 | — |
| `native-compiler-vectorized` | unused | single / 1 | 240 / 3 | 1/1 | 5.353421 | 2.756590 | 0.514921 | 0.476461–0.596201 | — |

### evdi-idle-optimization-20261105/pinned-benchmark-native-vectorized

Evidence ID: `history/evdi-idle-optimization-20261105/pinned-benchmark-native-vectorized.json`. Original SHA-256: `1134b587be0ae14531492dc2bd3d651110bda50f50fff06609fd65a6932bf131`. Denominator: `release-original`. 126 raw runs, 11 case groups.

Scope: synthetic helper-only benchmark; bootstrap from few paired repeats; not production power measurement. Estimator: median of paired CPU ratios.

Method: paired CPU-time ratios; alternating order; same permitted CPU for single-thread cases; source-update cost included equally.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `native-compiler-vectorized` | bar | pool / 1 | 120 / 5 | 8/8 | 2.908907 | 2.677285 | 0.914286 | 0.876795–1.021216 | 0.876795–1.021216 |
| `native-compiler-vectorized` | bar | single / 1 | 240 / 7 | 1/1 | 2.486728 | 2.264633 | 0.910832 | 0.889042–0.934909 | 0.899635–0.923761 |
| `native-compiler-vectorized` | full | pool / 1 | 120 / 5 | 8/8 | 2.932802 | 2.471142 | 0.826863 | 0.795642–0.850488 | 0.795642–0.850488 |
| `native-compiler-vectorized` | full | single / 1 | 240 / 7 | 1/1 | 2.785280 | 2.097715 | 0.745339 | 0.730513–0.759318 | 0.739410–0.755164 |
| `native-compiler-vectorized` | full | single / 2 | 60 / 3 | 1/1 | 9.459734 | 11.302386 | 1.204599 | 1.194789–1.205215 | 1.194789–1.205215 |
| `native-compiler-vectorized` | moving | single / 1 | 240 / 7 | 1/1 | 2.486372 | 0.479569 | 0.193534 | 0.171016–0.238235 | 0.178500–0.201036 |
| `native-compiler-vectorized` | noise | pool / 1 | 120 / 5 | 8/8 | 3.272926 | 2.929660 | 0.913399 | 0.851477–0.970476 | 0.851477–0.970476 |
| `native-compiler-vectorized` | noise | single / 1 | 240 / 7 | 1/1 | 3.087231 | 2.466715 | 0.786010 | 0.690029–0.810143 | 0.759749–0.801596 |
| `native-compiler-vectorized` | noise | single / 2 | 60 / 3 | 1/1 | 9.990620 | 12.321988 | 1.229909 | 1.198803–1.259836 | 1.198803–1.259836 |
| `native-compiler-vectorized` | static | single / 1 | 240 / 7 | 1/1 | 2.487327 | 0.536004 | 0.215221 | 0.171592–0.237053 | 0.179256–0.235471 |
| `native-compiler-vectorized` | unused | single / 1 | 240 / 7 | 1/1 | 2.869914 | 1.343303 | 0.467180 | 0.460927–0.476792 | 0.461841–0.473652 |

### evdi-idle-optimization-20261105/pinned-benchmark

Evidence ID: `history/evdi-idle-optimization-20261105/pinned-benchmark.json`. Original SHA-256: `28ec505994c78f12f59bfb8ebd8a0d5a2e14790a73a4ed9dc7cb5293e54ca335`. Denominator: `release-original`. 126 raw runs, 11 case groups.

Scope: synthetic helper-only benchmark; bootstrap from few paired repeats; not production power measurement. Estimator: median of paired CPU ratios.

Method: paired CPU-time ratios; alternating order; same permitted CPU for single-thread cases; source-update cost included equally.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `shared-converter` | bar | pool / 1 | 120 / 5 | 8/8 | 2.865168 | 2.760048 | 0.963311 | 0.884222–1.031299 | 0.884222–1.031299 |
| `shared-converter` | bar | single / 1 | 240 / 7 | 1/1 | 2.495567 | 2.269015 | 0.905865 | 0.885114–0.918591 | 0.898237–0.917715 |
| `shared-converter` | full | pool / 1 | 120 / 5 | 8/8 | 2.912569 | 2.439525 | 0.843922 | 0.806951–0.857548 | 0.806951–0.857548 |
| `shared-converter` | full | single / 1 | 240 / 7 | 1/1 | 2.752859 | 1.939991 | 0.704719 | 0.666852–0.711980 | 0.680129–0.708150 |
| `shared-converter` | full | single / 2 | 60 / 3 | 1/1 | 9.545878 | 8.692853 | 0.894283 | 0.884240–0.934152 | 0.884240–0.934152 |
| `shared-converter` | moving | single / 1 | 240 / 7 | 1/1 | 2.470431 | 0.484009 | 0.196265 | 0.184846–0.263679 | 0.185087–0.236860 |
| `shared-converter` | noise | pool / 1 | 120 / 5 | 8/8 | 3.284533 | 3.005394 | 0.913664 | 0.894699–0.957735 | 0.894699–0.957735 |
| `shared-converter` | noise | single / 1 | 240 / 7 | 1/1 | 3.053252 | 2.365450 | 0.772121 | 0.756133–0.794959 | 0.768628–0.792634 |
| `shared-converter` | noise | single / 2 | 60 / 3 | 1/1 | 10.317943 | 9.240246 | 0.903762 | 0.895551–0.909405 | 0.895551–0.909405 |
| `shared-converter` | static | single / 1 | 240 / 7 | 1/1 | 2.475729 | 0.414151 | 0.166414 | 0.161025–0.206989 | 0.164723–0.206587 |
| `shared-converter` | unused | single / 1 | 240 / 7 | 1/1 | 2.860615 | 1.316388 | 0.460176 | 0.446269–0.475375 | 0.448520–0.468996 |

### evdi-further-20261106/specialized-benchmark

Evidence ID: `history/evdi-further-20261106/specialized-benchmark.json`. Original SHA-256: `a3ad63c2ac63b74a564539959a7676c918a81a10d92237bd6a7eaa67a70db565`. Denominator: `shared-converter`. 72 raw runs, 12 case groups.

Scope: Synthetic process CPU, including conversion workers and matched synthetic input updates.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `specialized-channels` | bar | pool / 1 | 120 / 3 | 8/8 | 2.657753 | 2.589677 | 0.974386 | 0.937734–1.013119 | — |
| `specialized-channels` | bar | single / 1 | 120 / 3 | 1/1 | 2.206680 | 2.142192 | 0.970776 | 0.943970–0.996618 | — |
| `specialized-channels` | full | pool / 1 | 120 / 3 | 8/8 | 2.403464 | 2.322545 | 0.944506 | 0.933121–1.026093 | — |
| `specialized-channels` | full | single / 1 | 120 / 3 | 1/1 | 1.874008 | 1.904997 | 1.019423 | 1.007481–1.065112 | — |
| `specialized-channels` | full | single / 2 | 120 / 3 | 1/1 | 8.297883 | 8.237382 | 0.992170 | 0.989341–0.996824 | — |
| `specialized-channels` | moving | pool / 1 | 120 / 3 | 8/8 | 0.595893 | 0.599845 | 1.006632 | 0.814568–1.172533 | — |
| `specialized-channels` | moving | single / 1 | 120 / 3 | 1/1 | 0.457623 | 0.462821 | 0.987505 | 0.832599–1.054134 | — |
| `specialized-channels` | noise | pool / 1 | 120 / 3 | 8/8 | 2.890758 | 2.806660 | 0.981512 | 0.920453–0.987826 | — |
| `specialized-channels` | noise | single / 1 | 120 / 3 | 1/1 | 2.293898 | 2.336491 | 1.024251 | 1.003681–1.027859 | — |
| `specialized-channels` | noise | single / 2 | 120 / 3 | 1/1 | 9.064385 | 9.075431 | 1.004307 | 0.993380–1.005989 | — |
| `specialized-channels` | static | single / 1 | 120 / 3 | 1/1 | 0.570717 | 0.529447 | 0.955126 | 0.927687–0.996528 | — |
| `specialized-channels` | unused | single / 1 | 120 / 3 | 1/1 | 1.326027 | 1.288701 | 0.975810 | 0.963944–1.044548 | — |

### evdi-further-20261106/blocked-benchmark

Evidence ID: `history/evdi-further-20261106/blocked-benchmark.json`. Original SHA-256: `02615cc9721b65ffc45747cc1a36fd6a1d2887eb9e0d13845bd0f8d72ec17eca`. Denominator: `shared-converter`. 72 raw runs, 12 case groups.

Scope: Synthetic process CPU, including conversion workers and matched synthetic input updates.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `blocked-no-first-pixel` | bar | pool / 1 | 120 / 3 | 8/8 | 2.645926 | 2.433910 | 0.933289 | 0.919871–0.933375 | — |
| `blocked-no-first-pixel` | bar | single / 1 | 120 / 3 | 1/1 | 2.273167 | 2.019056 | 0.888213 | 0.878446–0.916607 | — |
| `blocked-no-first-pixel` | full | pool / 1 | 120 / 3 | 8/8 | 2.353792 | 2.376862 | 0.987425 | 0.954161–1.047455 | — |
| `blocked-no-first-pixel` | full | single / 1 | 120 / 3 | 1/1 | 1.871435 | 1.955249 | 1.047415 | 1.027909–1.068812 | — |
| `blocked-no-first-pixel` | full | single / 2 | 120 / 3 | 1/1 | 8.235571 | 8.337378 | 1.012362 | 1.000131–1.016133 | — |
| `blocked-no-first-pixel` | moving | pool / 1 | 120 / 3 | 8/8 | 0.667418 | 0.597914 | 0.895861 | 0.829547–1.121709 | — |
| `blocked-no-first-pixel` | moving | single / 1 | 120 / 3 | 1/1 | 0.527116 | 0.458348 | 0.935421 | 0.821684–0.995355 | — |
| `blocked-no-first-pixel` | noise | pool / 1 | 120 / 3 | 8/8 | 2.921678 | 2.961572 | 1.015572 | 0.995760–1.033334 | — |
| `blocked-no-first-pixel` | noise | single / 1 | 120 / 3 | 1/1 | 2.369050 | 2.290921 | 0.984462 | 0.938045–1.001663 | — |
| `blocked-no-first-pixel` | noise | single / 2 | 120 / 3 | 1/1 | 9.050081 | 9.125933 | 1.007834 | 1.005941–1.008507 | — |
| `blocked-no-first-pixel` | static | single / 1 | 120 / 3 | 1/1 | 0.466764 | 0.486564 | 1.013151 | 1.000256–1.401069 | — |
| `blocked-no-first-pixel` | unused | single / 1 | 120 / 3 | 1/1 | 1.297208 | 1.063164 | 0.832616 | 0.772201–1.003619 | — |

### evdi-further-20261106/banded-benchmark

Evidence ID: `history/evdi-further-20261106/banded-benchmark.json`. Original SHA-256: `ae6c6f65408b03ead4a8bc8039be0546d8e8557f724ee433dc1de8d986d8ab86`. Denominator: `shared-converter`. 168 raw runs, 12 case groups.

Scope: Synthetic process CPU, including conversion workers and matched synthetic input updates.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `banded-compare` | bar | pool / 1 | 240 / 7 | 8/8 | 2.660911 | 2.505513 | 0.940463 | 0.894101–0.960221 | 0.912703–0.947999 |
| `banded-compare` | bar | single / 1 | 240 / 7 | 1/1 | 2.283987 | 2.074382 | 0.909430 | 0.878466–0.943216 | 0.890929–0.937992 |
| `banded-compare` | full | pool / 1 | 240 / 7 | 8/8 | 2.340089 | 2.384376 | 1.016271 | 0.978171–1.059129 | 0.992755–1.049585 |
| `banded-compare` | full | single / 1 | 240 / 7 | 1/1 | 1.905539 | 1.907309 | 0.997357 | 0.957404–1.057727 | 0.959834–1.036720 |
| `banded-compare` | full | single / 2 | 120 / 7 | 1/1 | 8.226914 | 8.276951 | 1.004965 | 0.993976–1.007954 | 1.002388–1.007699 |
| `banded-compare` | moving | pool / 1 | 1000 / 7 | 8/8 | 0.603767 | 0.597272 | 0.981850 | 0.958512–1.071294 | 0.974457–0.989242 |
| `banded-compare` | moving | single / 1 | 1000 / 7 | 1/1 | 0.458518 | 0.459840 | 1.007830 | 0.954619–1.048933 | 0.955586–1.045874 |
| `banded-compare` | noise | pool / 1 | 240 / 7 | 8/8 | 2.911440 | 2.924693 | 1.004552 | 0.983465–1.038539 | 0.989915–1.021737 |
| `banded-compare` | noise | single / 1 | 240 / 7 | 1/1 | 2.315777 | 2.390921 | 1.024968 | 0.973644–1.048680 | 1.013431–1.036266 |
| `banded-compare` | noise | single / 2 | 120 / 7 | 1/1 | 9.095661 | 9.110904 | 1.003510 | 0.987789–1.018981 | 0.997982–1.015729 |
| `banded-compare` | static | single / 1 | 1000 / 7 | 1/1 | 0.445217 | 0.448119 | 1.026016 | 0.881706–1.110008 | 0.923917–1.089642 |
| `banded-compare` | unused | single / 1 | 240 / 7 | 1/1 | 1.294336 | 1.017116 | 0.776845 | 0.755881–0.814304 | 0.765328–0.803075 |

### evdi-further-20261106/blockedfast-benchmark

Evidence ID: `history/evdi-further-20261106/blockedfast-benchmark.json`. Original SHA-256: `913700daa50c372302444f2b4df7fcf5537eb43ec8bcf4d75a781082d48db04d`. Denominator: `shared-converter`. 168 raw runs, 12 case groups.

Scope: Synthetic process CPU, including conversion workers and matched synthetic input updates.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `blockedfast-compare` | bar | pool / 1 | 240 / 7 | 8/8 | 2.710102 | 2.489209 | 0.919433 | 0.856978–0.967527 | 0.882304–0.940830 |
| `blockedfast-compare` | bar | single / 1 | 240 / 7 | 1/1 | 2.242605 | 2.086913 | 0.920024 | 0.895506–0.966687 | 0.904843–0.939029 |
| `blockedfast-compare` | full | pool / 1 | 240 / 7 | 8/8 | 2.358303 | 2.389145 | 1.017959 | 0.952908–1.028859 | 0.972432–1.020787 |
| `blockedfast-compare` | full | single / 1 | 240 / 7 | 1/1 | 1.921818 | 1.887736 | 0.982069 | 0.947937–1.038675 | 0.970642–1.011055 |
| `blockedfast-compare` | full | single / 2 | 120 / 7 | 1/1 | 8.269945 | 8.307161 | 1.004500 | 0.995069–1.014831 | 1.003586–1.010155 |
| `blockedfast-compare` | moving | pool / 1 | 1000 / 7 | 8/8 | 0.599537 | 0.606632 | 1.004181 | 0.930081–1.027048 | 0.937786–1.022256 |
| `blockedfast-compare` | moving | single / 1 | 1000 / 7 | 1/1 | 0.469023 | 0.465179 | 0.986999 | 0.941644–1.050880 | 0.961492–1.025134 |
| `blockedfast-compare` | noise | pool / 1 | 240 / 7 | 8/8 | 2.931699 | 2.888333 | 0.985417 | 0.895583–0.994330 | 0.967372–0.990422 |
| `blockedfast-compare` | noise | single / 1 | 240 / 7 | 1/1 | 2.330734 | 2.427061 | 1.019011 | 0.976134–1.066674 | 0.992457–1.059323 |
| `blockedfast-compare` | noise | single / 2 | 120 / 7 | 1/1 | 9.085730 | 9.107939 | 1.001874 | 0.989700–1.023640 | 1.000159–1.016423 |
| `blockedfast-compare` | static | single / 1 | 1000 / 7 | 1/1 | 0.471480 | 0.457026 | 0.966522 | 0.907763–1.050136 | 0.948088–0.980943 |
| `blockedfast-compare` | unused | single / 1 | 240 / 7 | 1/1 | 1.307213 | 1.029900 | 0.778355 | 0.742290–0.838421 | 0.766504–0.830547 |

### publication-20261106/publication-benchmark

Evidence ID: `history/publication-20261106/publication-benchmark.json`. Original SHA-256: `84513891d9ac9c3d5d8069d8ee1682534187bb42fee45c4ea783136427618077`. Denominator: `upstream-publication-base`. 40 raw runs, 4 case groups.

Scope: Synthetic process CPU, including input updates and worker threads; no real EVDI copy, FIFO, FFmpeg, GPU or end-to-end latency. Five alternating pairs per case, GCC -O3, 1536x960 output.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `publication-integrated` | full | pool / 1 | 240 / 5 | 8/8 | 2.936387 | 2.372745 | 0.816214 | 0.804904–0.831981 | — |
| `publication-integrated` | full | single / 2 | 60 / 5 | 1/1 | 9.400807 | 8.285289 | 0.880125 | 0.869827–0.903755 | — |
| `publication-integrated` | noise | pool / 1 | 240 / 5 | 8/8 | 3.321063 | 2.977277 | 0.903912 | 0.853895–0.921341 | — |
| `publication-integrated` | static | single / 1 | 240 / 5 | 1/1 | 2.475131 | 0.433177 | 0.175012 | 0.168320–0.195228 | — |

### evdi-simd-20261106/clones-benchmark

Evidence ID: `history/evdi-simd-20261106/clones-benchmark.json`. Original SHA-256: `356d0cb2273abcc3a1916da3ea63ae32842e276e1f78da618a81760a3f566a97`. Denominator: `publication-integrated`. 72 raw runs, 12 case groups.

Scope: Process CPU including workers and synthetic source updates; excludes real EVDI, FIFO, FFmpeg and GPU. Alternating pairs, same affinity, generic GCC -O3.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `avx2-clones` | bar | pool / 1 | 240 / 3 | 8/8 | 2.408144 | 2.989682 | 1.241488 | 1.179934–1.266756 | — |
| `avx2-clones` | bar | single / 1 | 240 / 3 | 1/1 | 2.019652 | 2.569028 | 1.246240 | 1.243183–1.286964 | — |
| `avx2-clones` | full | pool / 1 | 240 / 3 | 8/8 | 2.360060 | 2.759148 | 1.157935 | 1.148470–1.197505 | — |
| `avx2-clones` | full | single / 1 | 240 / 3 | 1/1 | 1.871441 | 2.375716 | 1.291567 | 1.258772–1.297588 | — |
| `avx2-clones` | full | single / 2 | 60 / 3 | 1/1 | 8.209469 | 8.629369 | 1.061456 | 1.045881–1.069912 | — |
| `avx2-clones` | full | single / 4 | 60 / 3 | 1/1 | 26.467209 | 26.594550 | 1.007812 | 0.986915–1.013978 | — |
| `avx2-clones` | noise | pool / 1 | 240 / 3 | 8/8 | 2.874492 | 3.346560 | 1.163394 | 1.161268–1.184296 | — |
| `avx2-clones` | noise | pool / 4 | 60 / 3 | 8/8 | 48.057797 | 48.428222 | 1.007708 | 1.002237–1.008462 | — |
| `avx2-clones` | noise | single / 1 | 240 / 3 | 1/1 | 2.343812 | 2.793081 | 1.191683 | 1.175774–1.195307 | — |
| `avx2-clones` | noise | single / 2 | 60 / 3 | 1/1 | 9.007117 | 9.373787 | 1.040974 | 1.015863–1.043836 | — |
| `avx2-clones` | static | single / 1 | 240 / 3 | 1/1 | 0.426975 | 0.430494 | 1.014057 | 1.003468–1.042710 | — |
| `avx2-clones` | unused | single / 1 | 240 / 3 | 1/1 | 1.025277 | 0.970773 | 0.960429 | 0.946839–1.002327 | — |

### evdi-simd-20261106/sse41-benchmark

Evidence ID: `history/evdi-simd-20261106/sse41-benchmark.json`. Original SHA-256: `cb3a543bb8409c5440cc0d2e7fbe958335ae2d03bd42970c4e51d971bbfb2be8`. Denominator: `publication-integrated`. 72 raw runs, 12 case groups.

Scope: Process CPU including workers and synthetic source updates; excludes real EVDI, FIFO, FFmpeg and GPU. Alternating pairs, same affinity, generic GCC -O3.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `sse41-clones` | bar | pool / 1 | 240 / 3 | 8/8 | 2.463366 | 2.473401 | 1.004074 | 0.995140–1.029066 | — |
| `sse41-clones` | bar | single / 1 | 240 / 3 | 1/1 | 2.016138 | 2.111903 | 1.047499 | 1.029988–1.056972 | — |
| `sse41-clones` | full | pool / 1 | 240 / 3 | 8/8 | 2.621003 | 2.618310 | 1.013687 | 0.979541–1.072096 | — |
| `sse41-clones` | full | single / 1 | 240 / 3 | 1/1 | 2.224412 | 1.991360 | 0.859226 | 0.855603–0.999780 | — |
| `sse41-clones` | full | single / 2 | 60 / 3 | 1/1 | 8.163284 | 8.334076 | 1.007537 | 0.998767–1.028729 | — |
| `sse41-clones` | full | single / 4 | 60 / 3 | 1/1 | 27.752232 | 26.956666 | 0.971333 | 0.939597–1.077361 | — |
| `sse41-clones` | noise | pool / 1 | 240 / 3 | 8/8 | 2.899401 | 2.969945 | 1.024331 | 1.006287–1.051662 | — |
| `sse41-clones` | noise | pool / 4 | 60 / 3 | 8/8 | 48.100432 | 48.291212 | 1.003966 | 0.980420–1.073183 | — |
| `sse41-clones` | noise | single / 1 | 240 / 3 | 1/1 | 2.336470 | 2.436544 | 1.044247 | 0.978870–1.095394 | — |
| `sse41-clones` | noise | single / 2 | 60 / 3 | 1/1 | 9.026011 | 9.019158 | 0.998179 | 0.989053–1.025331 | — |
| `sse41-clones` | static | single / 1 | 240 / 3 | 1/1 | 0.661768 | 0.628062 | 0.949066 | 0.770121–1.188097 | — |
| `sse41-clones` | unused | single / 1 | 240 / 3 | 1/1 | 1.138045 | 1.040409 | 0.966165 | 0.914207–1.005638 | — |

### evdi-convert-20261106/sse2-benchmark

Evidence ID: `history/evdi-convert-20261106/sse2-benchmark.json`. Original SHA-256: `ba9867cce263c59d98dbe8682805338f46524e8da45a7a710a0a338b0c28a061`. Denominator: `writer-fifo`. 72 raw runs, 12 case groups.

Scope: Process CPU including workers and synthetic source updates; excludes real EVDI, FIFO, FFmpeg and GPU. Alternating pairs, same affinity, generic GCC -O3.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `handwritten-sse2` | bar | pool / 1 | 240 / 3 | 8/8 | 2.464317 | 2.465928 | 1.005158 | 0.959541–1.030218 | — |
| `handwritten-sse2` | bar | single / 1 | 240 / 3 | 1/1 | 2.080555 | 2.098056 | 1.004213 | 0.984357–1.012915 | — |
| `handwritten-sse2` | full | pool / 1 | 240 / 3 | 8/8 | 2.406438 | 2.420039 | 1.003887 | 1.003360–1.006806 | — |
| `handwritten-sse2` | full | single / 1 | 240 / 3 | 1/1 | 1.847486 | 1.917920 | 1.039879 | 0.977143–1.049622 | — |
| `handwritten-sse2` | full | single / 2 | 60 / 3 | 1/1 | 8.332913 | 8.276900 | 0.993310 | 0.993278–0.999338 | — |
| `handwritten-sse2` | full | single / 4 | 60 / 3 | 1/1 | 26.680519 | 26.726751 | 1.001733 | 0.993298–1.007951 | — |
| `handwritten-sse2` | noise | pool / 1 | 240 / 3 | 8/8 | 2.854167 | 2.961379 | 1.019246 | 1.001532–1.037563 | — |
| `handwritten-sse2` | noise | pool / 4 | 60 / 3 | 8/8 | 48.414448 | 47.674425 | 0.980960 | 0.980145–0.998083 | — |
| `handwritten-sse2` | noise | single / 1 | 240 / 3 | 1/1 | 2.290256 | 2.330891 | 1.017743 | 0.986295–1.019643 | — |
| `handwritten-sse2` | noise | single / 2 | 60 / 3 | 1/1 | 9.018443 | 9.119760 | 1.002022 | 0.984347–1.021993 | — |
| `handwritten-sse2` | static | single / 1 | 240 / 3 | 1/1 | 0.426010 | 0.434238 | 1.050562 | 0.956922–1.145142 | — |
| `handwritten-sse2` | unused | single / 1 | 240 / 3 | 1/1 | 0.996110 | 1.006418 | 1.010348 | 0.986543–1.064311 | — |

### evdi-convert-20261106/fused-benchmark

Evidence ID: `history/evdi-convert-20261106/fused-benchmark.json`. Original SHA-256: `bf64d3c709e4a77faf3d991fe6c2ebd53bf58f661a2db07fa1461842c951f4a8`. Denominator: `writer-fifo`. 72 raw runs, 12 case groups.

Scope: Process CPU including workers and synthetic source updates; excludes real EVDI, FIFO, FFmpeg and GPU. Alternating pairs, same affinity, generic GCC -O3.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `native-reference-conversion-fused` | bar | pool / 1 | 240 / 3 | 8/8 | 2.450188 | 2.049610 | 0.836622 | 0.825237–0.859056 | — |
| `native-reference-conversion-fused` | bar | single / 1 | 240 / 3 | 1/1 | 2.026670 | 1.599035 | 0.789118 | 0.782221–0.840057 | — |
| `native-reference-conversion-fused` | full | pool / 1 | 240 / 3 | 8/8 | 2.430574 | 2.004702 | 0.822895 | 0.764520–0.841866 | — |
| `native-reference-conversion-fused` | full | single / 1 | 240 / 3 | 1/1 | 2.107971 | 1.557752 | 0.738982 | 0.734885–0.751682 | — |
| `native-reference-conversion-fused` | full | single / 2 | 60 / 3 | 1/1 | 8.249784 | 8.171072 | 0.990459 | 0.990212–0.992772 | — |
| `native-reference-conversion-fused` | full | single / 4 | 60 / 3 | 1/1 | 27.613275 | 27.607050 | 0.986962 | 0.969914–1.012303 | — |
| `native-reference-conversion-fused` | noise | pool / 1 | 240 / 3 | 8/8 | 2.903980 | 2.503343 | 0.855222 | 0.846493–0.876608 | — |
| `native-reference-conversion-fused` | noise | pool / 4 | 60 / 3 | 8/8 | 49.040287 | 49.956535 | 0.987821 | 0.986012–1.037075 | — |
| `native-reference-conversion-fused` | noise | single / 1 | 240 / 3 | 1/1 | 2.418493 | 1.945811 | 0.816521 | 0.758860–0.819240 | — |
| `native-reference-conversion-fused` | noise | single / 2 | 60 / 3 | 1/1 | 9.082186 | 9.149569 | 1.007419 | 0.993613–1.011866 | — |
| `native-reference-conversion-fused` | static | single / 1 | 240 / 3 | 1/1 | 0.502786 | 0.550909 | 1.095713 | 1.065495–1.126824 | — |
| `native-reference-conversion-fused` | unused | single / 1 | 240 / 3 | 1/1 | 1.009916 | 0.967169 | 0.952083 | 0.943125–1.068007 | — |

### evdi-convert-20261106/fused-long-benchmark

Evidence ID: `history/evdi-convert-20261106/fused-long-benchmark.json`. Original SHA-256: `e752567318955f7fb23ec778c0f8eadd25c0744512ee31903f9746b697e0756f`. Denominator: `writer-fifo`. 196 raw runs, 14 case groups.

Scope: Process CPU including workers and synthetic source updates; excludes real EVDI, FIFO, FFmpeg and GPU. Alternating pairs, same affinity, generic GCC -O3.. Estimator: median of paired CPU ratios.

| Candidate | Case | Mode / scale | Grabs / pairs | Threads C/B | B CPU ms/grab | C CPU ms/grab | Original ratio C/B | Observed pair range | Historical CI95 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| `native-reference-conversion-fused` | bar | pool / 1 | 240 / 7 | 8/8 | 2.450233 | 2.029601 | 0.827684 | 0.785509–0.864736 | — |
| `native-reference-conversion-fused` | bar | single / 1 | 240 / 7 | 1/1 | 2.073744 | 1.638177 | 0.799852 | 0.743988–0.824375 | — |
| `native-reference-conversion-fused` | full | pool / 1 | 240 / 7 | 8/8 | 2.367135 | 1.899210 | 0.798841 | 0.773688–0.836947 | — |
| `native-reference-conversion-fused` | full | single / 1 | 240 / 7 | 1/1 | 1.878058 | 1.472283 | 0.781872 | 0.750896–0.805690 | — |
| `native-reference-conversion-fused` | full | single / 2 | 120 / 7 | 1/1 | 8.236529 | 8.246472 | 1.002116 | 0.989803–1.018934 | — |
| `native-reference-conversion-fused` | full | single / 4 | 60 / 7 | 1/1 | 27.105962 | 26.907041 | 0.992229 | 0.976354–1.016224 | — |
| `native-reference-conversion-fused` | moving | pool / 1 | 960 / 7 | 8/8 | 0.592259 | 0.586284 | 0.989590 | 0.955899–1.055376 | — |
| `native-reference-conversion-fused` | moving | single / 1 | 960 / 7 | 1/1 | 0.461435 | 0.446811 | 0.974015 | 0.924998–1.078296 | — |
| `native-reference-conversion-fused` | noise | pool / 1 | 240 / 7 | 8/8 | 2.877494 | 2.501041 | 0.858565 | 0.840652–0.887334 | — |
| `native-reference-conversion-fused` | noise | pool / 4 | 60 / 7 | 8/8 | 58.583887 | 57.022437 | 1.015182 | 0.934102–1.375750 | — |
| `native-reference-conversion-fused` | noise | single / 1 | 240 / 7 | 1/1 | 2.330561 | 1.852506 | 0.798491 | 0.760496–0.852450 | — |
| `native-reference-conversion-fused` | noise | single / 2 | 120 / 7 | 1/1 | 9.277365 | 9.082802 | 0.979354 | 0.960773–1.014694 | — |
| `native-reference-conversion-fused` | static | single / 1 | 960 / 7 | 1/1 | 0.466510 | 0.436774 | 0.940443 | 0.820523–1.098218 | — |
| `native-reference-conversion-fused` | unused | single / 1 | 960 / 7 | 1/1 | 1.010727 | 1.006598 | 0.988425 | 0.952959–1.055296 | — |

## Correctness and behavioral evidence

Recorded deterministic conversion results cover 136 unique cases, eight pixel formats (`XR24`, `AR24`, `XB24`, `AB24`, `RX24`, `RA24`, `BX24`, `BA24`) and scale factors 1–4. The preserved shared-converter, early-fusion, vectorized, handwritten-SSE2, later-fused and fallback result files have identical per-case output hashes. These are existing results checked for identity and count during report construction, not six fresh test executions. Pixel equivalence establishes output equality for these fixtures; it does not establish a performance win for a rejected implementation.

The retained work preserves the distinction between used RGB values and unused-channel noise, exact row comparison, keepalive behavior, frame publication, buffer ownership and writer progress. A changed RGB value can disappear in NV12 quantization; exact RGB detection is therefore stronger than comparing a few sampled output hashes. The historical mutation and regression logs test specific paths and are identified in the evidence JSON. Initial test environments lacked sanitizer runtime libraries, so no passing sanitizer claim is inferred from those historical logs.

The writer/FIFO evidence covers the mode-change buffer claim, immediate wakeup on publication, keepalive waiting and capacity fallback. A temporary FIFO resource shortage and later capacity restoration were environment/resource events. A smaller or restored pipe is not a measured color-converter speedup, and the passive CPU snapshots below cannot isolate that change.

## Passive installed observations

These four recorded CPU observations use different interval lengths and uncontrolled desktop content. Their names include “before” and “after” because those were the original recording names; that does not make them a matched benchmark. Percentages are fractions of one logical core. No ratio or causal improvement is derived from them.

| Logical evidence ID | Interval, seconds | UScreen CPU % | Helper CPU % | FFmpeg CPU % |
| --- | ---: | ---: | ---: | ---: |
| `history/evdi-convert-20261106/cpu-after.json` | 20.001742 | 0.55 | 2.45 | 2.45 |
| `history/evdi-convert-20261106/cpu-final.json` | 20.000464 | 0.50 | 2.10 | 2.30 |
| `history/evdi-install-20261106/cpu-after.json` | 15.002739 | 0.60 | 5.47 | 3.60 |
| `history/evdi-install-20261106/cpu-before.json` | 5.002095 | 0.80 | 11.40 | 4.80 |

The writer round also preserved two passive context-switch samples. The ten-second “before” sample recorded 232.08 voluntary writer context switches/s; the twenty-second “after” sample recorded 26.24/s. The latter recorded helper CPU of 1.25%, UScreen CPU of 0.50%, and FFmpeg CPU of 2.35% of one core. Their content and background activity were not held constant. These observations explain why an idle-wakeup change was investigated, but they do not establish a controlled CPU, GPU or energy saving. The exact interval lengths and sanitized numerical values are in `passive_writer_observations` in the JSON.

Separately, the controlled writer regression fixture recorded 45 voluntary context switches with the old waiting behavior and six with one converter thread or seven with the pool after the change. This counter also includes other blocking operations; it is not an exact count of condition waits. Four keepalive frames were still emitted during 40 unchanged grabs. A subsequent real change reached the private test pipe in 0–1 ms, and removing the publication signal made the mutant fail its 100 ms limit. Those are behavior-specific fixture results, not an isolated CPU benchmark. The keepalive deadline is 200 ms; the old 60 fps timeout woke at approximately 16.7 ms intervals.

The FIFO regression reproduced 65,536 bytes with the old one-shot growth request and 524,288 bytes after fallback. The candidate attempts 1 MiB, 512 KiB, 256 KiB, 128 KiB and 64 KiB in descending order, stopping on success and preserving a usable pipe if every request fails. Later, resource conditions changed and unprivileged growth requests were denied even for smaller capacities. An administrative adjustment restored 524,288 bytes on the existing live FIFO only; it did not guarantee that capacity on a future unprivileged restart. Neither an eightfold capacity change nor a temporary resource adjustment is evidence of an eightfold throughput increase or a converter CPU reduction.

High idle display latency remained unresolved in these historical live checks. The writer-round journal showed `encode→display` medians of approximately **795–817 ms**, with `capture→fifo` medians of approximately **2.4–4.0 ms**. The later native-fusion round showed approximately **763–815 ms** and **3.7–4.5 ms**, respectively. These source-reported ranges describe the observed idle regime, not a controlled latency comparison or an established root cause in Android. The helper CPU work did not prove that interactive display latency had been fixed. Source identities and metric scope are preserved in `source_reported_behavior_metrics` in the JSON.

## Excluded trials and limits

The rejected algorithms above remain measured comparisons. The following preliminary trials are different: they cannot support the indicated behavioral or causal claims.

| Trial | Disposition and reason |
| --- | --- |
| `first-full-reference-copy` | Full reference copy plus damage-based full conversion caused a measured last-pixel-change regression. The exact recalculated ratio is in its matched benchmark group. |
| `lagging-writer-fixture-failure` | The preserved trial ends with consume asserting g_latest_valid. Final retained source and later passing regression evidence are separate; this file is not evidence of a retained product regression or a performance measurement. |
| `private-trigger-pretrial` | Private preliminary phases recorded no encoder samples. The private server log also records a socket-listener failure. This does not prove the later live desktop trigger or quantify a production performance ratio. |
| `live-topology-precondition` | No phases and no samples were recorded; no valid trigger or ratio is available. |
| `live-already-busy-pretrial` | The preliminary live run started in the already-busy regime. It cannot establish that the tested action triggered the transition from a quiet regime. The later trigger-live run remains separate. |
| `ayugram-first-trial` | The recorded failure was TypeError: Object of type DictWrapper is not JSON serializable when serializing wm_state in phases. The record explicitly says numeric_result_available=false and excludes this trial from causal claims. It also records that a client map-state check alone did not prove normal restoration; the subsequent corrected trial was retained separately. |

No fixed share of Xorg/Cinnamon load, host GPU utilization, total host CPU, energy use, or end-to-end display latency is attributed to the final helper. Static and busy desktop regimes varied during the investigation; copied profile percentages and short live snapshots cannot fill that gap. The older project-wide numbers in [benchmarks.md](benchmarks.md) describe separate hardware and dates and are not denominators for this series.

## Publication and external maintainer results

[PR #49](https://github.com/majmichu1/UScreen/pull/49) was merged on **6 November 2026 at 03:38:38 UTC**, as `5ee4d2823dcbcc3ed29c147bfc9f8494d4ce1bbc`. The follow-up starts from `588a71f4d55e1556a40ec8b8a4d8172833b36927`, which also includes the upstream changelog/test-command update. The original publication benchmark remains a historical comparison of `6c09add` against `8fcfe9e`; the later upstream merge does not change its denominator.

The [maintainer's comment](https://github.com/majmichu1/UScreen/pull/49#issuecomment-6676221884) reports passing single-thread and pool tests under ASan/UBSan. It also reports approximate mocked 2960 × 1848 timings: static single-thread grabs decreased from about 7 ms to 2 ms, while full-change pool grabs increased from about 2.7 ms to 4.7 ms. These external results use another environment and retain the observed moving-frame regression. They are attributed observations, not locally reproduced measurements, not an added pair in our matrix, and not a local sanitizer success. At the time of that comment the maintainer had not yet tested on a tablet.

## Rechecking the numerical projection

The following standard-library Python check can be run from the repository root. It recalculates every published ratio and CPU cost from the sanitized recorded runs, checks CSV coverage, and checks the source identities attached to each comparison. It does not rerun a native benchmark or reconstruct a historical confidence interval.

```sh
python3 - <<'PY'
import csv, json, math, statistics
from pathlib import Path

data = json.loads(Path('docs/evdi-performance-evidence.json').read_text())
matrix = {r['comparison_id']: r for r in csv.DictReader(
    Path('docs/evdi-performance-matrix.csv').open(newline=''))
    if r['record_type'] == 'historical_cpu_comparison'}
seen = set()
groups = runs = 0
for bench in data['benchmarks']:
    runs += len(bench['raw_rows'])
    base = bench['denominator_variant']
    for group in bench['case_groups']:
        groups += 1
        samples = {}
        for variant, indices in group['raw_row_indices_by_variant'].items():
            samples[variant] = {bench['raw_rows'][i]['repeat']:
                               bench['raw_rows'][i] for i in indices}
            assert len(samples[variant]) == group['paired_repeats']
        for variant in bench['variants']:
            if variant == base:
                continue
            key = group['group_id'] + ':' + variant
            row = matrix[key]
            seen.add(key)
            ratios = [samples[variant][rep]['cpu_seconds'] /
                      samples[base][rep]['cpu_seconds']
                      for rep in group['repeat_ids']]
            cpu = statistics.median(r['cpu_seconds']
                                    for r in samples[variant].values())
            reference = statistics.median(r['cpu_seconds']
                                         for r in samples[base].values())
            checks = {
                'median_paired_cpu_ratio': statistics.median(ratios),
                'ratio_of_median_cpu_seconds': cpu / reference,
                'candidate_median_cpu_ms_per_simulated_grab':
                    cpu * 1000 / group['frames_per_variant_run'],
                'min_paired_cpu_ratio': min(ratios),
                'max_paired_cpu_ratio': max(ratios),
            }
            for field, value in checks.items():
                assert math.isclose(float(row[field]), value, rel_tol=1e-12)
            assert row['candidate_source_sha256'] == bench['variants'][variant]['source_sha256']
            assert row['denominator_source_sha256'] == bench['variants'][base]['source_sha256']
assert seen == set(matrix)
assert (len(data['stages']), len(data['benchmarks']), runs, groups, len(seen)) == (17, 16, 1382, 151, 163)
print('PASS: 17 source stages, 16 files, 1382 runs, 151 groups, 163 comparisons')
PY
```

The original confidence fields remain available for inspection with their original estimator and sample count. Repeated benchmarks, native regression execution, sanitizer execution, three-tablet validation, and an independently matched final-versus-upstream live comparison are separate checks; this report does not silently substitute a historical record for any of them.

The public numerical projection permits independent arithmetic checks, not a fresh reconstruction of every experimental executable. Rebuilding rejected candidates also requires the preserved source snapshots, patches, benchmark harness and matching compiler environment identified by the evidence records. Their portable identifiers and hashes do not imply that those original artifacts are included in this repository.

## Finite conversion-thread sweep

The native source `40cd003e…` was copied into four experimental variants differing only in `MAX_CONV_THREADS=1/2/4/8`. All ten workload/size combinations were checked with each cap, then measured in five alternating orders (`1,2,4,8`, then `8,4,2,1`). The two sizes were 1536 × 960 and 2560 × 1600; format was XR24 and scale was 1. Each run had 24 warmup frames and 180 subsequent frames. The full raw 200 timed runs, 40 byte-verification records and 40 summary rows are in `thread_sweep` in the numerical JSON. These records are not folded into the earlier input-mutation-inclusive CPU series.

The sweep ran on an Intel Core i5-1245U with 12 online logical CPUs, inherited affinity **4–10** and **nice 10**, using generic GCC 16.2.1 `-O3`. Eight workers therefore shared only seven allowed CPUs. A single synthetic helper ran at a time. The runner paused before timing while the coordinated compiler work finished; this was not proof of an otherwise exclusive machine, fixed frequency or fixed thermal state. The live experiment below had different scheduling conditions: CPUs **0–11**, nice **0**, and three simultaneous helpers.

The timed stage is the real `grab_now()` call with a stubbed full-damage EVDI boundary: RGB comparison, dirty masks, conversion and publication. It excludes synthetic source updates, setup, warmup, consume and byte-oracle work. Process CPU uses `CLOCK_PROCESS_CPUTIME_ID` and includes worker CPU and clock overhead; wall time uses `CLOCK_MONOTONIC`. There is no kernel framebuffer copy, FIFO, writer, FFmpeg or GPU. Stage wall time is not end-to-end display latency.

Byte verification compared all effective NV12 bytes with the actual one-thread full converter, saving/restoring the incremental RGB reference cache so the oracle could not repair it. The 40 verification runs compared **8,160 frames / 34,091,827,200 bytes**, with zero byte errors. This establishes equivalence to the production converter on the fixtures, not an independently implemented color-standard oracle. An independent evidence audit recomputed 2,470 checks and 521 manifest hashes and confirmed this finite scope.

All **36,000 timed frames** were below 16.7 ms for the measured stage. The largest observed frame was 10.678706 ms across all caps and 9.387729 ms at cap two. These maxima do not establish a pipeline-wide 60 fps guarantee. Absolute wall/CPU values below are medians of five run means. The p95 column is the median of run p95s, not a pooled p95. Ratios pair the same repeat against cap eight; ranges are observed minima/maxima, not CIs. Static and late-pixel cases include comparisons crossing one, and static frames skip conversion entirely.

| Cap | Exact experimental helper source SHA-256 |
| ---: | --- |
| 1 | `c402d4e5f05e2720ac8cd869eedbee39f08d6cb60a44449593946e33a16df27a` |
| 2 | `30da66849f556f19b43801ffac0e79511477d5601dc02fd3431674f3bc86433a` |
| 4 | `d0cfe2e33b845dafe7435080c73459ebe78df73d487813f7cf1aba880ac5a8f4` |
| 8 | `40cd003e232cc96ba4070ae33c2aa96f61da550244524790fc8ec66974f4f60e` |

### Thread sweep at 1536 × 960

| Case | Threads | Wall ms | CPU ms | Wall / 8 [range] | CPU / 8 [range] | Median run p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | --- | --- | ---: | ---: |
| full | 1 | 1.172320 | 1.160966 | 2.656822 [2.554983–2.794258] | 0.717952 [0.688856–0.756054] | 1.310092 | 1.639278 |
| full | 2 | 0.663304 | 1.210715 | 1.522058 [1.444385–1.598708] | 0.761980 [0.718043–0.771065] | 0.783448 | 1.069828 |
| full | 4 | 0.438941 | 1.332538 | 1.027825 [0.961507–1.030869] | 0.817468 [0.782561–0.847594] | 0.535049 | 0.838455 |
| full | 8 | 0.444018 | 1.608962 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 0.567599 | 0.841130 |
| noise | 1 | 1.222494 | 1.211047 | 2.394044 [2.322667–2.428257] | 0.654960 [0.629064–0.674584] | 1.342639 | 1.689463 |
| noise | 2 | 0.708806 | 1.279105 | 1.361975 [1.331266–1.471976] | 0.689874 [0.655917–0.705721] | 0.846294 | 1.230858 |
| noise | 4 | 0.516901 | 1.474982 | 0.986953 [0.960183–1.099280] | 0.803778 [0.741045–1.035485] | 0.630530 | 2.663505 |
| noise | 8 | 0.516736 | 1.847503 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 0.635904 | 1.225948 |
| bar | 1 | 1.630977 | 1.619377 | 1.753659 [1.735987–1.792133] | 0.783856 [0.776007–0.802384] | 1.928848 | 2.482888 |
| bar | 2 | 1.133122 | 1.695795 | 1.217731 [1.185856–1.232142] | 0.820842 [0.797500–0.833226] | 1.281060 | 1.703788 |
| bar | 4 | 0.925946 | 1.810654 | 1.002556 [0.974367–1.152280] | 0.888038 [0.867610–1.026836] | 1.097013 | 2.147290 |
| bar | 8 | 0.929165 | 2.052281 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 1.102588 | 1.410113 |
| latepixel | 1 | 0.546917 | 0.542710 | 1.061403 [0.839051–1.129420] | 0.956004 [0.766316–1.015343] | 0.973264 | 1.922053 |
| latepixel | 2 | 0.522731 | 0.513224 | 1.002568 [0.833341–1.167120] | 0.903999 [0.738230–1.010248] | 0.739717 | 1.649236 |
| latepixel | 4 | 0.561830 | 0.572544 | 1.030065 [0.801727–1.352701] | 0.923467 [0.773764–1.182427] | 0.751471 | 1.950197 |
| latepixel | 8 | 0.550814 | 0.613278 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 0.788827 | 1.367706 |
| static | 1 | 0.438189 | 0.435171 | 0.858229 [0.798514–1.037218] | 0.859007 [0.800071–1.035677] | 0.578209 | 1.120246 |
| static | 2 | 0.453675 | 0.450725 | 0.868647 [0.827241–1.034816] | 0.869378 [0.828068–1.035448] | 0.602201 | 1.188465 |
| static | 4 | 0.525435 | 0.521624 | 1.006043 [0.863290–1.064060] | 1.006129 [0.862437–1.063272] | 0.613776 | 1.285714 |
| static | 8 | 0.510324 | 0.507075 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 0.703458 | 1.310375 |

### Thread sweep at 2560 × 1600

| Case | Threads | Wall ms | CPU ms | Wall / 8 [range] | CPU / 8 [range] | Median run p95 ms | Maximum ms |
| --- | ---: | ---: | ---: | --- | --- | ---: | ---: |
| full | 1 | 3.381055 | 3.353901 | 1.966969 [1.386375–2.167788] | 0.436927 [0.323121–0.469192] | 3.650438 | 4.672223 |
| full | 2 | 1.952435 | 3.712728 | 1.096013 [0.779825–1.229626] | 0.468782 [0.347400–0.499709] | 2.573900 | 4.341035 |
| full | 4 | 1.625364 | 5.548742 | 0.881951 [0.703233–1.042230] | 0.651423 [0.561863–0.781606] | 1.972832 | 4.005574 |
| full | 8 | 1.709007 | 7.769272 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 2.121778 | 4.119545 |
| noise | 1 | 4.555892 | 4.518037 | 2.470181 [2.317801–2.902530] | 0.569834 [0.491356–0.657072] | 4.718197 | 6.322455 |
| noise | 2 | 2.580787 | 4.888961 | 1.373306 [1.266071–1.483365] | 0.580322 [0.486087–0.654927] | 2.855153 | 3.576342 |
| noise | 4 | 1.748256 | 5.955908 | 0.908213 [0.891817–0.958353] | 0.688865 [0.637617–0.763138] | 1.960402 | 2.692075 |
| noise | 8 | 1.857085 | 8.229330 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 2.122594 | 3.145291 |
| bar | 1 | 6.215581 | 6.163697 | 1.601824 [1.442434–1.915323] | 0.627480 [0.572838–0.730056] | 6.713276 | 10.678706 |
| bar | 2 | 4.584887 | 6.861658 | 1.156513 [1.027702–1.472124] | 0.683248 [0.618228–0.847151] | 5.179188 | 9.387729 |
| bar | 4 | 3.795402 | 8.001380 | 0.993285 [0.785549–1.041828] | 0.826275 [0.648338–0.852443] | 4.473622 | 6.023256 |
| bar | 8 | 3.881501 | 9.863470 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 4.442100 | 7.077055 |
| latepixel | 1 | 2.076519 | 2.045350 | 0.908780 [0.811026–1.094339] | 0.909110 [0.813070–1.092205] | 2.594784 | 4.326083 |
| latepixel | 2 | 2.099262 | 2.041552 | 0.912499 [0.861916–1.026241] | 0.901126 [0.849499–1.006063] | 2.424338 | 3.997510 |
| latepixel | 4 | 2.261778 | 2.165398 | 0.943338 [0.930722–1.030272] | 0.916495 [0.898641–1.002441] | 2.550398 | 4.301054 |
| latepixel | 8 | 2.300563 | 2.282692 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 2.905846 | 3.865109 |
| static | 1 | 2.063293 | 2.038719 | 1.016185 [0.976409–1.069349] | 1.019161 [0.976465–1.069111] | 2.386713 | 4.148922 |
| static | 2 | 2.042017 | 2.017536 | 1.002470 [0.995806–1.039826] | 1.001865 [0.995668–1.038578] | 2.310153 | 3.780908 |
| static | 4 | 2.016294 | 1.992213 | 1.003490 [0.946132–1.038718] | 1.003626 [0.946161–1.039947] | 2.246803 | 3.737737 |
| static | 8 | 2.009281 | 1.985014 | 1.000000 [1.000000–1.000000] | 1.000000 [1.000000–1.000000] | 2.356393 | 3.590392 |

One thread often used less CPU but had longer wall times. Two threads offered a candidate trade-off: the six full/noise/bar CPU ratios were approximately 0.762/0.690/0.821 at 1536 × 960 and 0.469/0.580/0.683 at 2560 × 1600. That justified a subsequent live test, not a universal change to the helper default. Four and eight threads remain fully reported; rejecting their selection as a default does not erase their lower wall times.

## Three-tablet integration and live A/B

The integration fixed distinct boundaries. Extra ADB sessions now reverse the fixed Android video/input listeners `8890`/`8891` to instance-specific host ports `8890 + 2i` / `8891 + 2i`; device and host port namespaces are not interchangeable. On Android, a fatal current decoder failure wakes the receiver by closing its current connection, allowing the receiver-owned tier-0-to-tier-1 retry path. Stale or stopped render threads must not close a replacement connection. The Samsung clients in this experiment used locally built release APKs containing that change; the Lenovo client used the official 1.2.13 APK. This is not a three-client test of the unchanged official APK.

X11 output ownership uses connector identity plus EDID when available, rather than identifying equal panels solely by equal EDID bytes. Numbered touch/pen names are isolated from the unnumbered first instance. A shared RandR event watcher remaps after display changes. The EDID detailed timing physical-size bytes were corrected in Rust and Python: requested 172 × 108 mm and 227 × 142 mm had decoded as 1708 × 3072 mm and 2275 × 3584 mm because the low/high-byte fields were ordered incorrectly. Cache generation three prevents reuse of old generated bytes. Slot-derived EDID serials distinguish simultaneous outputs but identify host session slots, **not permanent physical tablet identities**. Cinnamon's native per-device output triples are configured using actual Muffin monitor identities, while the existing XInput path remains available. A successful stored preference is not alone proof of the active touch transformation.

The earlier input failures remain part of the record: the pre-input-fix own-window counts were D1=0, D2=2, D3=0 despite correct video markers. A later quiet pre-mapping-fix trial recorded D2 events in another tablet's window and D2 own-window count zero. A separate repeat during user display rearrangement is inconclusive for a stable-topology product verdict; it is not relabeled a confirmed product defect. The EDID byte error is demonstrated, but these records do not identify a later CTM writer or prove that a specific Cinnamon action caused the original input misrouting.

After the coupled EDID/native-mapping changes, independently checked smoke evidence confirmed the expected physical monitor geometry/primary assignment, three correct touch matrices and each tablet's own video markers and press/release events. The following sequential A/B test used the same prepared scene and native capture modes: D1 at 1536 × 960, D2/D3 at 2560 × 1600, scale one and 60 fps targets. A used the original eight-cap helper source `40cd003e…`; B used the compile-time-two variant `30da6684…`. Both had CPUs 0–11 and nice 0. Each helper had nine OS threads in A and three in B, including its caller/conversion and writer threads.

This is **one matched A/B pair**, with no CI or statistical-significance claim. The scene is **low-entropy gradually changing solid color**, with static controls and one-active/all-active phases. It is not video or random-noise encoding. Each phase is approximately 12 seconds; recovery is approximately 15 seconds. CPU totals sum one host, three capture helpers and three encoders only: 100% means one logical core, and Xorg/Cinnamon/ADB/other processes are excluded. FPS comes from encoder frame-counter deltas within each phase, not displayed-frame timing. Reading CPU alone would hide the simultaneous-stream ceiling.

| Phase | A CPU %, one-core units | B CPU %, one-core units | B/A CPU | A encoder fps D1 / D2 / D3 | B encoder fps D1 / D2 / D3 |
| --- | ---: | ---: | ---: | --- | --- |
| static-all | 27.583 | 26.250 | 0.951677 | 4.949 / 4.949 / 5.044 | 4.949 / 5.045 / 4.949 |
| D1-full | 149.664 | 136.832 | 0.914259 | 59.788 / 4.950 / 4.881 | 60.032 / 5.046 / 4.951 |
| D2-full | 216.413 | 175.664 | 0.811706 | 4.951 / 57.787 / 5.046 | 4.951 / 58.840 / 4.951 |
| D3-full | 199.996 | 155.582 | 0.777927 | 5.046 / 4.951 / 61.267 | 4.951 / 5.127 / 59.599 |
| all-full | 269.996 | 212.579 | 0.787341 | 18.374 / 17.994 / 58.437 | 18.812 / 18.699 / 58.363 |
| static-recovery | 23.466 | 24.333 | 1.036934 | 4.960 / 4.959 / 4.866 | 5.034 / 5.033 / 4.960 |

In the all-active phase, the seven-process total was **269.996% versus 212.579% of one core**. The encoder rates were **18.374 / 17.994 / 58.437 fps** in A and **18.812 / 18.699 / 58.363 fps** in B. The lower CPU observation therefore did not establish 60 fps on all three devices or eliminate the existing concurrent pipeline ceiling. Single-active phases approached the 60 fps target for the active tablet, while inactive tablets remained near five keepalive fps.

Per-process GPU engine observations for the all-active phase are included below. Each percentage is relative to one i915 engine, not total GPU utilization or energy. All phase/role CPU and GPU observations, interval lengths, counter deltas and sample counts remain in the JSON; copy and video-enhance observations were zero in this record.

| Encoder | A render % | B render % | A video % | B video % |
| --- | ---: | ---: | ---: | ---: |
| D1 | 2.211249 | 2.286118 | 2.393659 | 1.778551 |
| D2 | 3.719005 | 3.856693 | 6.868308 | 6.992977 |
| D3 | 11.851670 | 11.783107 | 19.350572 | 20.230023 |

Both A and B recorded each tablet's own video sample and two own-window input events (press/release) for D1, D2 and D3. Video ownership uses the distinct markers, not a claim of bit-exact encoded colors: the recorded D2 center was RGB `[78,124,94]` for requested `[64,112,80]` in both runs. All requested/observed center samples remain in the JSON; the native converter's byte-equivalence fixtures are a separate check. The recorded topology was unchanged before/after each run; the test windows were destroyed. Actual touch CTMs matched their expected geometry after native mapping and after B. These bounded observations supersede the old blanket “three-tablet pending” status, while the later final-source and reconnect observations below have their own narrower scope; pen-only interaction remains untested.

### Retained initial-configuration thread hint and validation scope

The follow-up selects internal `--conversion-threads 2` **once, when creating the helper**, from the **initial configured** width at most 2560, height at most 1600, target at most 60 fps and scale one. The selected hint remains in force for that helper's lifetime. If the compositor subsequently negotiates a larger mode, the helper retains that hint: **there is no dynamically enforced mode-size boundary**. Larger initial configurations, scaled paths and faster targets keep the existing automatic limit, up to eight threads. The measured sizes justify the initial selection; no performance claim is made for a later larger negotiated mode.

The earlier B executable changed `MAX_CONV_THREADS` at compile time. The final source uses the runtime cap and is validated separately below. The same cap value does not make these the same source, binary or performance experiment. The recorded final run confirms the actual `--conversion-threads 2` arguments on all three helpers; it does not convert the previous A/B into a final-source speedup measurement.

## Portable converter benchmark

[host/evdi/evdi_conversion_bench.c](../host/evdi/evdi_conversion_bench.c) reuses the finite-sweep harness with a repository-relative include of `evdi_helper.c`. It uses the production runtime conversion cap instead of generating four source variants, and reports requested and actual counts so the existing online-CPU limit stays visible. It stubs only the EVDI boundary and opens no display device, FIFO, writer, encoder or GPU. Supported cases are `full`, `noise`, `bar`, `latepixel` and `static`, at either recorded native size. `measure` retains 24 warmup frames; `verify` has zero warmup so a small functional check remains small. The last stdout JSON record follows the helper's mode diagnostic.

Build and perform a five-frame byte check from the repository root, choosing an output filename for local artifacts:

Set `BENCH_OUTPUT` to a writable artifact filename before running these commands.

```sh
gcc -O3 -std=gnu11 -Wall -Wextra -pthread -ffunction-sections -fdata-sections \
  host/evdi/evdi_conversion_bench.c -Wl,--gc-sections -o "$BENCH_OUTPUT"
"$BENCH_OUTPUT" noise 1536 960 5 2 verify
```

Replace the final cap argument with 1, 2, 4 or 8 to select a cap. The included helper's existing `find_evdi_device` `snprintf` warning is outside the executed synthetic path; the build is not claimed warning-free. The local check for publishing this harness used one complete noise frame at each of the four caps with zero byte errors and an invalid-cap rejection, not a new timing series. Its source is newer than the historical sweep's `MAX_CONV_THREADS` snapshots, so future measurements must bind their current source, compiler and scheduling environment rather than claiming to replay the original binaries.

## Rechecking the additional numerical records

This second arithmetic check reads only the public data. It recalculates the finite sweep's paired CPU/wall ratios and the live encoder-counter rates. It does not launch the native harness or perform a new timing series.

```sh
python3 - <<'PY'
import json, math, statistics
from pathlib import Path

data = json.loads(Path('docs/evdi-performance-evidence.json').read_text())
sweep = data['thread_sweep']
runs = sweep['raw_measurements']
for group in sweep['summary']:
    selected = {cap: sorted((r for r in runs if
        (r['width'], r['height'], r['case'], r['threads']) ==
        (group['width'], group['height'], group['case'], cap)),
        key=lambda r: r['repeat']) for cap in (group['threads'], 8)}
    candidate, base = selected[group['threads']], selected[8]
    assert len(candidate) == len(base) == 5
    for field, metric in [('process_cpu_ms_frame', 'cpu'), ('wall_ms_frame', 'wall')]:
        ratios = [a[field] / b[field] for a, b in zip(candidate, base)]
        for statistic, actual in [('median', statistics.median(ratios)),
                                  ('min', min(ratios)), ('max', max(ratios))]:
            assert math.isclose(group[f'{statistic}_{metric}_ratio'], actual, rel_tol=1e-12)
assert (len(runs), len(sweep['raw_verification']), len(sweep['summary'])) == (200, 40, 40)
assert sum(r['compared_frames'] for r in sweep['raw_verification']) == 8160
assert sum(r['compared_bytes'] for r in sweep['raw_verification']) == 34091827200
for run in data['three_tablet_live_validation']['runs']:
    for phase in run['phases']:
        assert math.isclose(sum(phase['cpu_percent'].values()),
                            phase['cpu_total_one_core_percent'], rel_tol=1e-12)
        for cadence in phase['encoder_cadence'].values():
            assert math.isclose(cadence['fps'], cadence['counter_delta'] /
                                cadence['counter_interval_seconds'], rel_tol=1e-12)
print('PASS: 40 sweep summaries, 200 timed records, 40 byte checks, 12 live phases')
PY
```

## Final runtime-source validation observation

After the final reviewed host installation, a separate validation run recorded the actual runtime-cap source and all three native streams. This is **one separate observation, not a matched performance comparison to A or B**. No final-versus-A/B CPU ratio, CI or speedup is computed. The same low-entropy gradual solid-color scene and temporary separated outputs were used; the CPU total still covers only one host, three helpers and three encoders. The source identities differ from both earlier compile-time-cap variants.

| Recorded identity | SHA-256 |
| --- | --- |
| Helper source | `f3a4cbaffe543b11f11e5b6c7bb8801c48e1c9ffe19a54403267f13814e6aa79` |
| Installed helper executable | `1ec3b1047682021b9d1bfd454f0f74eb4afbad4e96514d61693ef1344a37e09a` |
| Installed reviewed host executable | `8e3b746994167cfbc8975bf18a829362f5d00d20df0c122e137d20c5251aecb6` |

All three recorded helper argument lists contain `--conversion-threads 2` and a 60 fps target. Six video samples were captured, and D1/D2/D3 each produced its own press/release pair (two own-window events per device). The topology was equal before/after the run and the test windows were destroyed. Recorded pixel-center samples remain explicit in the JSON; video ownership is not a bit-exact encoded-color claim.

| Final-source phase | Duration, seconds | Seven-process CPU %, one-core units | Encoder fps D1 / D2 / D3 |
| --- | ---: | ---: | --- |
| static-all | 12.000124 | 26.000 | 5.043 / 5.044 / 4.949 |
| D1-full | 12.000140 | 134.915 | 57.023 / 5.046 / 5.101 |
| D2-full | 12.000276 | 169.246 | 5.051 / 51.695 / 4.958 |
| D3-full | 12.000120 | 151.998 | 5.046 / 5.046 / 59.382 |
| all-full | 12.000089 | 192.415 | 18.280 / 17.803 / 55.862 |
| static-recovery | 15.000350 | 23.999 | 4.960 / 4.959 / 5.016 |

The all-active observation remained limited to approximately **18.280 / 17.803 / 55.862 encoder fps**, with 192.415% of one core across the seven identified processes. It does not establish concurrent 60 fps on all tablets. The different final-source figures are retained as observed; they are not divided by the earlier A/B figures to imply an additional gain. Full per-role CPU/GPU and frame-counter samples are in `final_runtime_observation` in the JSON and six `final_runtime_phase_observation` rows in the CSV.

### Reconnect observations and boundaries

Device-side `adb reconnect device`, followed by an application restart, completed successfully for D1, D2 and D3. The fixed device listeners `8890`/`8891` again mapped to each device's own host ports; the recorded reverse mappings were restored on all three. The subsequent final-source run above observed each device's own video and input. This is a tested software reconnect path, **not a physical cable-disconnection or reboot test**.

A host-side `adb reconnect` behaved differently: D3 disappeared from ADB discovery. The same disappearance was reproduced with both the UScreen daemon and tablet application stopped, while USB presence/authorization remained. This separates the observed failure from a running UScreen requirement; it does not identify the underlying actor or transport cause. A targeted USB add uevent restored discovery during diagnosis. No privileged recovery action was added to the application, and no general automatic recovery guarantee is claimed.

### Mode and layout scope

The recorded mode transition turned virtual outputs off, left three physical monitors present, cleared three native per-device output mapping keys, and turned virtual output mode back on. It **did not test pen-only taps, stylus behavior or the pen-only CTM**. Successful output/key transitions must not be described as a passed pen-only interaction test.

Independent frame/input and performance observations used a temporary separated output layout. The user's saved mirrored S5e arrangement was restored separately. An intermediate native API `verified` result was only a verification operation and did not prove persistence. The final receipt instead records activation of the fresh native **“Änderungen beibehalten”** confirmation, a persisted layout XML hash, and an **ordinary service restart without a manual xrandr restoration** that returned the exact saved output positions and physical primary assignment. Three fixed profiles and three actual touch CTMs were checked after that restart; the service was active and automatic session startup remained disabled.

A subsequent final-layout smoke recorded video samples and own-window press/release pairs for D1, D2 and D3, with unchanged topology and removal of both test windows. D2 and D3 intentionally shared the mirrored S5e output/window, so this final smoke **does not claim three distinct simultaneous pictures**. The earlier separated final-source run supplies the distinct-output observation. The normal-start receipt demonstrates one saved-layout service restart, not reboot persistence. No physical cable or reboot test was performed.

### Android APK signing scope

The Samsung clients used locally built release-mode APKs signed with a **local Android Debug certificate**, rather than the upstream release signing identity. Local APK signature verification passed the recorded v2/v3 checks, but that does not establish upstream signing provenance or in-place update compatibility. The Lenovo APK matched the official 1.2.13 release bytes. These results therefore do not establish three-tablet operation with unchanged official APKs on every client. Signing keys and certificate fingerprints are not published here.
