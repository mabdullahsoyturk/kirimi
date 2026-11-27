"""Collect the results of a multipart_bench run made by ``tuned.sh``.

    python collect_results.py WORKDIR OUTDIR

Copies the rendered report (``report.md``), combines the per-benchmark timings
into ``timings.json`` and summarizes the load recorded in ``OUTDIR/load`` into
``load-summary.txt``, comparing the baseline before the run with the run.
"""

import datetime
import json
import shutil
import statistics
import sys
from collections import defaultdict
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path

BENCH_CPU = "2"


@dataclass
class Period:
    """Load samples of one period, keyed by the end time of their interval."""

    name: str
    start: float
    end: float
    memory_gbs: list[float] = field(default_factory=list)
    other_cpus_busy: list[float] = field(default_factory=list)
    busiest_other_cpu: list[float] = field(default_factory=list)
    bench_cpu_busy: list[float] = field(default_factory=list)
    load1: list[float] = field(default_factory=list)
    pkg_temp_c: list[float] = field(default_factory=list)
    cpu_mhz: list[float] = field(default_factory=list)
    process_cpu: dict[str, float] = field(default_factory=lambda: defaultdict(float))
    process_samples: set[float] = field(default_factory=set)

    def contains(self, time: float, interval: float) -> bool:
        """Whether most of the interval ending at ``time`` is in this period."""
        return self.start <= time - interval / 2 < self.end


def save_report(workdir: Path, outdir: Path) -> None:
    shutil.copyfile(workdir / "README.md", outdir / "report.md")
    timings = {}
    for path in sorted((workdir / "var").glob("*.json")):
        result = json.loads(path.read_text())
        timings[result["name"]] = {"size": result["size"], "times": result["times"]}
    (outdir / "timings.json").write_text(json.dumps(timings, indent=1) + "\n")


def clock_times(lines: Iterable[str], start: float) -> Iterable[tuple[float, list[str]]]:
    """Yield (epoch, fields) for sysstat lines that start with HH:MM:SS."""
    day = datetime.datetime.fromtimestamp(start).date()
    previous = start - 1
    for line in lines:
        fields = line.split()
        if not fields or len(fields[0]) != 8 or fields[0][2] != ":":
            continue
        clock = datetime.time.fromisoformat(fields[0])
        time = datetime.datetime.combine(day, clock).timestamp()
        while time < previous - 3600:  # past midnight
            day += datetime.timedelta(days=1)
            time = datetime.datetime.combine(day, clock).timestamp()
        previous = time
        yield time, fields[1:]


def read_mpstat(path: Path, start: float, interval: float, periods: list[Period]) -> None:
    busy: dict[float, dict[str, float]] = defaultdict(dict)
    for time, fields in clock_times(path.read_text().splitlines(), start):
        if fields[0] == "CPU" or fields[0] == "all":
            continue
        busy[time][fields[0]] = 100 - float(fields[-1])  # last column: %idle
    for time, cpus in busy.items():
        others = [b for cpu, b in cpus.items() if cpu not in (BENCH_CPU, sibling(BENCH_CPU))]
        for period in periods:
            if period.contains(time, interval) and others:
                period.other_cpus_busy.append(statistics.fmean(others))
                period.busiest_other_cpu.append(max(others))
                if BENCH_CPU in cpus:
                    period.bench_cpu_busy.append(cpus[BENCH_CPU])


def sibling(cpu: str) -> str:
    path = Path(f"/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list")
    siblings = path.read_text().strip().replace("-", ",").split(",") if path.exists() else []
    return next((s for s in siblings if s != cpu), cpu)


def read_pidstat(path: Path, start: float, interval: float, periods: list[Period]) -> None:
    # Fields after the time: UID PID %usr %system %guest %wait %CPU CPU Command
    for time, fields in clock_times(path.read_text().splitlines(), start):
        if fields[0] == "UID" or len(fields) < 9:
            continue
        name = f"{' '.join(fields[8:])} (pid {fields[1]})"
        for period in periods:
            if period.contains(time, interval):
                period.process_cpu[name] += float(fields[6])
                period.process_samples.add(time)


def read_memory(path: Path, start: float, periods: list[Period]) -> None:
    """perf stat -I CSV: seconds since start, MiB, unit, event, ..."""
    traffic: dict[float, float] = defaultdict(float)
    for line in path.read_text().splitlines():
        fields = line.split(",")
        if len(fields) < 4 or line.startswith("#"):
            continue
        try:
            traffic[float(fields[0])] += float(fields[1])
        except ValueError:  # e.g. "<not counted>"
            continue
    previous = 0.0
    for offset in sorted(traffic):
        duration, previous = offset - previous, offset
        if duration <= 0:
            continue
        gbs = traffic[offset] * 2**20 / duration / 1e9
        for period in periods:
            if period.contains(start + offset, duration):
                period.memory_gbs.append(gbs)


def read_sensors(path: Path, periods: list[Period]) -> None:
    for line in path.read_text().splitlines()[1:]:
        fields = line.split(",")
        if len(fields) != 6:
            continue
        time = float(fields[0])
        for period in periods:
            if period.start <= time < period.end:
                period.load1.append(float(fields[1]))
                period.pkg_temp_c.append(float(fields[4]))
                period.cpu_mhz.append(float(fields[5]))


def read_interrupts(path: Path, cpu: str) -> dict[str, tuple[int, str]]:
    """Interrupt counts on ``cpu`` from a copy of /proc/interrupts."""
    lines = path.read_text().splitlines()
    column = lines[0].split().index(f"CPU{cpu}")
    counts = {}
    for line in lines[1:]:
        fields = line.split()
        if len(fields) > column + 1 and fields[column + 1].isdigit():
            description = " ".join(fields[len(lines[0].split()) + 1 :])
            counts[fields[0].rstrip(":")] = (int(fields[column + 1]), description)
    return counts


def interrupt_lines(load: Path) -> list[str]:
    before_path, after_path = load / "interrupts-before.txt", load / "interrupts-after.txt"
    if not (before_path.exists() and after_path.exists()):
        return []
    before = read_interrupts(before_path, BENCH_CPU)
    after = read_interrupts(after_path, BENCH_CPU)
    deltas = [
        (count - before.get(irq, (0, ""))[0], irq, description)
        for irq, (count, description) in after.items()
    ]
    deltas = sorted((d for d in deltas if d[0] > 0), reverse=True)
    lines = ["", f"Interrupts on CPU {BENCH_CPU} during the benchmark:"]
    lines += [f"  {count:9}  {irq:>5}  {description}" for count, irq, description in deltas]
    return lines if deltas else [*lines, "  none"]


def summarize(outdir: Path, interval: float) -> str:
    load = outdir / "load"
    markers = {
        key: float(value)
        for key, value in (
            line.split("=", 1) for line in (load / "markers.txt").read_text().splitlines()
        )
    }
    start = markers["recorders_start"]
    baseline = Period("baseline", start, markers["bench_start"])
    run = Period("benchmark", markers["bench_start"], markers["bench_end"])
    periods = [baseline, run]
    read_mpstat(load / "mpstat.txt", start, interval, periods)
    read_pidstat(load / "pidstat.txt", start, interval, periods)
    read_memory(load / "memory.csv", start, periods)
    read_sensors(load / "sensors.csv", periods)

    def stat(values: list[float], how: str, digits: int = 1) -> str:
        if not values:
            return "-"
        if how == "range":
            return f"{min(values):.0f}-{max(values):.0f}"
        value = statistics.fmean(values) if how == "mean" else max(values)
        return f"{value:.{digits}f}"

    rows = [
        ("duration (min)", [f"{(p.end - p.start) / 60:.1f}" for p in periods]),
        ("samples", [str(len(p.other_cpus_busy)) for p in periods]),
        ("memory traffic, mean (GB/s)", [stat(p.memory_gbs, "mean", 2) for p in periods]),
        ("memory traffic, max (GB/s)", [stat(p.memory_gbs, "max", 2) for p in periods]),
        ("other CPUs busy, mean (%)", [stat(p.other_cpus_busy, "mean") for p in periods]),
        ("other CPUs busy, max (%)", [stat(p.other_cpus_busy, "max") for p in periods]),
        ("busiest other CPU, max (%)", [stat(p.busiest_other_cpu, "max") for p in periods]),
        (f"CPU {BENCH_CPU} busy, mean (%)", [stat(p.bench_cpu_busy, "mean") for p in periods]),
        ("load average (1 min), max", [stat(p.load1, "max", 2) for p in periods]),
        ("package temperature, max (C)", [stat(p.pkg_temp_c, "max", 0) for p in periods]),
        (f"CPU {BENCH_CPU} frequency (MHz)", [stat(p.cpu_mhz, "range") for p in periods]),
    ]
    width = max(len(label) for label, _ in rows)
    lines = [
        f"Load recorded every {interval:.0f} s. 'Other CPUs' excludes CPU {BENCH_CPU}",
        "(benchmark) and its sibling. Memory traffic is system-wide and includes",
        "the benchmark's own.",
        "",
        f"{'':{width}}  {'baseline':>10}  {'benchmark':>10}",
    ]
    lines += [f"{label:{width}}  {a:>10}  {b:>10}" for label, (a, b) in rows]
    lines += ["", "Processes with the most CPU time during the benchmark (mean % of one CPU):"]
    samples = max(len(run.process_samples), 1)
    top = sorted(run.process_cpu.items(), key=lambda item: -item[1])[:12]
    lines += [f"  {total / samples:6.1f}  {name}" for name, total in top]
    lines += interrupt_lines(load)
    return "\n".join(lines) + "\n"


def main() -> None:
    workdir, outdir = Path(sys.argv[1]), Path(sys.argv[2])
    interval = float(sys.argv[3]) if len(sys.argv) > 3 else 15.0
    save_report(workdir, outdir)
    (outdir / "load-summary.txt").write_text(summarize(outdir, interval))


if __name__ == "__main__":
    main()
