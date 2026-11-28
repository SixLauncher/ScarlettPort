---
name: run-scarlettport
description: Build ScarlettPort's scarlett.exe on Linux and run it under Proton. Use when asked to build, run, test or smoke-test ScarlettPort, run a scarlett.exe subcommand, or hand a local build to SixLauncher.
---

ScarlettPort's runtime only targets Windows (MSVC). On Linux it is cross-built with `cargo-xwin` and
run through Proton's `runinprefix`, all wrapped by `.claude/skills/run-scarlettport/sp.sh`.
Playing goes through SixLauncher (`../launcher`, see its `run-launcher` skill). All paths are
relative to `ScarlettPort/`. Verified on CachyOS with Proton Experimental (11.0) and an RTX 3050.

## Prerequisites

```bash
sudo pacman -S --needed rustup clang lld llvm   # clang/lld/llvm were already installed here
cargo install cargo-xwin --locked               # 0.24.2; it downloads the MSVC CRT + Windows SDK on first use
```

`engine/rust-toolchain.toml` pins 1.98.1 with the `x86_64-pc-windows-msvc` target; rustup installs
it on first use. A Proton is needed: Steam's `Proton - Experimental` is the default
(`SP_PROTON=/path/to/proton-folder` for another one).

## Build

```bash
S=.claude/skills/run-scarlettport/sp.sh
$S build    # ~35 s warm -> engine/target/x86_64-pc-windows-msvc/release/scarlett.exe (16 MB, PE32+)
```

## Run (agent path)

```bash
$S smoke                     # --help, `fingerprint`, `inspect` of scarlett.exe itself: no game needed
$S exe config                # any subcommand, under Proton (throwaway prefix /tmp/scarlett-smoke-compat)
$S exe inspect "$($S winpath engine/target/x86_64-pc-windows-msvc/release/scarlett.exe)"
```

Arguments are passed to `scarlett.exe` as-is: convert Linux paths with `$S winpath <path>` (`Z:\...`).
To run a title, hand the build to SixLauncher (`../launcher`: `driver.sh cli setup --backend scarlett
--backend-exe <this exe>`), which adds the preset's `SCARLETT_*` environment, the GPU variables and the
data/cache dirs; `sixlauncher run --dry-run` prints that full command.

## Test

```bash
$S test                      # core, formats, pe, loader, pipeline: 157 passed under Proton
$S test scarlett-shader      # 287 passed / 1 failed (emit::tests::stub_pipeline_is_created), ~5 min
$S test scarlett-gpu -- compose:: --nocapture   # the composer on the real GPU; prints the readback RGBA
```

GPU tests run on the real card through vkd3d-proton (RTX 3050 here). A test that finds no device
prints `skipping ...: no D3D12 device` and still passes: check for the readback line, not the count.

## Gotchas

- **Tests don't build for the Linux host.** `cargo test` (the BUILDING.md command) fails with
  `cannot find module or crate windows_sys`, even for the "GPU-free" crates: `sp.sh test`
  cross-builds them with `--no-run` and runs each test `.exe` under Proton.
- **`proton run` prints nothing.** It goes through `steam.exe` and drops the console output (exit 0).
  `runinprefix` keeps stdout; it is also what SixLauncher uses.
- **Proton's `files/bin/wine` alone exits 53**: `libvkd3d-*.dll`/`wined3d.dll` not found, because the
  `proton` script is what sets up the DLL paths. Always go through `proton`.
- **Don't pipe `scarlett.exe` into `head`.** It panics with `failed printing to stdout ... (os error
  233)` when the pipe closes; capture the output first.
- **`proton runinprefix` loads Wine's own `d3d12.dll`** (upstream vkd3d: `Feature level 0xc000 is
  not supported`, so no D3D12 device at all): only the `run` verbs set Proton's DLL overrides.
  `sp.sh` sets `WINEDLLOVERRIDES=d3d12,d3d12core,dxgi=n,b` (vkd3d-proton + DXVK's dxgi, which
  Proton copies into the prefix), and creates a new prefix with `proton run` once so those DLLs are
  there. SixLauncher does the same for its ScarlettPort runs. Without it, 35 `scarlett-shader` tests
  fail with `no SM 6.6 device available` and GPU tests skip silently.
- **WARP doesn't exist under Proton** ("Não implementado"): tests that only run on WARP are skipped.
- **`SCARLETT_TEST_NO_GPU` does nothing**: BUILDING.md mentions it, no code reads it.
- **No Agility SDK folder (`D3D12\`)** is placed next to the exe: under Proton, D3D12 is vkd3d-proton.
  This has only been checked for the commands above, not for a game run.
- **Warnings are expected**: 6 in `scarlett-gpu` (`drop` of a `Copy` value etc.); the build succeeds.
