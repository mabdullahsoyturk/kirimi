#!/usr/bin/env bash
# Run multipart_bench on a tuned, isolated CPU core and record the machine's
# load while it runs. This is how the results in bench/RESULTS.md are made.
#
#     python bench/multipart_bench/run.py --setup-only   # as yourself, first
#     sudo bash bench/multipart_bench/tuned.sh
#
# Run it from a terminal that stays open for the whole run (about 10 minutes),
# not through a tool with a time limit for commands.
#
# Setup:
#
# - `pyperf system tune` (turbo off, fixed base frequency), with thermald and
#   power-profiles-daemon stopped.
# - The benchmark runs on CPU 2 with its hyperthread sibling CPU 3 idle. Both
#   are isolated from all other processes by a cgroup v2 cpuset partition, and
#   all interrupts that can be moved are moved to other CPUs. This also keeps
#   devices working: interrupt handler threads run at real-time priority 50,
#   so on CPU 2 they would never run while the benchmark does (a Wi-Fi card
#   whose interrupts were handled there stopped responding and needed a
#   reboot).
# - `nice -n -20 chrt -f 99` and TEMP on tmpfs, as in the upstream Makefile.
#   The benchmark itself runs as the invoking user.
#
# Isolation does not cover shared caches and memory bandwidth, so other
# programs can still slow down the benchmark. The load is therefore recorded
# for a baseline period before the run and during the run.
#
# Everything is written to results/<timestamp>/ next to this script while the
# benchmark runs: setup.txt, bench.log, load/ (raw recordings) and, once the
# run is complete, report.md, timings.json and load-summary.txt. System
# settings are restored on exit, also on errors, Ctrl-C and SIGTERM; a cgroup
# left behind by a run that was killed is removed at the start.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
WORK=$HERE/work
PY=$WORK/.venv/bin/python
CPU=2
SIBLING=3
CG=/sys/fs/cgroup/kirimi-bench
INTERVAL=15         # seconds between load samples
BASELINE=30         # seconds of load recording before the benchmark starts

if [[ $EUID -ne 0 || -z ${SUDO_USER:-} ]]; then
    echo "Run with sudo: sudo bash $0" >&2
    exit 1
fi
if ! "$PY" -c "import pyperf, psutil" 2>/dev/null; then
    echo "Set up the benchmark first: python $HERE/run.py --setup-only" >&2
    exit 1
fi
for tool in mpstat pidstat perf; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done
TEMP_DIR=/run/user/$(id -u "$SUDO_USER")
[[ -d $TEMP_DIR ]] || { echo "$TEMP_DIR does not exist" >&2; exit 1; }
export LC_ALL=C
# Root must not leave bytecode files in the user's venv.
export PYTHONDONTWRITEBYTECODE=1

OUT=$HERE/results/$(date +%Y-%m-%dT%H%M)
mkdir -p "$OUT/load"
MARKERS=$OUT/load/markers.txt

stopped=()
recorders=()
moved_irqs=()   # "irq:original affinity"

# Whether CPU $2 is in the CPU list $1 (e.g. "0-3,8").
in_cpu_list() {
    local part
    local -a parts
    IFS=, read -ra parts <<<"$1"
    for part in "${parts[@]}"; do
        if (($2 >= ${part%-*} && $2 <= ${part#*-})); then
            return 0
        fi
    done
    return 1
}

irq_name() {
    awk -v irq="$1:" '$1 == irq { print $NF }' /proc/interrupts
}

# Move all interrupts that can be moved off the benchmark CPUs.
move_irqs() {
    local others=() dir cpu irq old
    for dir in /sys/devices/system/cpu/cpu[0-9]*; do
        cpu=${dir##*cpu}
        [[ $cpu == "$CPU" || $cpu == "$SIBLING" ]] && continue
        [[ -f $dir/online && $(cat "$dir/online") == 0 ]] && continue
        others+=("$cpu")
    done
    local targets
    targets=$(IFS=,; echo "${others[*]}")
    for dir in /proc/irq/[0-9]*; do
        irq=${dir##*/}
        [[ -r $dir/effective_affinity_list ]] || continue
        local effective
        effective=$(cat "$dir/effective_affinity_list")
        [[ -n $effective ]] || continue
        if in_cpu_list "$effective" "$CPU" || in_cpu_list "$effective" "$SIBLING"; then
            old=$(cat "$dir/smp_affinity_list")
            if echo "$targets" >"$dir/smp_affinity_list" 2>/dev/null; then
                moved_irqs+=("$irq:$old")
                echo "  moved IRQ $irq ($(irq_name "$irq")) from CPUs $old"
            else
                echo "  could not move IRQ $irq ($(irq_name "$irq")), kernel-managed"
                if pgrep "^irq/$irq-" >/dev/null; then
                    echo "  WARNING: IRQ $irq has a handler thread that cannot run while" \
                        "the benchmark runs; its device may stop working" >&2
                fi
            fi
        fi
    done
}

# List interrupts that are still delivered to the benchmark CPUs.
remaining_irqs() {
    local dir irq effective
    for dir in /proc/irq/[0-9]*; do
        irq=${dir##*/}
        effective=$(cat "$dir/effective_affinity_list" 2>/dev/null) || continue
        [[ -n $effective ]] || continue
        if in_cpu_list "$effective" "$CPU" || in_cpu_list "$effective" "$SIBLING"; then
            echo "  still on CPUs $effective: IRQ $irq ($(irq_name "$irq"))"
        fi
    done
}

restore_irqs() {
    local entry
    for entry in "${moved_irqs[@]}"; do
        echo "${entry#*:}" >"/proc/irq/${entry%%:*}/smp_affinity_list" 2>/dev/null
    done
    moved_irqs=()
}
stop_recorders() {
    # mpstat, pidstat and perf finish their output on SIGINT. The sensor loop
    # is a subshell, which ignores SIGINT when started in the background.
    for pid in "${recorders[@]}"; do
        kill -INT "$pid" 2>/dev/null || true
    done
    if [[ -n ${sensor_loop:-} ]]; then
        kill -TERM "$sensor_loop" 2>/dev/null || true
        wait "$sensor_loop" 2>/dev/null || true
        sensor_loop=
    fi
    for pid in "${recorders[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    recorders=()
}
# Stop everything in the benchmark cgroup and give its CPUs back.
remove_cgroup() {
    [[ -d $CG ]] || return 0
    echo 1 >"$CG/cgroup.kill" 2>/dev/null
    for _ in {1..50}; do
        [[ -s $CG/cgroup.procs ]] || break
        sleep 0.1
    done
    echo member >"$CG/cpuset.cpus.partition" 2>/dev/null
    rmdir "$CG"
}

# The most important settings come first, in case the cleanup is cut short.
restore() {
    trap '' INT TERM HUP
    set +e
    echo "== Restoring system settings"
    remove_cgroup
    "$PY" -m pyperf system reset >/dev/null 2>&1
    restore_irqs # after pyperf, which may reset IRQ affinities
    for service in "${stopped[@]}"; do
        systemctl start "$service"
    done
    stop_recorders
    chown -R "$SUDO_USER:" "$HERE/results"
    echo "== Results: $OUT"
}
trap restore EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

if [[ -d $CG ]]; then
    echo "== Removing the cgroup of an earlier run"
    remove_cgroup
fi

echo "== Writing results to $OUT"
{
    echo "date: $(date -Iseconds)"
    echo "kernel: $(uname -r)"
    echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | xargs)"
    echo "repository: $(runuser -u "$SUDO_USER" -- git -C "$REPO" describe --always --dirty)"
    echo "packages:"
    "$PY" -c "
import importlib.metadata as m
for name in ['kirimi-multipart', 'multipart', 'python-multipart', 'werkzeug', 'django',
             'streaming-form-data', 'emmett-core', 'pyperf']:
    print(f'  {name} {m.version(name)}')
print('  python', __import__('sys').version.split()[0])"
} >"$OUT/setup.txt"

echo "== Stopping power management daemons"
for service in thermald power-profiles-daemon; do
    if systemctl is-active --quiet "$service"; then
        systemctl stop "$service"
        stopped+=("$service")
    fi
done
echo "stopped services: ${stopped[*]:-none}" >>"$OUT/setup.txt"

echo "== pyperf system tune"
"$PY" -m pyperf system tune || echo "(pyperf reported problems; see above)"

echo "== Moving interrupts off CPUs $CPU-$SIBLING"
# Not in a pipeline: move_irqs must run in this shell to record what it moved.
{
    echo "interrupts on CPUs $CPU-$SIBLING:"
    move_irqs
    sleep 1
    remaining_irqs
} >>"$OUT/setup.txt"
sed -n '/^interrupts on CPUs/,$p' "$OUT/setup.txt"

echo "== Isolating CPUs $CPU-$SIBLING"
mkdir "$CG"
echo "$CPU-$SIBLING" >"$CG/cpuset.cpus"
echo 0 >"$CG/cpuset.mems"
echo isolated >"$CG/cpuset.cpus.partition"
partition=$(cat "$CG/cpuset.cpus.partition")
if [[ $partition != isolated ]]; then
    echo "Could not isolate CPUs: $partition" >&2
    exit 1
fi
{
    echo "cpuset partition: CPUs $CPU-$SIBLING $partition, benchmark on CPU $CPU"
    echo
    "$PY" -m pyperf system show 2>&1
    echo
} >>"$OUT/setup.txt"

echo "== Recording load every ${INTERVAL}s (baseline: ${BASELINE}s)"
pkg_temp=$(grep -l x86_pkg_temp /sys/class/thermal/thermal_zone*/type | head -1)
pkg_temp=${pkg_temp%/type}/temp
echo "recorders_start=$(date +%s.%N)" >"$MARKERS"
nice -n 19 mpstat -P ALL "$INTERVAL" >"$OUT/load/mpstat.txt" &
recorders+=($!)
nice -n 19 pidstat -u -h "$INTERVAL" >"$OUT/load/pidstat.txt" &
recorders+=($!)
nice -n 19 perf stat -a -x, -I $((INTERVAL * 1000)) -o "$OUT/load/memory.csv" \
    -e uncore_imc_free_running_0/data_total/,uncore_imc_free_running_1/data_total/ &
recorders+=($!)
(
    echo "time,load1,load5,load15,pkg_temp_c,cpu${CPU}_mhz"
    while :; do
        read -r l1 l5 l15 _ </proc/loadavg
        echo "$(date +%s),$l1,$l5,$l15,$(($(cat "$pkg_temp") / 1000)),$(($(cat /sys/devices/system/cpu/cpu$CPU/cpufreq/scaling_cur_freq) / 1000))"
        sleep "$INTERVAL"
    done
) >"$OUT/load/sensors.csv" &
sensor_loop=$!
# In the background, so that signals are handled while waiting.
sleep "$BASELINE" &
wait $!

sync
echo 3 >/proc/sys/vm/drop_caches

echo "== Running multipart_bench (default profile); progress: tail -f $OUT/bench.log"
cp /proc/interrupts "$OUT/load/interrupts-before.txt"
echo "bench_start=$(date +%s.%N)" >>"$MARKERS"
start=$(date +%s)
status=0
# The subshell joins the isolated cgroup first; the benchmark inherits it,
# the real-time policy and the nice level, and runs as the invoking user.
# Desktop sessions often limit how long a real-time task may run without
# blocking (RLIMIT_RTTIME, e.g. 200 ms), after which the kernel kills it, so
# the limit is lifted for the benchmark.
# shellcheck disable=SC2016
bash -c '
    echo $$ >"$1/cgroup.procs"
    ulimit -S -H -R unlimited
    {
        echo "benchmark process:"
        chrt -f 99 nice -n -20 runuser -u "$2" -- taskset -c "$4" bash -c \
            "echo \"  rttime=\$(ulimit -R) cgroup=\$(cat /proc/\$\$/cgroup)\"; ps -o user,cls,rtprio,ni,psr -p \$\$ | sed \"s/^/  /\""
    } >>"$7"
    exec chrt -f 99 nice -n -20 runuser -u "$2" -- \
        env TEMP="$3" taskset -c "$4" "$5" "$6/run.py" --skip-install --profile default
' _ "$CG" "$SUDO_USER" "$TEMP_DIR" "$CPU" "$PY" "$HERE" "$OUT/setup.txt" \
    >"$OUT/bench.log" 2>&1 &
wait $! || status=$?
echo "bench_end=$(date +%s.%N)" >>"$MARKERS"
cp /proc/interrupts "$OUT/load/interrupts-after.txt"
echo "== Benchmark finished in $((($(date +%s) - start) / 60)) minutes (exit status $status)"
stop_recorders

if [[ $status -eq 0 ]]; then
    "$PY" "$HERE/collect_results.py" "$WORK" "$OUT" "$INTERVAL"
    cat "$OUT/load-summary.txt"
else
    echo "The benchmark failed; see $OUT/bench.log" >&2
fi
exit "$status"
