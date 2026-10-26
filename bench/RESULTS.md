# Benchmark results

Results of [multipart_bench](https://github.com/defnull/multipart_bench) at the
commit pinned in `multipart_bench/run.py`, with `--profile default` (95%
confidence, ±2% precision). Throughput in MB/s; percentages are relative to
`multipart`. Higher is better.

## Setup

- Intel Core i7-1260P, Linux 7.0, Python 3.12.12.
- `pyperf system tune` (turbo off, 2.1 GHz); thermald and power-profiles-daemon
  stopped.
- The benchmark ran on one P-core thread, with its sibling idle and both
  isolated from other processes by cgroup cpusets, under `nice -n -20` and
  `chrt -f 99` as in the upstream Makefile.
- Parsers: kirimi 0.1.0, multipart 2.0.1, python-multipart 0.0.32,
  werkzeug 3.1.9, django 6.1.2, streaming-form-data 2.1.0, emmett-core 1.4.6.

## Summary

| Scenario | Non-blocking: multipart | Non-blocking: kirimi | Blocking: multipart | Blocking: kirimi |
|---|---:|---:|---:|---:|
| simple | 33.08 | 163.99 (496%) | 19.59 | 155.56 (794%) |
| large | 45.93 | 252.73 (550%) | 30.29 | 213.63 (705%) |
| upload | 5462.83 | 18083.24 (331%) | 2090.90 | 2352.74 (113%) |
| mixed | 5192.02 | 16965.95 (327%) | 1846.99 | 2381.70 (129%) |
| worstcase_crlf | 3608.83 | 10538.54 (292%) | 1604.44 | 2136.16 (133%) |
| worstcase_lf | 5174.39 | 17376.41 (336%) | 1684.13 | 2295.61 (136%) |
| worstcase_bchar | 4223.66 | 10514.55 (249%) | 1461.62 | 2147.65 (147%) |
| worstcase_junk | 4147.54 | 18067.19 (436%) | 3899.30 | 18802.76 (482%) |

In the blocking scenarios with files larger than the spool limit (500 KB),
most of the time is spent writing temporary files. Writing 32 MB to a fresh
temporary file alone takes about 80% of the blocking `upload` time.

## All parsers

### simple

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 33.08 (100%) | 19.59 (100%) |
| kirimi | 163.99 (496%) | 155.56 (794%) |
| python-multipart | 9.33 (28%) | 5.41 (28%) |
| werkzeug | 6.45 (19%) | 4.85 (25%) |
| django | - | 2.01 (10%) |
| streaming-form-data | 0.65 (2%) | 0.62 (3%) |
| cgi | - | 3.97 (20%) |
| email | 3.54 (11%) | 3.02 (15%) |
| emmett-core | - | 69.89 (357%) |

### large

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 45.93 (100%) | 30.29 (100%) |
| kirimi | 252.73 (550%) | 213.63 (705%) |
| python-multipart | 16.30 (35%) | 9.98 (33%) |
| werkzeug | 9.45 (21%) | 7.86 (26%) |
| django | - | 4.07 (13%) |
| streaming-form-data | 0.94 (2%) | 0.91 (3%) |
| cgi | - | 6.19 (20%) |
| email | 10.11 (22%) | 8.71 (29%) |
| emmett-core | - | 124.60 (411%) |

### upload

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 5462.83 (100%) | 2090.90 (100%) |
| kirimi | 18083.24 (331%) | 2352.74 (113%) |
| python-multipart | 4669.09 (85%) | 1830.61 (88%) |
| werkzeug | 1492.12 (27%) | 955.56 (46%) |
| django | - | 1003.83 (48%) |
| streaming-form-data | 3585.16 (66%) | 1508.79 (72%) |
| cgi | - | 132.51 (6%) |
| email | 55.37 (1%) | 48.86 (2%) |
| emmett-core | - | 2358.91 (113%) |

### mixed

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 5192.02 (100%) | 1846.99 (100%) |
| kirimi | 16965.95 (327%) | 2381.70 (129%) |
| python-multipart | 4276.13 (82%) | 1451.00 (79%) |
| werkzeug | 1393.27 (27%) | 863.74 (47%) |
| django | - | 859.80 (47%) |
| streaming-form-data | 1700.68 (33%) | 957.13 (52%) |
| cgi | - | 131.64 (7%) |
| email | 60.56 (1%) | 57.74 (3%) |
| emmett-core | - | 2432.05 (132%) |

### worstcase_crlf

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 3608.83 (100%) | 1604.44 (100%) |
| kirimi | 10538.54 (292%) | 2136.16 (133%) |
| python-multipart | 3832.52 (106%) | 1233.42 (77%) |
| werkzeug | 2309.88 (64%) | 1112.44 (69%) |
| django | - | 1086.94 (68%) |
| streaming-form-data | 28.88 (1%) | 27.95 (2%) |
| cgi | - | 3.68 (0%) |
| email | 3.77 (0%) | 3.75 (0%) |
| emmett-core | - | 2404.19 (150%) |

### worstcase_lf

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 5174.39 (100%) | 1684.13 (100%) |
| kirimi | 17376.41 (336%) | 2295.61 (136%) |
| python-multipart | 4080.25 (79%) | 1371.43 (81%) |
| werkzeug | 2329.04 (45%) | 1101.05 (65%) |
| django | - | 1101.48 (65%) |
| streaming-form-data | 1579.51 (31%) | 886.25 (53%) |
| cgi | - | 1.69 (0%) |
| email | 2.37 (0%) | 2.35 (0%) |
| emmett-core | - | 2391.56 (142%) |

### worstcase_bchar

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 4223.66 (100%) | 1461.62 (100%) |
| kirimi | 10514.55 (249%) | 2147.65 (147%) |
| python-multipart | 2532.60 (60%) | 1042.79 (71%) |
| werkzeug | 1808.36 (43%) | 992.51 (68%) |
| django | - | 958.56 (66%) |
| streaming-form-data | 29.75 (1%) | 29.18 (2%) |
| cgi | - | 69.01 (5%) |
| email | 49.35 (1%) | 47.76 (3%) |
| emmett-core | - | 1373.61 (94%) |

### worstcase_junk

| Parser | Non-blocking (MB/s) | Blocking (MB/s) |
|---|---:|---:|
| multipart | 4147.54 (100%) | 3899.30 (100%) |
| kirimi | 18067.19 (436%) | 18802.76 (482%) |
| python-multipart | failed | failed |
| werkzeug | 49.10 (1%) | 48.94 (1%) |
| django | - | failed |
| streaming-form-data | failed | failed |
| cgi | - | 1120.13 (29%) |
| email | 114.57 (3%) | 114.53 (3%) |
| emmett-core | - | 19177.70 (492%) |
