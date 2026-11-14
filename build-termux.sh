#!/data/data/com.termux/files/usr/bin/bash
#
# Build WordCraft natively in Termux, tuned to survive Android's low-memory killer.
#
# Why the limits: this tablet has 5.7 GB RAM and usually ~1.5 GB available.
# Android's lmkd/duraspeed kills processes under memory pressure, and a plain
# `cargo build` would start 8 rustc jobs (one per core) and get the build (or the
# whole Termux app) killed mid-way. Two jobs plus no debug info keeps peak
# memory in the ~1-1.5 GB range.
#
set -e
cd "$(dirname "$0")"

JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_BUILD_JOBS="$JOBS"
export CARGO_PROFILE_DEV_DEBUG=0          # no debug sections: less RAM + disk per rustc
export CARGO_NET_GIT_FETCH_WITH_CLI=true  # Termux git works better than cargo's libgit2 here

avail_kb=$(awk '/MemAvailable/{print $2}' /proc/meminfo)
echo "[*] MemAvailable: $((avail_kb / 1024)) MB, using $JOBS parallel rustc jobs"
if [ "$avail_kb" -lt 700000 ]; then
    echo "[!] Low memory right now. Close other apps or run 'am kill-all' first." >&2
fi

# Watchdog: when Android gets close to the low-memory killer, pause the compilers for a moment so
# the kernel reclaims page cache, instead of letting lmkd kill the build (or Termux itself).
# rustc keeps its memory while paused, but stops asking for more.
watch_memory() {
    local cargo_pid=$1 paused=0 avail
    while kill -0 "$cargo_pid" 2>/dev/null; do
        avail=$(awk '/^MemAvailable/{print $2}' /proc/meminfo)
        if [ "$paused" -eq 0 ] && [ "$avail" -lt 400000 ]; then
            echo "[!] MemAvailable $((avail / 1024)) MB: pausing rustc until memory recovers" >&2
            pkill -STOP -x rustc 2>/dev/null
            paused=1
        elif [ "$paused" -eq 1 ] && [ "$avail" -gt 700000 ]; then
            echo "[*] MemAvailable $((avail / 1024)) MB: resuming rustc" >&2
            pkill -CONT -x rustc 2>/dev/null
            paused=0
        fi
        sleep 2
    done
    if [ "$paused" -eq 1 ]; then pkill -CONT -x rustc 2>/dev/null; fi
}
trap 'pkill -CONT -x rustc 2>/dev/null || true' EXIT INT TERM

# nice: keep the desktop / system responsive while the compiler runs
nice -n 10 cargo build --locked -p wordcraft --jobs "$JOBS" "$@" &
cargo_pid=$!
watch_memory "$cargo_pid" &
watch_pid=$!
wait "$cargo_pid"
status=$?
wait "$watch_pid" 2>/dev/null
exit "$status"
