# Note
AI: This is a high level document so details should be included up to a certain level, we should 
go as low as needed to explain the approach but not lower, for example specific variables or functions 
are not needed, except for eBPF since each  map/program is part of the architecture.

Through this work we aim to improve Scaphandre to make its power/energy measurements more accurate 
and more correct. We scope our work only for improving the CPU power attribution of the tool, due to 
the complexity of Memory power attribution as discussed the MENTRION paper and the limited scope of the 
Master's thesis.

The current model is flawed due to two main factors which our model aims to address. The current model 
is blind to effects caused by changes if frequency (DVFS), and not able to distinguish workloads running in parallel 
with the help of HyperThreading/SMT from those running separately.

The Scaphandre architecture aims to be decoupled and to allow easy integration. It has two parts 
exporters and sensors. Exporters define the outputs of the tool, meaning where does it report the power 
measurements it tracks for processes. Sensors function as inputs to the tool, meaning where does it 
get information to use for power measurement from.

The tool achieves this decoupling through pre-defined interfaces which those wishing to modify the 
tool should adhere to and implement.

On the sensor side, the `Sensor` trait requires two methods: `get_topology()` to retrieve the current topology and `generate_topology()` to discover and build it from hardware sources. Additionally, the `RecordGenerator` trait (implemented by `Topology`, `CPUSocket`, and `Domain`) provides `refresh_record()`, `get_records_passive()`, and `clean_old_records()` for periodic energy counter updates, while `RecordReader` — also implemented per-component — offers `read_record()` for direct counter reads.

On the exporter side, the `Exporter` trait requires `run()` — the main loop that drives the measurement cycle — and `kind()` which returns the exporter's identifier string (e.g. "stdout", "prometheus"). A `MetricGenerator` wrapper struct holds a `Topology` instance and provides `gen_all_metrics()` to transform raw topology and process data into `Metric` structs, and `pop_metrics()` to drain them for output.

The exporters are responsible for polling the sensors for new information. The update cycle works as follows: in each iteration, the exporter calls `topology.refresh()`, which drains eBPF ring buffer events, reads RAPL energy counters from sysfs (or MSR), polls `/proc/stat` for per-core stats, refreshes process tracking data, and reads the eBPF CPU_TIME map for per-CPU busy time. The exporter then calls `metric_generator.gen_all_metrics()` to convert the refreshed state into `Metric` structs, and finally `pop_metrics()` to drain them for emission through the exporter's output channel.

However regardless of the sensor from which the tool gathers its metrics or the exporter used, the 
tool uses a unified method for power attribution.
This is why this work mainly focuses on updating the power attribution with some modifications to 
the powercap_rapl sensor to get some extra inputs.

The current solution only focuses on Linux support due to the reliance on eBPF which is not as much 
supported on Windows.

Due to the reliance on the RAPL sensor, the only two hardware platforms supported are AMD and Intel. 
ARM processors are not supported.

# Novelty
We propose a new solution to power attribution by suggesting to calculate first attribute power to 
cores then distribute it among running processes. This approach would allow to simplify the attribution 
logic and better reflect and reason about the power which would help achieve better models since 
the solution is better tied to the hardware.

We also improve upon the literature by estimating idle and activation power in real-time without 
relying on a pre-calibration phase where special workloads are run, or the tool requires a specific 
idle period.


# Scaphandre power attribution pipeline

The data flow proceeds through three layers connected by the trait interfaces:

1. **Sensor layer**: The sensor (e.g. `PowercapRAPLSensor`) implements the `Sensor` trait and builds a `Topology` containing `CPUSocket`, `Domain`, and `CPUCore` objects via `generate_topology()`. Each component implements `RecordReader` to read hardware energy counters from RAPL sysfs.

2. **Power attribution layer**: `Topology::refresh()` orchestrates all updates: it drains eBPF events for idle/activation detection, reads RAPL energy counters via the `RecordReader` implementations, reads `/proc/stat` and perf counters for core activity metrics, and computes per-core and per-process power using the attribution formula. The `RecordGenerator` trait on `Topology`, `CPUSocket`, and `Domain` provides the standardised `refresh_record()` interface for periodic counter updates.

3. **Exporter layer**: An exporter implements the `Exporter` trait and wraps a `MetricGenerator` holding the `Topology`. In its `run()` loop, it calls `topology.refresh()` to advance the measurement cycle, then `gen_all_metrics()` to create `Metric` objects, and `pop_metrics()` to drain them for output.

A diagram of this pipeline is provided in the final architecture document.


# Architecture
The current tool architecture has 4 parts, the Sensor part which is responsible for retrieving information 
about the system and creating a topology of it that is used for the power attribution in order to get relevant information,
the Exporter part which is meant for outputting power measurements and interacting with external systems, The 
eBPF part which is meant to gather more information about the system and about processes to support 
finer-grained attribution, The power attribution part which is the core that maps sensor power measurements 
to individual process works, the core entity that manages this process is Topology, this is wrapped 
and controlled by a MetricGenerator which continuously polls it for updates.

A Mermaid diagram illustrating this four-part architecture is provided in the final architecture document.

## Sensor

The sensor is responsible for discovering the hardware topology and providing access to energy measurement counters. It scans system interfaces — primarily the `powercap` sysfs hierarchy at `/sys/class/powercap/` and performance counters via `perf_event_open` — to enumerate CPU sockets, their RAPL domains (core, package, DRAM, PSYS), and the logical cores belonging to each socket. The resulting `Topology` structure serves as the central representation of the system that both the power attribution logic and exporters operate on. The `PowercapRAPLSensor` is the primary implementation on Linux and is responsible for detecting the `intel_rapl` kernel module, discovering domain folders, and optionally supporting MMIO-based RAPL access on newer platforms.

## Power attribution
The current power attribution model aims to address SMT, DVFS, and contention while also keeping a 
low overhead profile.

The model attributes power measured using RAPL first to cores and then it distributes core power 
between processes based on utilization.

Since our work mainly focuses on CPU power attribution our current implementation changes Scaphandre 
implementation by moving away from total host power to Core domain power falling back to package power 
in case the domain is not available.

After reading the input power (core/package domain), it first subtracts background power 
which is activation power by default and falls back to idle power in case the activation power 
is not yet estimated. It then attributes active power to cores first. We also fixed the wraparound 
issue described by the RAPL in action paper.

The current method relies on proportional power attribution where the domain power is attributed 
to cores based on their work, which is determined by a coefficient that is calculated using the 
formula:

(1 + IPC) * APERF * APERF / MPERF

UCC (Unhalted Core Cycle) is the PMU that increments when the core is in C0, and increments at the 
rate that the core's clock is running in meaning for each clock tick it increments by 1, and it 
increments faster when the core is in higher frequency.

MPERF increments at the base frequency when the core is in C0.
APERF increments at the actual frequency the core is running in when in C0. Unlike UCC APERF is 
not a register that takes and shows raw input, it also factors in thermal effects, throttling, 
thus being a better mediator of the actual performance the core is at.

APERF / MPERF is a standard ratio/metric and is recommended by Intel, it mediates how fast the core 
is running, meaning that a high value for the ratio implies the core is running faster.

IPC (Instructions Per Cycle) is calculated as INST_RETIRED / UCC (Unhalted Core Cycles). 
This value mediates how much effective work the core is doing in terms of instructions being retired per cycle. 
The interesting case about this is that it naturally helps differentiate work being done when a 
parallel CPU is running, since this gets lower compared to running in isolation, this is because 
how HyperThreading works is that it shares execution units between two threads which means that 
the throughput - in terms of instructions - gets lower due to contention between threads and thus 
lowering the IPC ratio, which is due to the fact that UCC is the same regardless of parallel or 
isolated execution since it has more to do with the cycles being used by the core which is unaffected 
by parallel or isolated execution. In an isolated scenario a given thread has all the execution 
units to itself, which would allow it to have higher throughput due to lower contention.

The reason for the combination is:
APERF / MPERF: as discussed is a standard mediator of how fast the core was running. But the issue 
is, since both only increment when the core is in C0 APERF / MPERF proxy how fast the core was 
running when in C0. This means that this ratio is blind to the case where two cores running at the 
same frequency run for significantly different lengths of time. The ratio would assign both the same 
value. To simplify, while it can tell you "how fast the core was running", it cannot tell "for how long".

To get insights into the second question we used APERF, since a core that was active for much of the 
sampling interval, would have a higher value than one that only ran for a fraction, because it 
spent more time in C0. Metrion uses UCC which is also a good mediator. We chose to avoid using 
UCC to avoid errors due to multiplexing since UCC is a PMU while APERF is a dedicated continuously running 
register.

Now the coefficient APERF (or UCC) * APERF / MPERF would give us the effective performance the 
core had, but wouldn't allow us to know what the core did with this performance.

This is specially relevant in the test scenario proposed by the Investigating Kepler paper by Bellal et al. 
Since two cores running at the exact same frequency for the exact same time period (for example 
both having c-states disabled) would get the same coefficient while one is actually doing computation 
and the other is just consuming frequency/stalled.

This is why we chose to have IPC which would mediate "what the core is doing". The reason for the 
(1 + IPC) term is that for the second core in the previous example which would have no instructions 
executing IPC would be 0 because INST is 0, which would imply the core is consuming no power/doing no 
work which is not correct.

This formula improves upon the one proposed by METRION by removing the need for hard-coded machine 
specific constants, using the more hardware grounded IPC value.

After calculating the per core power the process power is calculated by first proportionally dividing 
the core power among processes that ran on it based on CPU time given us for a process its 
per-core power for that interval. To get the process power for an interval, the per-core power is 
then summed up to get total process power.

While the current approach doesn't have the level of granularity METRION has, we aimed to construct 
a simpler model at the start that is easy to optimize and to get correct. Since actual frequency 
shifts, SMT, Contention and C-state effects happen at the CPU level, the current model is much easier 
to validate and to reason about and to improve. This model is also an improvement in terms of 
real-time reporting ability and the ability to support SMT without preset constants like the 
previous method does. We also don't lose much accuracy since all of our validation setups have 
the process running in isolation with as low of a noise on the core as possible, which means 
that Process power is roughly equal to Core power, thus we are effectively measuring process power 
directly and validating the final-like attribution method that would attribute power to processes 
using the same methodology but at a higher level (core rather than process).

In the codebase, the attribution is implemented as follows. `CPUSocket::get_core_coefs()` computes a per-core coefficient using the formula:

```
coefficient_i = (1 + IPC_i) * APERF_i * (APERF_i / MPERF_i)
```

The coefficients are then normalised into proportions by `get_core_proportions()` so they sum to 1.0 across all cores on the socket. `get_proportional_core_diff_power_microwatts()` distributes the socket's active power to each core proportionally:

```
core_power_i = proportion_i * socket_active_power
```

For process-level attribution, the eBPF `PID_TIMES` map provides per-process per-CPU busy time. `Topology::get_process_core_time_data(pid)` reads these values together with total per-CPU busy time from the `CPU_TIME` map, and computes `proportion_i = process_time_i / total_time_i` for each logical CPU. The process power for a core is then `core_power_i * proportion_i`, and total process power is the sum across all cores. When eBPF data is unavailable, the system falls back to OS-reported CPU time percentages from `/proc/stat`.

## eBPF
Linux doesn't record the per-CPU time but rather records general CPU time.
This was the initial motivation for opting into eBPF as it allows for a much accurate and fair 
model.

The mechanism of per-CPU time tracking is achieved through two programs one attached to `sched_switch` 
events and another attached to a timer that ticks every 10ms. The first aims to track CPU time 
at the context switch level. The second is for tracking long running processes that don't switch 
very often.

The eBPF subsystem uses seven maps:

- **`PID_TIMES`** (PerCpuHashMap<u32, u64>) — accumulates busy time in nanoseconds per process (TGID), per logical CPU. Keyed by TGID, value accumulates delta each time a process is switched on or off a CPU.
- **`PID_LAST`** (PerCpuHashMap<u32, u64>) — stores the last timestamp when a process was switched onto a CPU, used to compute elapsed time on context switch.
- **`TID_TO_TGID`** (HashMap<u32, u32>) — maps thread IDs (TID) to their process group ID (TGID), seeded on every context switch to allow per-process aggregation.
- **`CPU_TIME`** (Array<u64>) — per-logical-CPU accumulator of non-idle busy time in nanoseconds, updated by both the sched_switch and timer programs.
- **`CPU_SNAPSHOT`** (Array<u64>) — snapshot of `CPU_TIME` taken by `cpu_state_tick` to compute per-CPU deltas for idle/activation detection.
- **`CPU_STATE_EVENTS`** (RingBuf) — ring buffer that carries `CpuStateEvent` structs (socket_id + event_type) from kernel to userspace when a socket is detected fully idle or with exactly one active core.
- **`CPU_TO_SOCKET`** (Array<u16>) — maps each logical CPU index to its physical socket ID (plus one offset, so zero signals end of available CPUs). Populated by userspace at startup from `/sys/devices/system/cpu/*/topology/physical_package_id`.

We also specifically handle one failure mode. Since eBPF maps are naturally limited in size we specifically 
handle the case where the system is in high load and a large number of processes get created while 
old entries are stored in the map by deleting entries once a process (TGID) exits. This also handles 
an important error case, since Linux reassigns old unassigned PIDs, it might be the case that a 
process start incrementting after an old process has already exited, assigning it work it didn't 
do.

But because eBPF allows observing the system at a finer granularity it is used as the mechanism for 
detecting idle and activation windows in order to measure idle and activation power.

The reason for choosing eBPF for such task is that while idle power can be simply detected by tracking 
low utilization windows and recording the power measurement for them lowering the power 
at each measurement to get closer to idle power. Activation power is more tricky because it would 
require exactly one-core to be active while all being idle. The only way to observe it is by 
having finer granularity but a userspace loop would consume too much resources.

The method for calculating activation/idle power is a cooperation between userspace and kernelspace 
eBPF tracks CPU_TIME changes for all CPUs on the system by keeping a snapshot of the values 
and comparing the last snapshot with the latest CPU_TIME values, if a CPU's diff is 0 then that 
core was idle, if all cores were idle then the whole socket was idle and an idle event 
is emitted to a RING BUFFER, if only one was active then an activation event is emitted to the ring 
buffer.

The idle/activation detection is organised per socket. The `CPU_TO_SOCKET` eBPF array maps each logical CPU to its physical socket, populated at startup by reading `physical_package_id` from sysfs. The `cpu_state_tick` program is attached to only one CPU per socket (the first logical CPU of each package). When it fires, it iterates all CPUs, uses `CPU_TO_SOCKET` to group them by socket, and counts how many CPUs in each socket had non-zero `CPU_TIME` delta since the last snapshot. If a socket has zero active CPUs an `IdleEvent` is pushed to the ring buffer for that socket; if exactly one CPU is active an `ActivationEvent` is pushed. On the userspace side, `Topology::refresh()` drains the ring buffer, groups events by socket ID, and passes them to each `CPUSocket`'s `refresh_activation_idle_records()` method which records the current host power as either an idle or activation power measurement.

To track activation/idle power a tick based handlers is attached to the first core in a socket, 
since idle/activation is relevant to a given socket, the program gets executed each 2ms.

The userspace code then reads these events from the ring buffer and if either activation or idle 
records are detected it records the current host power and compares it to historic values and takes 
the minimum. The power subtracted from the pure RAPL measurement is background power, which is 
the maximum between activation power and idle power, this is because with our current measurement 
approach it might be the case - tough unlikely - that activation power would be lower than idle power 
if the core wasn't detected idle using eBPF which might lead to activation power being lower 
depending on host measurement. The current measurement method however would eventually converge 
to true activation/idle power as the idle power gets lower, meaning activation power would also 
converge to relatively higher measurement. Since activation power incorporates also idle power 
we use as the power to subtract form raw RAPL measurement.

Because activation also includes idle power the maximum between the two values is used as the 
power to subtract from the power reading since activation should converge to a higher or equal value 
to idle.

A diagram of the eBPF programs and maps with their interactions is provided in the final architecture document.

## Exporter

Each exporter holds a `MetricGenerator` that wraps the shared `Topology`. In its `run()` loop, the exporter calls `topology.refresh()` to advance one measurement cycle — reading RAPL energy counters, draining eBPF events, refreshing process data, and computing power attribution. It then calls `metric_generator.gen_all_metrics()` which internally calls the various metric generation methods (`gen_self_metrics`, `gen_host_metrics`, `gen_socket_metrics`, `gen_system_metrics`, `gen_process_metrics`) to transform the refreshed `Topology` state into a flat vector of `Metric` structs. Finally, `pop_metrics()` drains these metrics, and the exporter serialises them to its output format (JSON, Prometheus, stdout, etc.). The cycle repeats on a configurable interval, typically every 1–2 seconds.

# Future Perspectives
- The current model uses per-CPU time to attribute CPU power to processes proportionally, we aim 
to improve it by incorporting actual work done by the process.
