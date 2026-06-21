# Per-Core Power Attribution Approach

This document explains how Scaphandre estimates the power consumption of each individual CPU core. The goal is to go beyond a single host-level power reading and say *"core 0 used X microwatts, core 1 used Y microwatts"* — without any hardware support for per-core power sensors.

---

## The Problem

RAPL (Running Average Power Limit) gives us the total power consumed by the whole CPU package, but not a breakdown per core. We need to figure out how to split that total among the individual cores in a meaningful way.

---

## Step 1 — Measure What Each Core Is Actually Doing

Every refresh cycle, for each core, we read three hardware counters from the CPU's MSRs (Model-Specific Registers):

| Counter | What it measures |
|---------|-----------------|
| **APERF** | Actual Performance — how many cycles the core ran at its real operating frequency |
| **MPERF** | Maximum Performance — how many cycles would have elapsed at the CPU's maximum frequency |
| **IPC**   | Instructions Per Cycle — how efficiently the core is executing (computed from perf event counters) |

From these we derive a **coefficient** for each core:

```
coef = (1 + IPC) × APERF × (APERF / MPERF)
```

Breaking this down:

- `APERF / MPERF` is the fraction of time the core was running at full speed (frequency ratio).
- Multiplying by `APERF` again weights cores that ran more cycles more heavily.
- `(1 + IPC)` boosts cores that were doing more useful work per cycle.

The result is a single number that captures both *how busy* a core was and *how efficiently* it was doing work. A core sitting idle gets a coefficient near 0; a core doing heavy, efficient computation gets a high coefficient.

We then compare coefficients between two consecutive measurements to get the **coefficient delta** (Δcoef) per core — positive if the core got busier, negative if it got less busy.

---

## Step 2 — Observe the Change in Total Host Power

We also track the host-level power reading (from RAPL) in a buffer. By subtracting the previous reading from the current one we get the **power delta** (Δpower) — positive if total power went up, negative if it went down.

---

## Step 3 — Attribute the Power Change to Individual Cores

We now have two things:
- A vector of signed coefficient deltas, one per core.
- A single signed power delta for the whole host.

The core assumption is: **each core's power change is proportional to its coefficient delta**.

Mathematically, the power change for core *i* is:

```
power_change_i = (Δcoef_i / |Δcoef_total|) × abs_power_delta_total
```

where `|Δcoef_total|` is the sum of the *absolute values* of all coefficient deltas, and `abs_power_delta_total` is the sum of the *magnitudes* of all per-core power changes (the unknown we want to solve for).

The sign of `Δcoef_i` is preserved, so a core whose coefficient went up gets a positive power change (it's using more power) and one whose coefficient went down gets a negative one.

### How do we find `abs_power_delta_total`?

We use the constraint that the individual changes must add up to the observed host power delta. Three situations arise:

---

### Case 1 — Power changed and net activity changed (`Δpower ≠ 0`, `net_coef_change ≠ 0`)

`net_coef_change` is the plain sum of all coefficient deltas (some positive, some negative). If this is non-zero, the system as a whole got busier or less busy.

We write the constraint:

```
sum_i(power_change_i) = Δpower
```

Substituting the proportional formula for each `power_change_i` and solving:

```
abs_power_delta_total = (|Δcoef_total| / net_coef_change) × Δpower
```

Once we have `abs_power_delta_total`, each core's power change follows directly.

---

### Case 2 — Power changed but net activity cancelled out (`Δpower ≠ 0`, `net_coef_change = 0`)

This happens when some cores got busier by exactly as much as others got less busy. The power shift is therefore driven by something other than net CPU activity (memory, I/O, uncore components, etc.). In this situation we have no signal to attribute the host power change to specific cores, so **we keep each core's power reading unchanged** and fall through to the zero-change path below.

---

### Case 3 — Power did not change (`Δpower = 0`)

Even when the host total is stable, individual cores can be trading power among themselves — one spins up while another idles down.

Here we use the constraint that the changes must sum to zero:

```
sum_i(power_change_i) = 0
```

We pick one core (call it the anchor, core 0) and estimate its power change using the running `coef_to_power` scale factor:

```
selected_power = coef_to_power × Δcoef_0
```

Substituting the proportional formula for all other cores into the zero-sum constraint and solving:

```
abs_power_delta_total = -selected_power × (|Δcoef_total| / sum_of_remaining_deltas)
```

The same proportional formula then gives each core's change.

---

## Step 4 — Apply the Change and Update the Estimate

Once we know `power_change_i` for each core, we simply add it to that core's previous power reading:

```
new_power_i = previous_power_i + power_change_i
```

We also maintain a running scale factor `coef_to_power`:

```
coef_to_power = (coef_to_power + abs_power_delta_total / |Δcoef_total|) / 2
```

This is just a running average of the ratio between power magnitude and coefficient magnitude. It is used as a fallback estimator when edge cases prevent a full solution (for example when all coefficient deltas are zero, or when the denominator in the zero-sum formula is zero).

---

## Bootstrap / Cold Start

On the very first measurement there is no previous per-core power reading to add a delta to. In that case we fall back to a simpler proportional method: each core's power is set to its share of the total host power, where the share is computed directly from its coefficient as a fraction of the sum of all coefficients.

---

## Summary

| Situation | What we do |
|-----------|-----------|
| First measurement | Assign each core a share of total host power proportional to its coefficient |
| Power changed, cores changed activity | Solve for the total magnitude of changes using the host delta as a constraint, then split it among cores by their coefficient delta magnitudes and signs |
| Power changed, but net activity cancelled | Keep per-core powers unchanged (the change is from non-core sources) |
| Power unchanged, cores shifted activity | Use the zero-sum constraint with the `coef_to_power` estimator to find how much cores traded power with each other |
| All coefficient deltas are zero | Keep the previous reading (nothing changed) |

The key insight is that we never need to know the absolute power of a core directly — we only need to track *changes*, and we have enough constraints (the observed host delta and the proportionality assumption) to solve for those changes at each step.
