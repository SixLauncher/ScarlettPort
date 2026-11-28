#!/usr/bin/env bash
# Build ScarlettPort's Windows runtime on Linux (cargo-xwin) and run it under Proton.
#
#   sp.sh build                 scarlett.exe (release, diagnostics, Agility SDK exports)
#   sp.sh exe <args...>         run scarlett.exe <args> under Proton (Linux paths in args are NOT
#                               translated: pass Windows paths, see `winpath`)
#   sp.sh winpath <path>        Linux path -> Z:\... path scarlett.exe understands
#   sp.sh smoke                 --help, fingerprint and `inspect` of scarlett.exe itself (no game)
#   sp.sh test [crate...] [-- filter/libtest args]
#                               cross-build unit tests and run them under Proton (default: the
#                               GPU-free crates; scarlett-gpu gets the diagnostics feature)
#
# Env: SP_PROTON (Proton folder, default ~/.steam/root/steamapps/common/Proton - Experimental),
#      SP_COMPAT (throwaway prefix, default /tmp/scarlett-smoke-compat),
#      SP_WITH_GAME=1 (build/test even while a game runs), CARGO_BUILD_JOBS (default 8).
set -euo pipefail

# Memory. `build` and `test` (release rustc x 24 jobs, then D3D12 devices under Proton) next to
# a running game (~9 GB in its scope plus 3-4 GB of NVIDIA driver memory no cgroup counts) filled
# the 16 GB of RAM and the zram swap twice on 2026-11-28: systemd-oomd then killed the whole
# desktop session, where the uncapped build ran. So: not while a game runs, fewer jobs, and in a
# capped scope of their own, where past the ceiling only the build is killed.
if [[ -z ${SP_SCOPED:-} && ( ${1:-} == build || ${1:-} == test ) ]]; then
  if [[ -z ${SP_WITH_GAME:-} ]] && { systemctl --user list-units 'sixlauncher-game-*' --state=running --plain --no-legend 2>/dev/null | grep -q . || pgrep -f 'scarlett\.exe run' >/dev/null; }; then
    echo "sp.sh: a game is running: close it before building or testing (or SP_WITH_GAME=1)" >&2
    exit 3
  fi
  ram=$(awk '/^MemTotal:/{print $2 * 1024}' /proc/meminfo)
  if systemd-run --user --scope --quiet --collect true 2>/dev/null; then
    exec env SP_SCOPED=1 systemd-run --user --scope --quiet --collect --unit="sp-$1-$$" \
      -p MemoryHigh=$((ram * 45 / 100)) -p MemoryMax=$((ram / 2)) -p MemorySwapMax=$((ram / 8)) "$0" "$@"
  fi
fi
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8}

UNIT=$(cd "$(dirname "$0")/../../.." && pwd)
ENGINE=$UNIT/engine
TARGET=x86_64-pc-windows-msvc
EXE=$ENGINE/target/$TARGET/release/scarlett.exe
PROTON=${SP_PROTON:-$HOME/.steam/root/steamapps/common/Proton - Experimental}
export STEAM_COMPAT_DATA_PATH=${SP_COMPAT:-/tmp/scarlett-smoke-compat}
export STEAM_COMPAT_CLIENT_INSTALL_PATH=$HOME/.steam/root
export PATH=$HOME/.cargo/bin:$PATH
# scarlett-shader is left out: 35 of its tests need a D3D12 SM 6.6 device, and under Proton WARP
# is "not implemented" and the hardware device fails in a fresh prefix. `sp.sh test scarlett-shader`
# still runs it (expect 253 passed / 35 failed).
NOGPU_CRATES=(scarlett-core scarlett-formats scarlett-pe scarlett-loader scarlett-pipeline)

# `runinprefix`, not `run`: `run` goes through steam.exe and swallows the console output.
# Plain files/bin/wine fails too (exit 53): it lacks Proton's vkd3d DLL paths.
# `runinprefix` also skips Proton's DLL overrides: without these Wine's own d3d12.dll (upstream
# vkd3d, "Feature level 0xc000 is not supported") loads and there is no D3D12 device at all.
# The same overrides SixLauncher sets for its runinprefix runs.
export WINEDLLOVERRIDES=${WINEDLLOVERRIDES:-d3d12,d3d12core,dxgi=n,b}
proton() {
  mkdir -p "$STEAM_COMPAT_DATA_PATH"
  # A fresh prefix only gets vkd3d-proton/DXVK copied in by a `run` verb: create it that way once.
  [[ -f $STEAM_COMPAT_DATA_PATH/pfx/system.reg ]] || "$PROTON/proton" run /bin/true >/dev/null 2>&1 || true
  "$PROTON/proton" runinprefix "$@" 2> >(grep -v '^ntsync' >&2)
}
winpath() { printf 'Z:%s' "$(realpath "$1" | tr / '\\')"; }

case ${1:-} in
  build)
    cd "$ENGINE"
    cargo xwin rustc --release --target $TARGET -p scarlett --bin scarlett --features diagnostics --locked \
      -- -C link-arg=/EXPORT:D3D12SDKVersion,DATA -C link-arg=/EXPORT:D3D12SDKPath,DATA
    echo "$EXE"
    ;;
  exe) shift; proton "$EXE" "$@" ;;
  winpath) winpath "$2"; echo ;;
  smoke)
    # Capture, then trim: piping into `head` closes the pipe and scarlett.exe panics (os error 233).
    out=$(proton "$EXE" --help); head -3 <<<"$out"
    proton "$EXE" fingerprint
    out=$(proton "$EXE" inspect "$(winpath "$EXE")"); head -6 <<<"$out"
    echo "smoke ok"
    ;;
  test)
    shift; crates=(); filter=()
    while (($#)); do [[ $1 == -- ]] && { shift; filter=("$@"); break; }; crates+=("$1"); shift; done
    ((${#crates[@]})) || crates=("${NOGPU_CRATES[@]}")
    feats=(); [[ " ${crates[*]} " == *" scarlett-gpu "* ]] && feats=(--features scarlett-gpu/diagnostics)
    cd "$ENGINE"
    mapfile -t exes < <(cargo xwin test --release --locked --target $TARGET --lib --no-run "${feats[@]}" \
        $(printf -- '-p %s ' "${crates[@]}") 2>&1 | sed -nE 's/.*Executable .*\((.*\.exe)\)/\1/p')
    # No test program: the build failed (its errors were filtered out above). Not a pass.
    ((${#exes[@]})) || { echo "sp.sh: the tests did not build (cargo xwin test --no-run for the errors)" >&2; exit 1; }
    fail=0
    for t in "${exes[@]}"; do
      echo "== $(basename "$t")"
      threads=(--test-threads=4); [[ " ${filter[*]} " == *" --test-threads"* ]] && threads=()
      res=$(proton "$ENGINE/$t" "${threads[@]}" "${filter[@]}" 2>&1 | grep -E "^test result|panicked at|readback|skipping|^error" || true)
      grep -E "^test result|readback|skipping" <<<"$res"; grep -q '^test result: ok' <<<"$res" || { fail=1; grep -E 'panicked at|^error' <<<"$res" | head -5; }
    done
    exit $fail
    ;;
  *) sed -n '2,13p' "$0"; exit 2 ;;
esac
