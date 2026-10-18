# xbabe2 CPU slices for the max-parallel-code phase (set by Grok Bot for Jepson, Oct 17 2026, 9:28 PM MDT)

xbabe2 has 128 logical CPUs (siblings are i and i+64) and 125 GiB of RAM. The allowed agent range is CPUs 0-50 and 64-114, which is 102 logical CPUs. At 03:30Z these were already pinned or reserved, so leave them alone: 0-5, 16-21, 32-43 (Joil workers plus the Joil lane pool 36-39), 68-77, 90-91 and 110-111 (Joil lane pool). CPUs 51-63 and 115-127 are outside the agent range. Don't use them.

Each lane uses 2 CPUs. That leaves about 64 free logical CPUs, enough for about 32 new lanes, gated by load and RAM.

| Project | Slice (logical CPUs) | Lane pairs |
|---|---|---|
| JopeDime | 22-31, 92-101 | 10 |
| DOPE | 78-89 | 6 |
| Jarambit | 6-15 | 5 |
| Joil | lease pool only: 36-37, 38-39, 90-91, 110-111 (renewal-002; the lease caps Joil at 12 cores / 36 GiB / 15 GiB scratch) | 4 |
| SHARED overflow | 44-50, 64-67, 102-109, 112-114 | about 10 |

Joil may NOT use overflow CPUs. Its lease cap is binding until Jepson raises it.

Launch guard, checked before every launch: the 1-minute load must be at or below 85 and MemAvailable at or above 25 GB. For Joil, its lease's stricter guard (load at or below 70, MemAvailable at or above 40 GiB) still applies. Stagger launches about 60 seconds apart.

## Overflow claims (append one line per 2-CPU pair BEFORE using it: `<cpus> <project> <screen> <UTC>`)
44,45 DOPE dope_lane_w02 2026-10-18T03:43:46Z
102,103 JopeDime jope_i06_codex 2026-10-18T03:44:30Z
104,105 JopeDime jope_i10_codex 2026-10-18T03:44:30Z
106,107 JopeDime jope_i12_codex 2026-10-18T03:44:30Z
108,109 JopeDime jope_i13_codex 2026-10-18T03:44:30Z
46,47 DOPE dope_lane_c04 2026-10-18T03:44:50Z
48,49 DOPE dope_lane_f01 2026-10-18T03:45:54Z
64,65 DOPE dope_lane_m00 2026-10-18T03:46:58Z
112,113 JopeDime jope_i07_codex 2026-10-18T04:01:30Z
66,67 DOPE dope_lane_u03 2026-10-18T04:04:23Z
112,113 JopeDime jope_i07_codex RELEASED 2026-10-18T04:07:45Z; CPU 112 occupied by another workload, no test launched
112,113 JopeDime jope_i07_codex 2026-10-18T04:14:10Z
112,113 JopeDime jope_i07_codex RELEASED 2026-10-18T04:16:00Z; captain directive assigns CPUs 18,19; 112,113 not used
