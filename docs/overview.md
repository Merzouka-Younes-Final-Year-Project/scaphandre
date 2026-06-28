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

{MENTION SENSOR INTERFACES WITH A BRIEF EXPLANATION}
{MENTION EXPORTER INTERFACES WITH A BRIEF EXPLANATION}

The exporters are responsible for polling the sensors for new information using Topology::refresh. {CORRECT ME IF I AM WRONG, EXPLAIN HOW DOES UPDATING RESULTS WORK}

However regardless of the sensor from which the tool gathers its metrics or the exporter used, the 
tool uses a unified method for power attribution.
This is why this work mainly focuses on updating the power attribution with some modifications to 
the powercap_rapl sensor to get some extra inputs.

The current solution only focuses on Linux support due to the reliance on eBPF which is not as much 
supported on Windows.

Due to the reliance on the RAPL sensor, the only two hardware platforms supported are AMD and Intel. 
ARM processors are not supported.

# Scaphandre power attribution pipeline
{EXPLAIN HOW DOES DATA/INPUTS FLOW THROUGH SENSOR, POWER ATTRIBUTION MODULE AND EXPORTER AND WHERE 
THE VARIOUS INTERFACES SIT, JUST AT A HIGH LEVEL, MAKE SURE TO ADD A DIAGRAM WITH MEMRAID OR SOMETHING 
JUST SOMETHING I CAN EXPORT AS PNG LATER}


# Architecture
The current tool architecture has 4 parts, the Sensor part which is responsible for retrieving information 
about the system and creating a topology of it that is used for the power attribution in order to get relevant information,
the Exporter part which is meant for outputting power measurements and interacting with external systems, The 
eBPF part which is meant to gather more information about the system and about processes to support 
finer-grained attribution, The power attribution part which is the core that maps sensor power measurements 
to individual process works, the core entity that manages this process is Topology, this is wrapped 
and controlled by a MetricGenerator which continuously polls it for updates.

{GENERATE AN ILLUSTRATION USING MERMAID OR ANY TOOL THAT WOULD HAVE GOOD COMPREHENSIBLE VISUALS 
IF YOU NEED MCP LET ME KNOW I WOULD LOVE INPUTS ON ILLUSTRATION}

## Sensor
{EXPLAIN WHAT DOES THE SENSOR DO}

## Power attribution
The current power attribution model aims to address SMT, DVFS, and contention while also keeping a 
low overhead profile.

The model attributes power measured using RAPL first to cores and then it distributes core power 
between processes based on utilization.

Since our work mainly focuses on CPU power attribution our current implementation changes Scaphandre 
implementation by moving away from total host power to Core domain power falling back to package power 
in case the domain is not available.

After reading the input power (core/package domain) it attributes it to cores first.

The current method relies on proportional power attribution where the domain power is attributed 
to cores based on their work, which is determined by a coefficient that is calculated using the 
formula:

(1 + IPC) * APERF * APERF / MPERF

TODO: Explain rationale behind formula

This is similar to the formula used by METRION but since we are measuring these at the core level 
we benefit from lower overhead due to a single access as well as simpler access.

After calculating the per core power the process power is calculated by first proportionally dividing 
the core power among processes that ran on it based on CPU time given us for a process its 
per-core power for that interval. To get the process power for an interval, the per-core power is 
then summed up to get total process power.

While the current approach doesn't have the level of granularity METRION has, we aimed to construct 
a simpler model at the start that is easy to optimize and to get correct. Since actual frequency 
shits, SMT, Contention and C-state effects happen at the CPU level, the current model is much easier 
to validate and to reason about and to improve. This model is also an improvement in terms of 
real-time reporting ability and the ability to support SMT without preset constants like the 
previous method does. We also don't lose much accuracy since all of our validation setups have 
the process running in isolation with as low of a noise on the core as possible, which means 
that Process power is roughly equal to Core power, thus we are effectively measuring process power 
directly and validating the final-like attribution method that would attribute power to processes 
using the same methodology but at a more granular level.

{IMPROVE ON THIS WITH WHAT YOU HAVE ACCESS THROUGH CODE}

## eBPF
Linux doesn't record the per-CPU time but rather records general CPU time.
This was the initial motivation for opting into eBPF as it allows for a much accurate and fair 
model.

The mechanism of per-CPU time tracking is achieved through two programs one attached to `sched_switch` 
events and another attached to a timer that ticks every 10ms. The first aims to track CPU time 
at the context switch level. The second is for tracking long running processes that don't switch 
very often.

{ADD DETAILS ABOUT THE MAPS AND HOW THEY ARE USED}

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

{CLARIFY THAT THIS SETUP IS PER SOCKET AND HOW THE PER SOCKET HANDLING WORKS}

To track activation/idle power a tick based handlers is attached to the first core in a socket, 
since idle/activation is relevant to a given socket, the program gets executed each 2ms.

The userspace code then reads these events from the ring buffer and if either activation or idle 
records are detected it records the activation/idle power and compares it to historic values and takes 
the minimum.

Because activation also includes idle power the maximum between the two values is used as the 
power to subtract from the power reading since activation should converge to a higher or equal value 
to idle.

{ADD A DIGRAM OF THE ENTIRE eBPF SIDE WITH PROGRAMS AND MAPS}

## Exporter
{EXPLAIN HOW THE EXPORTER USES THE METRICGENERATOR TO POLL NEW METRICS AT A HIGH LEVEL}
