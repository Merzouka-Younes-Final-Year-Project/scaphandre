# Per-Core Power Attribution Approach

This document explains how Scaphandre estimates the power consumption of each individual CPU core. The goal is to go beyond a single host-level power reading and say *"core 0 used X microwatts, core 1 used Y microwatts"* — without any hardware support for per-core power sensors.

This document records two iterations of the delta-based method:

- `v_delta_of_delta`: the original approach.
- `v_corrected_delta_of_delta`: the current approach.

Before those two versions, there was a simpler absolute proportional allocation method. It is included below only as motivation for why the delta-based model was introduced.

---

## The Problem

RAPL (Running Average Power Limit) gives us the total power consumed by the whole CPU package, but not a breakdown per core. We need to figure out how to split that total among the individual cores in a meaningful way.

---

## Motivation: Why Absolute Proportional Allocation Was Rejected

The earlier baseline approach attributed host power directly from the current coefficients:

```
power_i = (coef_i / sum_j(coef_j)) × host_power
```

At first glance this looks reasonable: a core with a larger coefficient gets a larger share of the observed host power.

The problem is that this makes cores affect each other even when one core's own workload did not change.

### Experimental setup that exposed the flaw

The test setup in `../tests/test.sh` isolates one workload core running a stable `stress-ng` load, then later wakes up additional cores by disabling their C-states and fixing their frequency.

The expectation for the original workload core was simple:

- its own workload stayed stable,
- its own coefficient stayed roughly stable,
- so its attributed power should also stay roughly stable.

What actually happened with the proportional model was different: once the additional cores became non-idle, the attributed power of the original workload core dipped.

### Why the dip happens mathematically

Suppose one core has coefficient `c`.

Before the other cores are activated:

- total coefficient sum is `s`,
- host power is `r`,
- attributed power for that core is:

  ```
  (c / s) × r
  ```

After the other cores become active:

- total coefficient sum becomes `s'`,
- host power becomes `r'`,
- attributed power becomes:

  ```
  (c / s') × r'
  ```

For the first core's power to remain unchanged, the proportional model requires:

```
(c / s') × r' = (c / s) × r
```

which implies:

```
r' = (s' / s) × r
```

and equivalently:

```
r' - r = ((s' - s) / s) × r
```

This is exactly the relationship checked by `../tests/scripts/expected.py`.

### What this means in practice

The proportional model is only correct if the increase in host power scales strongly enough to cancel the dilution caused by the larger denominator `s'`.

In the observed measurements, that did not happen. The added host power was smaller than the proportional model required. As a result, the unchanged workload core lost attributed power simply because other cores entered the denominator.

That is the core flaw of absolute proportional allocation: it couples cores through the instantaneous coefficient sum and can move a stable core's power even when that core did not meaningfully change its own work.

### Why this led to delta-of-delta

The motivation for `v_delta_of_delta` was to stop attributing absolute host power from scratch at every sample.

Instead, the idea was:

- keep the previous per-core power state,
- look only at changes in per-core work,
- attribute the observed host power change to those work changes.

Under that reasoning, if the original workload core stays roughly stable, its coefficient delta should stay small, while the extra work caused by newly non-idle cores should receive most of the observed host power delta.

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

---

## Version Notes

### v_delta_of_delta

This is the original version of the model documented above.

It inferred per-core power changes from coefficient changes and the measured host-level power delta, but it had a failure mode when the net coefficient change was small relative to the individual coefficient deltas. In that case, the computed energy changes could become disproportionately large, because the host delta was effectively amplified through the ratio used to distribute power across cores.

The practical symptom was that a modest real-world change at host level could produce an unrealistically large per-core energy jump.

### v_corrected_delta_of_delta

The current implementation keeps the same basic delta-based attribution, but adds a correction step so the per-core changes are grounded back against the measured host-level power delta. That makes the attribution more consistent with the observed measurement and avoids the large-energy-amplification problem from `v_delta_of_delta`.

In other words, the new version fixes the original issue where a smaller net coefficient could cause a larger-than-realistic energy change.

The remaining flaw is that the model is still stateful in a way that can get stuck:

- a large host-level spike can push the core power estimate up sharply,
- later measurements may not provide a strong enough corrective signal,
- and the model can keep returning the inflated absolute core powers instead of converging back down.

So `v_corrected_delta_of_delta` improves the scaling, but it does not yet make the absolute estimate self-correcting over time.

### How the corrected version anchors to host-level power

The corrected version still starts from the same two signals:

- the signed coefficient change for each core,
- the signed host-level power delta for the whole machine.

The host delta is used as the external reference that fixes the scale of the per-core attribution.

#### Main delta path

When the host power changes and the coefficient deltas do not cancel out, the algorithm does this:

1. Compute the total signed coefficient change:

   ```
   net_coef_change = sum_i(Δcoef_i)
   ```

2. Compute the total absolute coefficient movement:

   ```
   abs_coef_total = sum_i(|Δcoef_i|)
   ```

3. Use the host delta to solve for a total core-power-change magnitude:

   ```
   abs_power_delta_total = (abs_coef_total / net_coef_change) × Δpower
   ```

4. Distribute that total across cores proportionally to their signed coefficient deltas:

   ```
   raw_change_i = (Δcoef_i / abs_coef_total) × abs_power_delta_total
   ```

5. Re-ground the result against the measured host delta:

   ```
   estimated_delta_power = (abs_power_delta_total / abs_coef_total) × net_coef_change
   corrected_change_i = raw_change_i × Δpower / estimated_delta_power
   ```

This last step is the anchoring step. It says the host measurement is the scale reference, and the per-core changes must be normalized so that reference stays consistent.

#### Anchor fallback path

If the host delta is zero, or if the coefficient deltas cancel out, the code falls back to an anchor-based estimate.

In that path:

1. The first core’s coefficient delta is taken as the anchor.
2. The running `coef_to_power` factor is used to estimate that core’s power change:

   ```
   selected_power = coef_to_power × Δcoef_0
   ```

3. The remaining coefficient deltas are used to solve the zero-sum or cancelled-signal case.
4. The resulting vector is again re-scaled so the anchor stays consistent with the inferred total change.

This means the fallback path is not anchored to the current host delta directly; it is anchored to the historical `coef_to_power` estimate, which acts as the local reference when the host signal is not usable.

#### What the anchoring achieves

Anchoring prevents the attribution from being driven only by coefficient ratios. The host-level delta constrains the absolute size of the update, so the core vector remains tied to the observed machine-level change rather than floating freely.

The remaining limitation is that this anchoring constrains a single update, but it does not automatically force the accumulated absolute per-core state to relax when later samples are weak or ambiguous. That is why a large spike can still leave the model stuck at an inflated value.
