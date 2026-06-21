# JSON Exporter Output Format

Each measurement cycle the JSON exporter writes a single JSON object to stdout or a file. This document describes the top-level structure and every field, with particular attention to the per-core fields added in the current version.

## Top-level object

```jsonc
{
  "host": { ... },
  "idle": 1500000.0,          // optional – host idle power, µW
  "activation": 3000000.0,    // optional – host activation power, µW
  "background": 1500000.0,    // optional – host background power, µW
  "consumers": [ ... ],
  "sockets": [ ... ],
  "cores": [ ... ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `host` | object | Host-level power summary (see below) |
| `idle` | float \| null | Total host idle power in microwatts |
| `activation` | float \| null | Total host activation power in microwatts |
| `background` | float \| null | Total host background power in microwatts |
| `consumers` | array | Top processes by power consumption |
| `sockets` | array | Per-socket breakdown |
| `cores` | array | Per-core breakdown (see below) |

## `host` object

```jsonc
{
  "consumption": 12000000.0,
  "timestamp": 1750000000.123,
  "components": { "disks": [ ... ] },
  "activation": 3000000.0,
  "background": 1500000.0
}
```

`consumption` is the total host power in microwatts at this timestamp. `activation` and `background` are optional breakdowns of that total.

## `cores` array

Each element describes one logical CPU core for the current interval.

```jsonc
{
  "id": 0,
  "consumption": 450000.0,
  "timestamp": 1750000000.123,
  "coefficient": 0.312,
  "proportion": 0.085,
  "coefficient_diff": 0.014,
  "power_change_microwatts": 12000.0
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | int | Zero-based logical core index |
| `consumption` | float | Estimated current power consumed by this core, in microwatts |
| `timestamp` | float | Unix timestamp (seconds) of this measurement |
| `coefficient` | float | The core's activity coefficient for this interval (see below) |
| `proportion` | float | This core's share of the total coefficient sum, in [0, 1] |
| `coefficient_diff` | float | Signed change in this core's coefficient since the previous interval |
| `power_change_microwatts` | float | Attributed signed change in this core's power since the previous interval, in microwatts |

### `coefficient`

The coefficient captures how hard a core is working relative to its theoretical maximum. It is computed from hardware performance counters (APERF, MPERF, IPC):

```
coefficient = (1 + IPC) × APERF × (APERF / MPERF)
```

A higher coefficient means the core was both running at a higher fraction of its maximum frequency *and* executing instructions more efficiently. See [`docs/approach.md`](approach.md) for the full derivation.

### `proportion`

`proportion = coefficient_i / sum(coefficient_j for all j)`

This is the fraction of total core activity attributed to core `i`. It is used to split the host power reading proportionally when no better information is available.

### `coefficient_diff`

The signed change in `coefficient` from the previous interval to the current one. A positive value means the core became more active; negative means it became less active. This is the input to the power attribution algorithm.

### `power_change_microwatts`

The estimated change in this core's power consumption since the previous interval, in microwatts. It is computed as:

```
power_change_i = (coefficient_diff_i / sum(|coefficient_diff_j|)) × abs_power_delta_total
```

where `abs_power_delta_total` is derived from the measured change in host-level power (when available) or estimated from the running `coef_to_power` scaling factor (see [`docs/approach.md`](approach.md)). A positive value means the core drew more power than in the previous interval; negative means it drew less.

`consumption` is the cumulative result of applying these changes over time:

```
consumption[t] = consumption[t-1] + power_change_microwatts[t]
```

So `consumption` is the absolute estimate and `power_change_microwatts` is the interval delta that produced it.
