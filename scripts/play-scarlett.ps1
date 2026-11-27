# Launches a title on the ScarlettPort-based runtime from your own dump (you need your own legally
# obtained copy; nothing is included in this repository).
# Usage: .\scripts\play-scarlett.ps1 -Game "D:\path\to\dump" [-AutoInput] [-Console series-s|series-x]
#          [-NoOwnUpscaler] [-KeepWatchdog] [-FastBoot] [-RayTracing] [-SaveSlot name] [-Precompile] [-SkipPrecompile]
#
# Environment it sets (you can also set these by hand before running the exe):
#   SCARLETT_GAME_DIR       the dump folder
#   SCARLETT_OWN_UPSCALER=1 ask for the title's own upscaler on the Series S profile (recommended)
#   SCARLETT_NO_WATCHDOG=1  disarm the title's stall watchdog so it can wait for slow first-run
#                          shader builds instead of ending itself
#   SCARLETT_RELAY=2        submission relay, form 2
#   SCARLETT_PIPELINE_WAIT_MS=0  do not hold a draw back waiting for a pipeline build
#   The engine's CAND4 group (THIN, CARD_BUFFERS_MIN_KB, CARD_STEPS, ...), as SixLauncher sets it.
#   SCARLETT_FAST_BOOT=1    with -FastBoot: press through logos and menus (testing only)
#   SCARLETT_BVH_BUILDER=1  with -RayTracing: the title's ray-traced indirect light, for that launch only
#   SCARLETT_SAVE_SLOT      with -SaveSlot: pick a save slot by name
#   SCARLETT_AUTO_INPUT=1   with -AutoInput: press A on a timer so menus advance without a pad
#   SCARLETT_PRESS_FILE     press.txt in the repo root: write button names ("A", "DOWN DOWN A") to press them
param(
    [Parameter(Mandatory)] [string]$Game,
    # The console profile the title sees. series-s is the recommended default.
    [ValidateSet('series-s', 'series-x')] [string]$Console = 'series-s',
    # Press A on a timer so the title screens advance without a controller.
    [switch]$AutoInput,
    # Opt-in picture and shader translation fixes (SCARLETT_LOOK, SCARLETT_SHADER_FIXES); not yet measured on the RTX 3060.
    [switch]$Look,
    # Do not set SCARLETT_OWN_UPSCALER=1.
    [switch]$NoOwnUpscaler,
    # Do not set SCARLETT_NO_WATCHDOG=1.
    [switch]$KeepWatchdog,
    # Testing only: SCARLETT_FAST_BOOT=1.
    [switch]$FastBoot,
    # Run the title's ray-traced lighting passes with its software BVH traversal.
    [switch]$RayTracing,
    # Save slot name (SCARLETT_SAVE_SLOT).
    [string]$SaveSlot = '',
    # Run `scarlett shaders` and `scarlett warm` even if they already ran once.
    [switch]$Precompile,
    # Do not run `scarlett shaders` / `scarlett warm` (shaders are then translated while you play).
    [switch]$SkipPrecompile
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$vic  = Join-Path $root 'engine\target\release\scarlett.exe'


if (-not (Test-Path $vic)) { throw 'Not built yet. Run .\scripts\setup-scarlett.ps1 first.' }
$exe = Join-Path $Game 'game_scarlett_master.exe'
if (-not (Test-Path $exe)) {
    throw "No game_scarlett_master.exe in '$Game'. Point -Game at your own dump."
}

$env:SCARLETT_GAME_DIR = $Game
if (-not $NoOwnUpscaler) { $env:SCARLETT_OWN_UPSCALER = '1' }
if (-not $KeepWatchdog) { $env:SCARLETT_NO_WATCHDOG = '1' }
$env:SCARLETT_RELAY = '2'
$env:SCARLETT_PIPELINE_WAIT_MS = '0'
# SCARLETT_CARD_BUFFERS_MB is deliberately not set: the engine scales this ceiling from VRAM
# (512 MiB on a <=12 GiB budget) and reserves at most 1 GiB of texture room for card buffers,
# so overriding only the ceiling (the old 4096) leaves the two out of step.

# The engine's settings, identical to SixLauncher's Stable preset (CAND4 in its
# crates/core/src/scarlett.rs) and to engine/Play-ScarlettPort.cmd. Keep the three lists in step.
$live = [ordered]@{
    'SCARLETT_AMBIENT_AIR'          = '1.5'
    'SCARLETT_TEXTURE_NO_WAIT'      = '1'
    'SCARLETT_WIDE_BACK'            = '1'
    'SCARLETT_CARD_REHASH_RUNS'     = '1'
    'SCARLETT_JOIN_COPIES'          = '1'
    'SCARLETT_INDIRECT_WINDOW'      = '1'
    'SCARLETT_RELAY_COPY'           = '1'
    'SCARLETT_CARD_STEPS'           = '2'
    'SCARLETT_INDIRECT_BATCH'       = '2'
    'SCARLETT_MAP_THREADS'          = '1'
    'SCARLETT_CARD_AWAKE'           = '1'
    'SCARLETT_SHOW_AFTER'           = '1'
    'SCARLETT_AMBIENT'              = '0.8'
    'SCARLETT_THIN'                 = '1'
    'SCARLETT_JOIN_PARTS'           = '8'
    'SCARLETT_UPLOAD_AHEAD'         = '1'
    'SCARLETT_LEVELS_ON_DEMAND'     = '1'
    'SCARLETT_CARD_BUFFERS_MIN_KB'  = '64'
}
foreach ($k in $live.Keys) { Set-Item "env:$k" $live[$k] }
if ($FastBoot) { $env:SCARLETT_FAST_BOOT = '1' }
if ($Look) { $env:SCARLETT_LOOK = '1'; $env:SCARLETT_SHADER_FIXES = '1' }
if ($SaveSlot) { $env:SCARLETT_SAVE_SLOT = $SaveSlot }
# Write button names here ("A", "DOWN DOWN A") to press them while the game runs.
$env:SCARLETT_PRESS_FILE = Join-Path $root 'press.txt'
Set-Content -Path $env:SCARLETT_PRESS_FILE -Value '' -NoNewline
if ($AutoInput) { $env:SCARLETT_AUTO_INPUT = '1' }

# Translate every shader into the cache (`scarlett shaders`, a few minutes), then make the host
# pipelines earlier runs recorded (`scarlett warm`; none on a first run). Done once, marked by a
# stamp file; -Precompile runs both again (after an engine or driver update).
$stamp = Join-Path $root 'logs\shaders.done'
if (-not $SkipPrecompile -and ($Precompile -or -not (Test-Path $stamp))) {
    Write-Host 'Translating the title''s shaders (once, a few minutes)...' -ForegroundColor Cyan
    $ErrorActionPreference = 'Continue'
    & $vic shaders $Game
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw "scarlett shaders failed (exit $code)." }
    Write-Host 'Making the pipelines earlier runs recorded...' -ForegroundColor Cyan
    $ErrorActionPreference = 'Continue'
    & $vic warm
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    if ($code -ne 0) { throw "scarlett warm failed (exit $code)." }
    New-Item -ItemType Directory -Force (Split-Path $stamp) | Out-Null
    Set-Content -Path $stamp -Value (Get-Date -Format s)
}

$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force $logs | Out-Null
$log = Join-Path $logs ("scarlett-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
Write-Host "Starting on the $Console profile... log: $log" -ForegroundColor Cyan
# Windows PowerShell 5.1 turns a native program's stderr into a terminating error under 'Stop'
# when it is redirected, which would kill the run on the first log line.
$ErrorActionPreference = 'Continue'
# For this launch only: PowerShell's environment belongs to the whole session, so a plain launch
# after one with -RayTracing in the same window would still run with ray tracing. A value set by
# hand before running the script is kept when the switch is left out.
$rayTracingBefore = $env:SCARLETT_BVH_BUILDER
if ($RayTracing) { $env:SCARLETT_BVH_BUILDER = '1' }
try {
    & $vic run $exe --execute --console $Console *> $log
} finally {
    $env:SCARLETT_BVH_BUILDER = $rayTracingBefore
}
exit $LASTEXITCODE
