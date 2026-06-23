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

---

### v_diff_pid — Current approach

The two previous versions accumulated absolute per-core power estimates and applied corrections on top of them. The structural problem with that design was that any large spike would inflate the absolute state, and later measurements could not force it back down — the correction was applied to the current update, not to the running total.

`v_diff_pid` discards the multi-stage correction pipeline entirely and replaces it with two well-defined stages:

1. **Diff-based nudge** — distribute the change in host power across cores by each core's share of total coefficient movement.
2. **Scalar PID** — correct the aggregate residual between the nudged sum and the measured host power, distributing the correction by current activity share.

The integral term in the PID loop is what makes the absolute estimate self-correcting: if the sum of per-core estimates persistently deviates from the measured host power, the integral accumulates and drives it back. That is the structural fix for the "stuck at an inflated value" problem from `v_corrected_delta_of_delta`.

---

#### Stage 1 — Diff-based nudge

At each sample, for each core *i*, we compute:

```
weight_i = Δcoef_i / sum_j(|Δcoef_j|)
nudged_power_i = prev_power_i + weight_i × host_diff
```

where `host_diff = host_power_t − host_power_{t−1}` is the signed change in total host power, and `Δcoef_i = coef_i_t − coef_i_{t−1}` is the signed change in that core's coefficient.

The weights are signed. A core whose coefficient increased gets a positive weight and is nudged up; one whose coefficient decreased gets a negative weight and is nudged down. A core with a flat coefficient gets a weight near zero and its power estimate barely moves, regardless of what other cores do.

The equal-split fallback (`weight_i = 1/n`) only applies when the total coefficient churn is essentially zero, meaning no core changed its activity between samples.

This is the principal fix for the cross-core leakage problem from earlier versions. Because the nudge is gated on each core's own Δcoef, a stable core is not dragged by the transients of active cores.

**This stage does not enforce `Σ nudged_power_i == host_power`.** That is intentional. The constraint is enforced gradually by Stage 2, not by a hard rescale at every cycle.

---

#### Stage 2 — Scalar PID residual correction

After the nudge, the sum of per-core estimates will generally differ from the measured host power. The residual is:

```
residual = host_power − sum_i(nudged_power_i)
```

A standard PID loop accumulates this residual and computes a scalar correction:

```
integral += residual × dt           (clamped to ±PID_INTEGRAL_CLAMP)
derivative = (residual − last_residual) / dt
correction = KP × residual + KI × integral + KD × derivative
```

Current gain values (module-level constants, easy to retune):

| Constant | Value |
|---|---|
| `PID_KP` | `0.4` |
| `PID_KI` | `0.05` |
| `PID_KD` | `0.0` |
| `PID_INTEGRAL_CLAMP` | `50 000 µW` |

`KD` is zero by default. Derivative on noisy per-sample residuals injects jitter into all cores simultaneously; it should only be enabled (with a smoothing EMA on the residual) if P+I alone produce sustained oscillation.

The scalar correction is distributed across cores by **current activity share** — each core's coefficient as a fraction of the total:

```
corrected_power_i = nudged_power_i + correction × proportion_i
```

where `proportion_i = coef_i / sum_j(coef_j)`.

Using current proportions (not Δcoef proportions) means the correction lands on whichever cores are doing work right now, regardless of whether their activity just changed.

The integral term is what makes this model self-correcting over time. If the nudged sum is persistently below the host power (positive residual), the integral grows, the correction grows, and the per-core estimates rise until the residual closes. The proportional term responds immediately to each cycle's residual; the integral term eliminates steady-state bias over multiple cycles.

---

#### Anti-windup: integral reset on core transitions

Core activation and deactivation events (a core going from zero to non-zero activity, or vice versa) are detected before the PID step:

```
transition detected if:  (prev_power ≤ 1.0 AND current_coef > 0)   ← activation
                      OR  (prev_power > 1.0  AND current_coef ≤ 1e-9) ← deactivation
```

When a transition is detected, `core_pid_integral` is reset to zero before the correction is applied.

Without this reset, integral bias accumulated during a steady state (e.g. 60 seconds of a stable workload) would be carried into the next phase and immediately over-correct the estimate at the moment of the transition. The reset discards that stale bias so the PID loop starts clean from the new operating point.

---

#### Bootstrap (cold start)

On the very first sample, `core_power_buffer` is empty and there is no previous power state to nudge from. In that case the estimate is seeded with an even split:

```
initial_power_i = host_power / n
```

No PID correction is applied on the first sample. The loop begins correcting from the second sample onward.

---

#### dt: actual elapsed time

The PID integral term requires a time step `dt`. This is computed from the timestamps of the last two entries in `power_buffer`:

```
dt = (timestamp_t − timestamp_{t-1}).max(1ms)
```

Using real elapsed time rather than an assumed constant keeps the integral units consistent (µW·s accumulates to µJ) regardless of whether the sampling cadence is uniform.

---

#### Output: non-negativity floor, no rescale

After the PID correction, a floor is applied:

```
final_power_i = max(corrected_power_i, 0.0)
```

There is no rescale-to-sum step. Forcing `Σ final_power_i == host_power` at every cycle would zero out the residual before the integral term could accumulate it, defeating the purpose of having an integral term. The PID loop converges `Σ power_i` toward `host_power` over a few cycles; it does not need to be exact at every individual sample.

---

#### Summary of the new design

| Stage | Input | Output | Purpose |
|---|---|---|---|
| Diff nudge | `prev_powers`, `Δcoef`, `host_diff` | `nudged_powers` | Track each core's own activity change without coupling through a shared denominator |
| Scalar PID | `nudged_powers`, `host_power`, `proportions`, `dt` | `corrected_powers` | Drive `Σ power_i → host_power` over time; integral eliminates steady-state bias |
| Floor | `corrected_powers` | `final_powers` | Prevent negative power values |

The PID state is two scalars (`core_pid_integral`, `last_residual`), not a per-core vector. All the complexity of the old six-stage pipeline is replaced by these two well-understood stages with five tunable constants.

---

### v_host_constrained_observer — Current approach

`v_corrected_delta_of_delta` fixed the scaling problem of the original version but left one structural limitation: the correction step operated on a single update and had no mechanism to pull the accumulated absolute state back toward reality when later samples were weak. A large spike could leave the running per-core totals inflated indefinitely.

`v_host_constrained_observer` addresses this by restructuring the computation as a **predict–correct observer loop**, a pattern used in control systems such as drone attitude estimators. In those systems, an inertial predictor (fast, noisy) is continuously corrected by a slower, more reliable measurement. Here:

- the **predictor** uses each core's own coefficient evolution to advance its power estimate from the previous state,
- the **corrector** uses the host-level RAPL measurement as the reliable external reference that pulls the vector back toward the true total.

The per-core state is never trusted to drift unchecked: every cycle it is predicted forward, then corrected against the measurement.

---

#### Five-stage pipeline

Each call to `read_core_powers_record` runs five stages in sequence:

```
predict → build_reference → update_uncertainty → correct → blend → normalize
```

---

#### Stage 1 — Predict (`predict_core_powers`)

The predictor advances each core's power estimate using the ratio of its current coefficient to its previous coefficient:

```
ratio_i = clamp(coef_i_t / coef_i_{t-1}, 0.25, 4.0)
predicted_i = prev_power_i × ratio_i ^ RATIO_GAMMA        (RATIO_GAMMA = 0.75)
```

The ratio captures whether the core got busier or less busy. Raising it to `RATIO_GAMMA < 1` applies a damping: a core that doubled its coefficient does not get double the power immediately — the exponent moderates the response, preventing the predictor from overreacting to a single noisy sample.

The ratio is clamped to `[0.25, 4.0]` to guard against division-by-near-zero when a coefficient is very small, and to prevent runaway predictions when a coefficient spikes.

For cores that were previously idle (`prev_coef ≈ 0`) but now have a non-zero coefficient, the predictor seeds them using the global `coef_to_power` scale:

```
predicted_i = NEW_CORE_GAIN × coef_to_power × coef_i_t      (NEW_CORE_GAIN = 0.2)
```

The conservative gain (0.2) avoids over-committing to a newly active core before any correction has been applied.

`coef_to_power` itself is a running EMA of `total_power / total_coef` from the last accepted final state, updated at the end of each cycle.

---

#### Stage 2 — Build reference (`build_reference_core_powers`)

In parallel with the predictor, a reference allocation is computed from scratch using only the current coefficients and the measured host power:

```
weight_i = coef_i ^ GAMMA          (GAMMA = 0.5)
reference_i = host_power × (weight_i / sum_j(weight_j))
```

Using `coef^0.5` (square root) rather than `coef` directly compresses the dynamic range: a core with a very high coefficient is not given a proportionally outsized share. This makes the reference more conservative than a direct proportional split and reduces the pull toward overcommitting to active cores.

The reference is **not used as the final answer**. It is the measurement in the observer sense — the slowly-varying, host-anchored signal that keeps the predictor from drifting.

---

#### Stage 3 — Update per-core uncertainty (`update_core_uncertainty`)

Each core carries an `uncertainty` value in `[0.05, 1.0]` that tracks how much its current estimate should be distrusted. Higher uncertainty means the corrector and blender will act more aggressively on that core.

The uncertainty for each core is updated every cycle as an EMA (decay = 0.4) of a freshly computed score:

```
fresh_i = MIN_UNCERTAINTY
        + 0.8 × change_share_i      ← core's share of total coefficient movement
        + 0.4 × jump_score_i        ← |predicted - prev| / host_power, clamped to 1
        + 0.35 × activation_score_i ← 0.35 if core just activated, else 0
        + 0.3 × mismatch_score_i    ← |predicted - reference| / host_power, only if already uncertain

uncertainty_i = 0.4 × prev_uncertainty_i + 0.6 × fresh_i
```

The logic:
- A core whose coefficient is changing a lot (`change_share` high) gets higher uncertainty — its predictor trajectory is less trustworthy.
- A core whose prediction jumped far from its previous value (`jump_score` high) gets higher uncertainty — the predictor may have overreacted.
- A core that just woke from idle (`activation_score`) gets elevated uncertainty immediately — the seeded value is a guess.
- A core that was already uncertain *and* whose prediction disagrees with the reference (`mismatch_score`) gets an additional push toward higher uncertainty.

Cores that have been stable for several cycles decay toward `MIN_UNCERTAINTY = 0.05`, meaning the corrector barely touches them.

---

#### Stage 4 — Apply host-residual correction (`apply_host_residual_correction`)

The residual between the predicted sum and the measured host power is:

```
residual = host_power − sum_i(predicted_i)
```

This residual must be redistributed across cores. The distribution uses a weighted score per core that combines three signals, with the sign of the residual selecting which direction of each signal to use:

| Signal | Weight | Meaning |
|---|---|---|
| `coef_score` | 0.45 | Core's share of rising (residual > 0) or falling (residual < 0) coefficient movement |
| `delta_score` | 0.35 | Core's share of predicted-power movement in the direction opposing the residual |
| `transition_score` | 0.15 | Whether the core just activated (residual > 0) or deactivated (residual < 0) |
| `uncertainty` | 0.05 | Baseline: higher-uncertainty cores absorb a small share unconditionally |

The weighted scores are normalised to sum to 1, then used to distribute the residual:

```
corrected_i = predicted_i + (normalized_score_i × residual)
```

The intuition: if the host power is higher than predicted, the correction should land mostly on cores whose coefficients just rose and which just became active — those are the most likely cause of the unaccounted power. If it is lower, it should land on cores whose coefficients fell and which just deactivated.

---

#### Stage 5 — Blend with reference (`blend_core_powers_with_reference`)

After correction, each core's estimate is softly pulled toward the reference allocation. The blend weight per core is:

```
blend_i = clamp(0.12 × uncertainty_i + 0.08 × change_share_i + 0.2 × uncertainty_i × mismatch_i, 0, 0.2)
final_i = corrected_i + blend_i × (reference_i − corrected_i)
```

The cap of 0.2 means the reference can move each core's estimate by at most 20% of the gap between the corrected value and the reference. This prevents the reference (which is a simple proportional split) from overriding the observer's accumulated state in steady conditions, while still allowing it to pull clearly drifted estimates back.

---

#### Stage 6 — Normalize to host (`normalize_core_powers_to_host`)

The blended vector is rescaled so its sum exactly equals `host_power`:

```
scale = host_power / sum_i(blended_i)
final_i = blended_i × scale
```

This hard constraint is the outer boundary of the observer. Whatever the predictor, corrector, and blender produced, the final output is always consistent with the measured host total.

---

#### `coef_to_power` update

After the final powers are produced, the global scale factor is updated as a slow EMA:

```
coef_to_power = 0.8 × coef_to_power + 0.2 × (total_final_power / total_coef)
```

This is used by the predictor in the next cycle to seed newly activating cores.

---

#### Bootstrap (cold start)

On the first sample there is no previous state to predict from. The reference allocation (the `coef^0.5` proportional split against `host_power`) is used directly, then normalized to the host total. `coef_to_power` is seeded from that initial state.

---

#### Summary

| Stage | Method | Purpose |
|---|---|---|
| Predict | `predict_core_powers` | Advance each core from its own coefficient ratio; decouple cores from each other |
| Reference | `build_reference_core_powers` | Compute a host-anchored `coef^0.5` split as the measurement signal |
| Uncertainty | `update_core_uncertainty` | Track per-core distrust as an EMA; gates how aggressively correction and blending act |
| Correct | `apply_host_residual_correction` | Redistribute `host − Σpredicted` to the cores most responsible, by uncertainty-weighted signal scores |
| Blend | `blend_core_powers_with_reference` | Softly pull drifted estimates back toward the reference; capped at 20% of the gap |
| Normalize | `normalize_core_powers_to_host` | Hard-enforce `Σfinal == host_power` |

The remaining limitation of this approach is that the correction and blend are applied per-cycle but do not accumulate state about how persistently the estimate has deviated. A core that is consistently 10% too high will be nudged down each cycle, but the nudge size is the same on cycle 2 as it is on cycle 200. That persistent steady-state bias is what the successor (`v_diff_pid`) addresses with an integral term.
