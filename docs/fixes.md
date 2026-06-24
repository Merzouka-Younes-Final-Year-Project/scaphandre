# Fixes and Infrastructure Improvements

This file tracks correctness fixes and infrastructure improvements that are not part of the core attribution algorithm.

---

## RAPL counter wrap-around (`fix(rapl)`)

**Tag context:** between `v_pid` and `v_propo_preserving_delta_of_delta`

### Problem

The RAPL energy counter is a hardware register with a finite range. When it reaches its maximum value it wraps around to zero. The previous code detected this case by checking `last < previous` and silently discarding the sample (treating it as if no energy had been consumed). This caused a full measurement interval to be lost every time the counter wrapped.

### Fix

The kernel exposes the counter maximum via `max_energy_range_uj` in the powercap sysfs tree. This value is read at sensor initialisation and stored in `CPUSocket::rapl_max_uj` and `Domain::rapl_max_uj`.

When the counter appears to have decreased, the true delta is computed as:

```
microjoules = max_uj - previous_microjoules + last_microjoules
```

This correctly recovers the energy consumed across the wrap boundary instead of discarding it.

The fix applies to both the socket-level (`CPUSocket::get_records_diff_power_microwatts`) and domain-level (`Domain::get_records_diff_power_microwatts`) power calculations.

---

## eBPF: TID vs TGID confusion (`fix(ebpf)`)

**Tag context:** between `v_pid` and `v_propo_preserving_delta_of_delta`

### Problem

The eBPF `sched_switch` tracepoint receives the thread ID (TID) of the outgoing task (`prev_pid`), not the process ID (TGID/PID as seen from userspace). The userspace code keys per-process accounting by TGID. Using raw TIDs meant multi-threaded processes were split into many unrelated per-thread buckets rather than being aggregated under the process.

The `sample_tick` perf event had the same bug: it used the lower 32 bits of `pid_tgid` (which is TID) instead of the upper 32 bits (which is TGID).

### Fix

- A new kernel-side map `TID_TO_TGID` is maintained. When a task is scheduled in, its TID→TGID mapping is inserted using `bpf_get_current_pid_tgid()`.
- When a task is scheduled out, its accumulated CPU time is attributed to its TGID via a lookup in `TID_TO_TGID`, falling back to the TID if no entry exists.
- `sample_tick` was corrected to extract TGID from the upper 32 bits of `bpf_get_current_pid_tgid()`.

This makes eBPF-side per-process CPU time accounting consistent with what userspace sees.

### Consequence for background power calculation

Background power is derived from CPU idle/active state events emitted by the `cpu_state_tick` perf event. These events were being attached to every logical CPU, causing redundant per-socket aggregation (each socket's idle state was being counted once per core rather than once per socket).

The userspace loader was changed to attach `cpu_state_tick` only to the first physical CPU of each socket (by reading `/sys/devices/system/cpu/cpuN/topology/physical_package_id`). This ensures one idle/active measurement per socket, consistent with how RAPL reports socket-level power.

---

## CPU-domain scoping of power tracking (`feat(core): Scoped attribution to CPU only`)

**Tag context:** `v_propo_preserving_delta_of_delta`

Previously the delta-of-delta algorithm used the total host RAPL power (entire package) as the signal it was trying to attribute to cores. This introduced noise from DRAM, uncore, and other non-CPU components whose power changes have nothing to do with per-core computation.

All per-core attribution now uses the CPU RAPL sub-domain power only. The `get_records_diff_power_microwatts_per_domain` helper selects the CPU domain record if available, falling back to the full package reading only when no domain-specific record exists.

The per-core state (`core_coef_buffer`, `core_power_buffer`, `cpu_power_buffer`, `coef_to_power`) was also moved from `Topology` down to `CPUSocket`, where it logically belongs. Each socket maintains its own independent attribution state.
