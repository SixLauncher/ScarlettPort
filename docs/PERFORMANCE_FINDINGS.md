# Performance findings after the 0.4.7 rename

This is a **static audit** of the current tree (rebased onto `213abe1`). No game dump, GPU or
profiler run was available to the author, so nothing here is a claimed fps measurement.
Every item says what the code does, why it can cost frame time, and how to confirm it on the
reference RTX 3060. Names and line numbers below are the current ones: `engine/crates/scarlett-*`,
`engine/Play-ScarlettPort.cmd`, `scripts/play-scarlett.ps1`, and `SCARLETT_*` switches.

The engine reads `SCARLETT_*`; `VICTORY_*` is only a silent fallback
(`engine/crates/scarlett-core/src/env.rs:13-26`). `alias_scarlett()` still exists
(`env.rs:33-47`) but now copies *legacy* `VICTORY_*` values into unset `SCARLETT_*` variables,
the reverse of the old helper — it is called once at startup from
`engine/apps/scarlett/src/main.rs:186`.

## Status after commit 1f99d77

| # | Finding | Status | Confidence |
|---|---|---|---|
| 1 | Manual launch path applied a dead environment and omitted CAND4 | **Fixed** in `1f99d77` (renamed `scripts/play-scarlett.ps1` sets CAND4, dead names removed) | High |
| 2 | `SCARLETT_CARD_BUFFERS_MB=4096` against a 1 GiB texture reserve | **Fixed** in `1f99d77` (override dropped) | Medium-high |
| 3 | Preset switches that became no-ops | **Partly fixed** — `-RayTracing` now sets `SCARLETT_BVH_BUILDER`; Look / shader-fixes / friendly / raytracing switches still dead | High |
| 4 | Published frame breakdown is from the older picture | Still true; `SCARLETT_GPU_PROFILE` now exists, so re-measure with it | High |
| 5 | `SCARLETT_RELAY_COPY=1` on, torn-piece checker off | Open | High (config gap) |
| 6 | `SCARLETT_FILL_TITLE=1` off | Open, needs 0.4.7 re-measure | Medium |
| 7 | Linear write-watch interval scans; `.nth(n)` in descriptor workers | Open | Medium |
| 8 | Stale task-board pointers and experiment names | Open | High |

## 1-2. What was fixed, and why it mattered

`launcher/src/presets.rs (now SixLauncher's crates/core/src/scarlett.rs):37-56` defines `CAND4`, the 18 live engine settings;
`launcher/src/presets.rs:105-120` (`stable_env`) and `engine/Play-ScarlettPort.cmd:18-38`
apply them. Before `1f99d77`, `play-scarlett.ps1` applied ~19 names that no 0.4.6+ engine
reads (`SCARLETT_PRECOMPILED_GRAPHICS`, `SCARLETT_READFIX`, `SCARLETT_SYNC_PRECISE`,
`SCARLETT_NARROW_*`, `SCARLETT_PREP_*`, `SCARLETT_MEM_SUBPAGE`, `SCARLETT_COPY_NOQUERY`,
`SCARLETT_FRIENDLY`, `SCARLETT_OWN_UPSCALER`, `SCARLETT_NO_WATCHDOG`, … ) and omitted all of
CAND4, including `SCARLETT_THIN` (`list/thin.rs:58`) and
`SCARLETT_CARD_BUFFERS_MIN_KB` (`shadow.rs:227-236`). The published RTX 3060 numbers
(`docs/STATUS.md:30`) were taken with that environment, so they did not describe the same
engine configuration as the launcher.

The script also set `SCARLETT_CARD_BUFFERS_MB='4096'`. That value is the **maximum total**
bytes of card-side title-buffer copies (`shadow.rs:217-247,425`); the engine's own default is
`(16, 512)` MiB on a <=12 GiB budget (`shadow.rs:179-193`). The texture queue reserves room
for those copies in `card_room_kept()` (`shadow.rs:296-307`, consumed at `resource.rs:1700,2041`),
which clamps the reservation:

```rust
const SEEN: u64 = 1 << 30;
card_limits().1.min(SEEN).saturating_sub(CARD_BYTES[0].load(...) + CARD_REFUSED[1].load(...))
```

The old script thus allowed a 4 GiB ceiling while reserving at most 1 GiB. `engine/docs/findings.md`
records texture refusals and a black ground when reservation and ceiling drift apart. Both
issues are resolved in `scripts/play-scarlett.ps1:55-84`, which now comments the pairing
explicitly.

**Verify the fix:** run once from the launcher and once from `play-scarlett.ps1` with the same
warm cache and save; the `card buffer limits` line (`shadow.rs:244`) and fps line should agree.

## 3. Preset switches that are still no-ops

- `SCARLETT_LOOK` / `SCARLETT_SHADER_FIXES` (`launcher/src/presets.rs:12-15`, `play-scarlett.ps1:86`) have no reader in `engine/crates/**`. The 0.4.7 engine has ambient and sky light on by default (`SCARLETT_AMBIENT_AIR=1.5`, `SCARLETT_AMBIENT=0.8` in CAND4), and there is no `SCARLETT_SHADER_FIXES` translator path. The Look preset changes nothing and should be retired before anyone uses it for a picture/fps A/B (`README.md:347`).
- `SCARLETT_RAYTRACING` (`launcher/src/engine.rs:138`) is still unread. **The toggle itself is fixed:** `-RayTracing` in `play-scarlett.ps1:124-129` and the launcher's Ray tracing setting (`launcher/src/tuning.rs:128`, `pages.rs:223`) now set `SCARLETT_BVH_BUILDER=1`, which the engine gates ray shaders on (`pipeline/make.rs:59-84` uses `SCARLETT_BVH_BUILDER`, `SCARLETT_RT_RESOLVE`, `SCARLETT_ACTIVATE_RAYS`). A run with the toggle on is no longer identical to one with it off; only the old environment variable is dead.
- `SCARLETT_FRIENDLY` is gone with `scarlett-core/src/friendly.rs`; the task board entry at `README.md:219,341,351` has no implementation.

## 4. The published breakdown is from the older picture

`README.md:135,174,336` and `docs/ROADMAP.md:13,15` publish a 187 ms split (55 ms preparing,
31 ms refresh copies, 76 ms card wait, 19 ms title reads) and 93-111 ms of GPU time over
~1,100 pipelines. `README.md:336` now states explicitly that these numbers are from the older,
darker picture; the full-picture engine is slower again. They should not be used to rank work
on the current engine.

The profiler has since been added: `SCARLETT_GPU_PROFILE=1` (`scarlett-gpu/src/gpuprof.rs:1-48`,
`frametl.rs`) reports card time by pipeline, and `scarlett-gpu/src/capture.rs` can log one line
per present (`SCARLETT_CAPTURE_LOG=1`). A separate switch, `SCARLETT_GPU_TIMING`
(`host.rs:1748,2382`), still forces `late_stage()` to 0, disabling pipelining while it times
individual lists. Do not confuse the two when measuring the default relay path. In-session
A/B is `SCARLETT_AB` (`ab.rs:1-7`), diagnostics build only.

## 5. Code-level hot-path candidates

None of these has been measured here. Switch names are the current `SCARLETT_*` ones.

- **`SCARLETT_RELAY_COPY` without `SCARLETT_FETCH_CHECK` (confidence: high for the gap).** Both launch paths set `SCARLETT_RELAY_COPY=1` (`presets.rs:44`, `play-scarlett.ps1:71`), whose comment records "48 ms of a 120 ms frame" waiting for seven fetches on the drawing queue (`host.rs:2487-2505`). The checker that makes a fetch beside later submissions trustworthy is `SCARLETT_FETCH_CHECK=1` (`host/torn.rs:2,30-35`), off by default; `host.rs:4690` only builds it when both the switch and a copy queue are present. It costs "3-4 ms for 96 MB beside the merge". Add `SCARLETT_FETCH_CHECK=1` to the verified preset, or document why it is omitted — otherwise a picture A/B can be measuring torn constants instead of the renderer. Verify with the suspect-piece count in the frame line (`host.rs:3651`).

- **`SCARLETT_FILL_TITLE` (confidence: medium).** `fill_is_titles()` (`host.rs:2787-2811`) is off by default. Its comment says one 480-byte fill of the title's constants ring a frame waited for every flight's copy-back, "some 30 ms of a 130 ms frame (38 of the 40 waits named)". `SCARLETT_CARD_STEPS=2` may already remove many of those waits. A/B it with two full runs (`SCARLETT_AB` is not wired to this `OnceLock` switch); check for stale constants in new areas (`host.rs:2803-2805`).

- **Write-watch interval lookup (confidence: medium).** `Journal.ahead` is a `Vec<(usize, usize)>` (`watch.rs:131`) and `asked`/`writes`/`nothing_new` test containment with `j.ahead.iter().any(...)` (`watch.rs:647-665,740-757`). These are called once per host-copy refresh, so the scan repeats for every refresh against a list that grows per submission. A merged, sorted interval list with `partition_point` would make it logarithmic, but must preserve "already asked" semantics exactly. Verify with the "asking the write watch" ms in the frame line.

- **Descriptor worker partitioning (confidence: medium).** `before.chunks(SYNC_AT_A_TIME).nth(n)`, `parts.chunks(SYNC_PAGES).nth(n)` and `make.chunks(SYNC_AT_A_TIME).nth(n)` run *inside* the worker closures (`descriptor.rs:328,364,455`), so worker `n` discards `n` chunks before working. Precomputing the chunk slices once and indexing them removes the repeated walk with no semantic change. Likely sub-millisecond, but it is on the preparing path (`descriptor.rs:353-356`).

- **Texture staging span (confidence: low-medium).** `SCARLETT_TEXTURE_STAGE_MS` defaults to 250 ms and catch-up triples it (`draw.rs:2624-2637`). With a streaming backlog one pass can occupy 250-750 ms of a frame on a recording thread — a plausible contributor to the "world-entry copy stall" (`README.md:340`). A/B 16/32 ms and `SCARLETT_TEXTURE_CATCHUP=0`, watching the texture-queue line; surfaces may stay black longer.

- **Thread wakeups (confidence: low-medium).** `SCARLETT_WIDE_SPIN` defaults to 0; `shadow.rs:1474-1479` says a helper thread is woken "95 times a frame". A/B a small spin value and watch total CPU time, not only wall time.

## 6. Ranked experiment plan (needs the reference machine)

Same dump, warm cache, same save and route; ≥3 repeats; median and p95 from `frames.csv`.
Change one variable at a time. Do not use `SCARLETT_GPU_TIMING` while comparing frame times.

1. **Baseline** — launcher Stable vs. fixed `scripts/play-scarlett.ps1`; they must agree.
2. **Fetch-check pairing** — `SCARLETT_FETCH_CHECK` 0 vs. 1 with `SCARLETT_RELAY_COPY=1` already on; inspect suspect pieces and the picture before trusting other A/Bs.
3. **Constant-ring waits** — `SCARLETT_FILL_TITLE` 0 vs. 1, with and without `SCARLETT_CARD_STEPS=2`.
4. **Card-buffer ceiling** — `SCARLETT_CARD_BUFFERS_MB` unset vs. 512/1024 on the 12 GB card; watch refused textures. The 4096 override is gone from the script, so only test it if you deliberately re-add it.
5. **Buffer placement** — `SCARLETT_CARD_BUFFERS_MIN_KB` 16 vs. 64 (CAND4) vs. 0.
6. **Threaded recording** — `SCARLETT_THIN` 1 vs. 0; `SCARLETT_THIN_THREADS` 4/8/16 (`list/thin.rs:303`).
7. **Copy switches that exist** — `SCARLETT_SAFE_COPY` is default-on (`safecopy.rs:12,325`); A/B 0 vs. 1. The other names in `presets.rs:76-83` (`SKIP_SAME`, `WRITE_BESIDE`, `FILL_FREE`, `COPY_FETCH`, `QUEUE2`) are absent from `engine/crates` — remove them from the task board instead of spending runs on them.
8. **Card steps and indirect** — `SCARLETT_CARD_STEPS` 0/1/2 (`host/card_steps.rs:79`), `SCARLETT_INDIRECT_WINDOW` (`draw.rs:341`). Both on in CAND4; isolate on NVIDIA.
9. **Wave64 on NVIDIA** — last; `README.md:336` notes 64-lane programs run as two 32-lane halves. This is a translator/cache-key change, so budget a full shader rebuild.

Do not re-test the ideas already recorded as no-gain or harmful in `engine/docs/findings.md:68-81`
(shader optimiser mask on that card, pipelining switch, lock split, thin LTO, removing the
settle, mapping pages on 16 threads, wave/fma/reciprocal tweaks) unless the renamed engine
reintroduces the same path.

### Stale pointers that still cost investigation time

- `README.md:338,340` points to `scarlett-gpu/src/zerocopy.rs` and `memcopy.rs`; neither file exists. The live equivalents are `shadow.rs`, `safecopy.rs`, `host.rs` and `host/place.rs`. There is no `SCARLETT_ZERO_COPY` switch to tune.
- `shadow.rs:255` still says `SCARLETT_CARD_ROOM_FOR_BUFFERS` is "off by default", but `card_room_kept_on()` at `shadow.rs:282-285` is on unless set to `0`. An A/B planned from the comment would start from the wrong assumption.
- `docs/RUNTIMES.md:47,95` still describes `SCARLETT_PRECOMPILED_GRAPHICS` and `scarlett precompile-graphics`; the current script runs `scarlett shaders` and `scarlett warm` (`play-scarlett.ps1:93-112`).

## Limitations

- Static analysis only; no build, dump or GPU in the environment. "No reader" means not present in `engine/crates/**/*.rs`.
- Expected improvements are hypotheses ranked by code evidence and prior measurements on different hardware, not measured gains on the RTX 3060. No in-game verification was performed for this document.
