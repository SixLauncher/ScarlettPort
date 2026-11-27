# Building

Windows 10 22H2 or Windows 11 x64 only. Neither runtime builds on macOS or Linux.

## Prerequisites

- Rust via [rustup](https://rustup.rs), MSVC target (`stable-x86_64-pc-windows-msvc`). `engine` pins its own toolchain in `rust-toolchain.toml`; rustup installs it on first use.
- Visual Studio Build Tools with the "Desktop development with C++" workload and a Windows 10/11 SDK.
- PowerShell 5.1 or later for `scripts/*.ps1`.
- A GPU and driver with Shader Model 6.6 and mesh shader support to run (not needed to build).

Commands below are run from the repository root.

## XCL

```powershell
cd runtime\xcl
cargo build --release -p xcl --locked
cargo test --release -p xcl --lib --locked -- --test-threads=4
```

GPU tests print `SKIPPED <test>: <reason>` and pass when the adapter cannot run them. Or use `.\scripts\setup.ps1`, which also fetches the Agility SDK. More notes: `runtime/xcl/AGENTS.md`.

## ScarlettPort runtime

```powershell
cd engine
cargo rustc --release -p scarlett --bin scarlett --features diagnostics --locked -- -C link-arg=/EXPORT:D3D12SDKVersion,DATA -C link-arg=/EXPORT:D3D12SDKPath,DATA
```

The two `/EXPORT` flags are required so Windows loads the Agility SDK placed next to the executable. `.\scripts\setup-scarlett.ps1` runs this command and does the Agility SDK step.

The source in `engine` is the ScarlettPort runtime; its fork point is in `engine/UPSTREAM.md` and the credits in [CREDITS.md](../CREDITS.md). The executable is `scarlett.exe`. Run it with `.\scripts\play-scarlett.ps1 -Game <your dump>` (Series S profile by default); the environment it sets is described in [RUNTIMES.md](RUNTIMES.md).

Tests that do not need a GPU: `cargo test --release --locked --lib -p scarlett-core -p scarlett-formats -p scarlett-pe -p scarlett-loader -p scarlett-shader -p scarlett-pipeline`. Tests in `scarlett-gpu` and `scarlett-hle` need a Shader Model 6.6 device. With `SCARLETT_TEST_NO_GPU=1` they run as on a machine without a card (device tests are skipped).

## Agility SDK

Windows 10 needs the D3D12 Agility SDK (1.614.1) next to the executable, in a `D3D12` folder containing `D3D12Core.dll` and `d3d12SDKLayers.dll`. `setup-scarlett.ps1` downloads it from NuGet (`Microsoft.Direct3D.D3D12`) into `engine\target\release\D3D12`. To do it by hand, download the package, unzip it, and copy both DLLs from `build\native\bin\x64`. Do not commit these DLLs.

## Playing a build

ScarlettPort has no launcher of its own any more: [SixLauncher](https://github.com/SixLauncher/launcher) runs `scarlett.exe` (choose the ScarlettPort backend and, under Custom, `engine\target\release\scarlett.exe` with its `D3D12` folder beside it). `scripts\play-scarlett.ps1` still runs a build directly.

Releases are built by `.github/workflows/release.yml` on a `v*` tag: the ScarlettPort runtime in release mode, the Agility SDK from NuGet, a ZIP with `scarlett.exe`, `D3D12\` and `SCARLETT_COMMIT.txt`, and SHA-256 checksums. No binaries are committed.

## Where things live

| What | Where |
|---|---|
| Build output | `runtime\xcl\target`, `engine\target` |
| Runtime data and cache (no switches set) | `%LOCALAPPDATA%\ScarlettPort\data` and `\cache`; SixLauncher sets `SCARLETT_DATA_DIR` and `SCARLETT_CACHE_DIR` to its own folders |
| Shader caches | `<cache>\scarlett` (World), `<cache>\dxil` (Classic); build progress in `<cache>\build-world.json` / `build-classic.json` |
| XCL title data, pipeline journal | `%XCL_DATA_ROOT%` or `%LOCALAPPDATA%\XCL`, then the package name; `cache\pipelines.bin` inside it |

Caches are safe to delete; they are rebuilt on the next run (slowly: first-run shader compiles stall the world).
