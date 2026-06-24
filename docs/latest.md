# Latest Changes — v_propo_preserving_delta_of_delta

This file documents the changes introduced in `v_propo_preserving_delta_of_delta`, the current approach. For the full rationale and history see [`docs/approach.md`](approach.md). For infrastructure fixes (RAPL wrap-around, eBPF corrections, CPU-domain scoping) see [`docs/fixes.md`](fixes.md).

---

## Sign consistency check

Before attributing a power delta to cores, the algorithm now checks that the direction of the host CPU power change is consistent with the direction of aggregate core activity:

```
if power_delta × net_coef_change < 0 → return previous core powers unchanged
```

If the CPU power went up while total core activity (sum of coefficient deltas) went down, or vice versa, the two signals contradict each other. The most likely cause is a power shift from non-core sources (memory pressure, thermal, frequency transitions) that the coefficient signal has no way to explain. In that case the previous per-core powers are returned unchanged rather than producing a physically implausible attribution.

---

## Delta muffling by relative coefficient change

Each core's attributed power change is attenuated before being applied:

```
attenuation_i = clamp(|Δcoef_i| / before_last_coef_i, 0, 1)
candidate_i   = prev_power_i + attenuation_i × power_change_i
```

`before_last_coef_i` is the core's coefficient from the second-to-last measurement — the baseline the current Δcoef is measured against.

**Why this helps:** the attribution algorithm assumes power change is proportional to coefficient change. That assumption is most credible when the coefficient change is large relative to where the coefficient already was. A tiny Δcoef on top of a large existing coefficient is a low-confidence signal — the difference could easily be noise. Clamping `|Δcoef| / before_last_coef` to [0, 1] and using it as a multiplier on the delta reduces the influence of low-confidence intervals without discarding them entirely.

Falls back to `attenuation = 1.0` (no muffling) when `before_last_coef_i` is zero or unavailable.

---

## Negative-core recalibration

After the attenuated delta is applied, any core with a negative result is recalibrated:

```
if candidate_i < 0:
    power_i = coef_to_power × before_last_coef_i
```

A negative core power means the accumulated state has drifted below zero. There is no ground truth available to correct the drift gradually, so the estimate is reset to `coef_to_power × before_last_coef_i`. This is the power level the core would have if the running scale factor were applied directly to its last known coefficient — a physically plausible anchor that does not require any additional measurement.

Only the affected core is recalibrated. Cores with valid positive values are unchanged.

---

## Proportionality-preserving rescale

After muffling and recalibration, the per-core powers are rescaled so each core's share of total power matches its share of the current total coefficient:

```
power_i = coef_i × (Σ power_j / Σ coef_j)
```

`coef_i` is the core's coefficient from the latest measurement interval. The total power magnitude (`Σ power_j`) is preserved; only the distribution is adjusted.

**Why this helps:** the muffled delta update correctly tracks *changes* in per-core power, but it is blind to whether the absolute level is consistent with the core's current activity. A core that was heavily loaded in past cycles can carry a high power estimate into intervals where it is nearly idle, because each delta only adds a small correction and never re-anchors to absolute activity.

The coefficient directly reflects how hard a core is working in the current interval. Rescaling to be proportional to the current coefficient eliminates this history-drift problem: a core that is now nearly idle will be pulled toward a low estimate regardless of its past; a core that just became active will be pulled up.

Skipped when either sum is zero (all cores idle, or no coefficient data available).

---

## Socket-level rescale

After the proportionality rescale, the per-core vector is normalised to exactly match the measured socket CPU power:

```
residual = socket_cpu_power − Σ power_i
power_i  = power_i + (power_i / Σ power_i) × residual
```

This hard outer constraint is applied last. It ensures the final per-core vector is always consistent with the directly measured socket-level RAPL reading. The residual is distributed proportionally to each core's current power estimate.
