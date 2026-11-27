# Benchmark results

Results of [multipart_bench](https://github.com/defnull/multipart_bench) at the
commit pinned in `multipart_bench/run.py`, with `--profile default` (95%
confidence, ±2% precision). Throughput in MB/s; percentages are relative to
`multipart`. Higher is better.

## Setup

- Intel Core i7-1260P, Linux 7.0 (Ubuntu kernel 7.0.0-44-generic), Python 3.12.12.
- Measured with `multipart_bench/tuned.sh`: `pyperf system tune` (turbo off,
  2.1 GHz), power-profiles-daemon stopped, all movable interrupts moved to other
  CPUs.
- The benchmark ran on one P-core thread, with its sibling idle and both
  isolated from other processes by a cgroup cpuset partition, under
  `nice -n -20` and `chrt -f 99` as in the upstream Makefile.
- Parsers: kirimi 0.1.0, multipart 2.0.1, python-multipart 0.0.34,
  werkzeug 3.1.9, django 6.1.3, streaming-form-data 2.1.0, emmett-core 1.4.7.
- Raw timings, the exact setup and the machine's load during the run are in
  [`multipart_bench/results/2026-11-27T1801/`](multipart_bench/results/2026-11-27T1801/). Other
  CPUs were 0.4% busy on average.

## Summary

| Scenario | Non-blocking: multipart | Non-blocking: kirimi | Blocking: multipart | Blocking: kirimi |
|---|---:|---:|---:|---:|
| simple | 32.75 | 163.10 (498%) | 20.34 | 154.77 (761%) |
| large | 45.34 | 263.81 (582%) | 31.25 | 216.21 (692%) |
| upload | 4374.84 | 15281.60 (349%) | 1838.08 | 2340.68 (127%) |
| mixed | 4217.43 | 14715.02 (349%) | 1653.46 | 2420.90 (146%) |
| worstcase_crlf | 3161.57 | 9759.52 (309%) | 1407.68 | 2271.71 (161%) |
| worstcase_lf | 4195.53 | 14708.43 (351%) | 1587.98 | 2339.48 (147%) |
| worstcase_bchar | 3170.62 | 9536.19 (301%) | 1407.94 | 2260.59 (161%) |
| worstcase_junk | 3585.13 | 17544.78 (489%) | 3515.61 | 18479.19 (526%) |

In the blocking scenarios with files larger than the spool limit (500 KB),
most of the time is spent writing temporary files. Writing 32 MB to a fresh
temporary file alone takes about 70% of the blocking `upload` time.

Compared with the previous results (kernel 7.0.0-38, October 2026), the
scenarios that move large amounts of data got slower for most parsers,
including those whose version did not change: `multipart` 2.0.1 by 12–25%
(non-blocking) and 4–12% (blocking), werkzeug 3.1.9 by 2–15%. The small-form
scenarios are within ±4%, so the cause is in the system, not in the parsers.
kirimi's blocking results improved by up to 6% despite this, because spooled
parts are now written to the temporary file directly.

## All parsers

### simple

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 32.75 (100%) | 20.34 (100%) |
| kirimi | 163.10 (498%) | 154.77 (761%) |
| python-multipart | 15.34 (47%) | 6.98 (34%) |
| werkzeug | 6.55 (20%) | 4.82 (24%) |
| django | - | 2.03 (10%) |
| streaming-form-data | 0.66 (2%) | 0.62 (3%) |
| cgi | - | 3.93 (19%) |
| email | 3.59 (11%) | 3.19 (16%) |
| emmett-core | - | 69.71 (343%) |

### large

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 45.34 (100%) | 31.25 (100%) |
| kirimi | 263.81 (582%) | 216.21 (692%) |
| python-multipart | 23.12 (51%) | 12.53 (40%) |
| werkzeug | 9.61 (21%) | 7.95 (25%) |
| django | - | 4.18 (13%) |
| streaming-form-data | 0.94 (2%) | 0.91 (3%) |
| cgi | - | 6.21 (20%) |
| email | 10.25 (23%) | 8.85 (28%) |
| emmett-core | - | 117.48 (376%) |

### upload

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 4374.84 (100%) | 1838.08 (100%) |
| kirimi | 15281.60 (349%) | 2340.68 (127%) |
| python-multipart | 3893.68 (89%) | 1722.68 (94%) |
| werkzeug | 1363.14 (31%) | 889.77 (48%) |
| django | - | 946.23 (51%) |
| streaming-form-data | 3586.75 (82%) | 1508.81 (82%) |
| cgi | - | 133.08 (7%) |
| email | 55.50 (1%) | 49.13 (3%) |
| emmett-core | - | 2300.37 (125%) |

### mixed

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 4217.43 (100%) | 1653.46 (100%) |
| kirimi | 14715.02 (349%) | 2420.90 (146%) |
| python-multipart | 3720.05 (88%) | 1317.18 (80%) |
| werkzeug | 1287.12 (31%) | 823.74 (50%) |
| django | - | 831.85 (50%) |
| streaming-form-data | 1708.30 (41%) | 968.58 (59%) |
| cgi | - | 131.88 (8%) |
| email | 60.25 (1%) | 57.90 (4%) |
| emmett-core | - | 2348.59 (142%) |

### worstcase_crlf

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 3161.57 (100%) | 1407.68 (100%) |
| kirimi | 9759.52 (309%) | 2271.71 (161%) |
| python-multipart | 3486.98 (110%) | 1168.20 (83%) |
| werkzeug | 2092.59 (66%) | 1032.39 (73%) |
| django | - | 1023.15 (73%) |
| streaming-form-data | 28.77 (1%) | 28.02 (2%) |
| cgi | - | 3.73 (0%) |
| email | 3.72 (0%) | 3.70 (0%) |
| emmett-core | - | 2325.88 (165%) |

### worstcase_lf

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 4195.53 (100%) | 1587.98 (100%) |
| kirimi | 14708.43 (351%) | 2339.48 (147%) |
| python-multipart | 3684.21 (88%) | 1285.94 (81%) |
| werkzeug | 1988.47 (47%) | 1023.84 (64%) |
| django | - | 1016.87 (64%) |
| streaming-form-data | 1587.03 (38%) | 888.49 (56%) |
| cgi | - | 1.71 (0%) |
| email | 2.34 (0%) | 2.33 (0%) |
| emmett-core | - | 2316.07 (146%) |

### worstcase_bchar

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 3170.62 (100%) | 1407.94 (100%) |
| kirimi | 9536.19 (301%) | 2260.59 (161%) |
| python-multipart | 2636.80 (83%) | 1040.21 (74%) |
| werkzeug | 1665.59 (53%) | 922.41 (66%) |
| django | - | 902.65 (64%) |
| streaming-form-data | 29.80 (1%) | 29.34 (2%) |
| cgi | - | 68.99 (5%) |
| email | 48.83 (2%) | 47.28 (3%) |
| emmett-core | - | 1265.18 (90%) |

### worstcase_junk

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 3585.13 (100%) | 3515.61 (100%) |
| kirimi | 17544.78 (489%) | 18479.19 (526%) |
| python-multipart | failed | failed |
| werkzeug | 48.17 (1%) | 47.91 (1%) |
| django | - | failed |
| streaming-form-data | failed | failed |
| cgi | - | 1111.47 (32%) |
| email | 113.99 (3%) | 113.19 (3%) |
| emmett-core | - | 18511.59 (527%) |
