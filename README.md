<p align="center">
    <img src="https://github.com/hubblo-org/scaphandre/raw/main/docs_src/scaphandre.cleaned.png" width="200">
</p>
<h1 align="center">
  Scaphandre
</h1>

<h3 align="center">
    Your tech stack doesn't need so much energy ⚡
</h3>

---

Scaphandre *[skafɑ̃dʁ]* is a metrology agent dedicated to electric [power](https://en.wikipedia.org/wiki/Electric_power) and energy consumption metrics. The goal of the project is to permit to any company or individual to **measure** the power consumption of its tech services and get this data in a convenient form, sending it through any monitoring or data analysis toolchain.

**Scaphandre** means *heavy* **diving suit** in [:fr:](https://fr.wikipedia.org/wiki/Scaphandre_%C3%A0_casque). It comes from the idea that tech related services often don't track their power consumption and thus don't expose it to their clients. Most of the time the reason is a presumed bad [ROI](https://en.wikipedia.org/wiki/Return_on_investment). Scaphandre makes, for tech providers and tech users, easier and cheaper to go under the surface to bring back the desired power consumption metrics, take better sustainability focused decisions, and then show the metrics to their clients to allow them to do the same.

This project was born from a deep sense of duty from tech workers. Please refer to the [why](https://hubblo-org.github.io/scaphandre-documentation/why.html) section to know more about its goals.

**Warning**: this is still a very early stage project. Any feedback or contribution will be highly appreciated. Please refer to the [contribution](https://hubblo-org.github.io/scaphandre-documentation/contributing.html) section.

![Fmt+Clippy](https://github.com/hubblo-org/scaphandre/workflows/Tests/badge.svg?branch=main)
[![](https://img.shields.io/crates/v/scaphandre.svg?maxAge=25920)](https://crates.io/crates/scaphandre)
<a href="https://gitter.im/hubblo-org/scaphandre?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge"><img src="https://badges.gitter.im/Join%20Chat.svg"></a>

Join us on [Gitter](https://gitter.im/hubblo-org/scaphandre) or [Matrix](https://app.element.io/#/room/#hubblo-org_scaphandre:gitter.im) !

---

## ✨ Features

- measuring power/energy consumed on **bare metal hosts**
- measuring power/energy consumed of **qemu/kvm virtual machines** from the host
- **exposing** power/energy metrics of a virtual machine, to allow **manipulating those metrics in the VM** as if it was a bare metal machine (relies on hypervisor features)
- **per-core power attribution** — distributes CPU-domain RAPL power to individual cores using hardware performance counters (APERF, MPERF, IPC)
- **eBPF-based idle/activation detection** — real-time socket-level idle and activation power tracking without offline calibration
- **background power subtraction** — automatically separates idle/activation overhead from active workload power
- **process-level power attribution** — distributes per-core power to processes by eBPF-observed CPU time
- exposing metrics as a **[prometheus](https://prometheus.io) (HTTP) exporter**
- sending metrics in push mode to a **[prometheus](https://prometheus.io) [Push Gateway](https://github.com/prometheus/pushgateway)**
- sending metrics to **[riemann](http://riemann.io/)**
- sending metrics to **[Warp10](http://warp10.io/)**
- works on **[kubernetes](https://kubernetes.io/)**
- storing power consumption metrics in a **JSON** file (with per-core coefficient, proportion, and power breakdown)
- showing basic power consumption metrics **in the terminal**
- operating systems supported so far : **Gnu/Linux**, **Windows 10, 11 and Server 2016/2019/2022**
- packages available for **RHEL 8 and 9, Debian 11 and 12 and Windows**, also **NixOS** (community support)

Here is an example dashboard built thanks to scaphandre: [https://metrics.hubblo.org](https://metrics.hubblo.org).

<a href="https://metrics.hubblo.org"><img src="https://github.com/hubblo-org/scaphandre/raw/main/docs_src/grafana-dash-scaphandre.cleaned.png" width="800"></a>

## 📄 How to ... ?

You'll find everything you may want to know about scaphandre in the [documentation](https://hubblo-org.github.io/scaphandre-documentation), like:

- 🏁 [Getting started](https://hubblo-org.github.io/scaphandre-documentation/tutorials/getting_started.html)
- 💻 [Installation & compilation on GNU/Linux](https://hubblo-org.github.io/scaphandre-documentation/tutorials/installation-linux.html) or [on Windows](https://hubblo-org.github.io/scaphandre-documentation/tutorials/installation-windows.html)
- 👁️ [Give a virtual machine access to its power consumption metrics, and break the opacity of being on the computer of someone else](https://hubblo-org.github.io/scaphandre-documentation/how-to_guides/propagate-metrics-hypervisor-to-vm_qemu-kvm.html)
- 🎉 [Contributing guide](https://hubblo-org.github.io/scaphandre-documentation/contributing.html)
- [And much more](https://hubblo-org.github.io/scaphandre-documentation)

If you are only interested in the code documentation [here it is](https://docs.rs/scaphandre).

## 📅 Roadmap

The ongoing roadmap can be seen [here](https://github.com/hubblo-org/scaphandre/projects/1). Feature requests are welcome, please join us.

## ⚖️  Footprint

In opposition to its name, scaphandre aims to be as light and clean as possible. One of the main focus areas of the project is to come as close as possible to a 0 overhead, both about resources consumption and power consumption.

## 🙏 Sponsoring

If you like this project and would like to provide financial help, here's our [sponsoring page](https://github.com/sponsors/hubblo-org).
Thanks a lot for considering it !

---

## 🛠 Development

### Prerequisites

1. stable rust toolchains: `rustup toolchain install stable`
1. nightly rust toolchains: `rustup toolchain install nightly --component rust-src`
1. (if cross-compiling) rustup target: `rustup target add ${ARCH}-unknown-linux-musl`
1. (if cross-compiling) LLVM: (e.g.) `brew install llvm` (on macOS)
1. bpf-linker: `cargo install bpf-linker` (`--no-default-features` on macOS)

### Build & Run

Use `cargo build`, `cargo check`, etc. as normal. Run your program with:

```shell
cargo run --release
```

Cargo build scripts are used to automatically build the eBPF correctly and include it in the program.

### Cross-compiling on macOS

Cross compilation should work on both Intel and Apple Silicon Macs.

```shell
cargo build --package scaphandre --release \
  --target=${ARCH}-unknown-linux-musl \
  --config=target.${ARCH}-unknown-linux-musl.linker=\"rust-lld\"
```

The cross-compiled program `target/${ARCH}-unknown-linux-musl/release/scaphandre` can be copied to a Linux server or VM and run there.

## Repository Guide

The project is organised as a Cargo workspace with three crates:

### [`scaphandre/`](scaphandre/) — Main binary

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry point: CLI parsing, exporter dispatch, top-level event loop |
| `src/lib.rs` | Library root; re-exports sensor types |
| `src/bpf.rs` | eBPF program loader: attaches `context_switch_tracker`, `sample_tick`, `cpu_state_tick`, and `process_exit_cleanup` programs |
| `src/sensors/mod.rs` | **Core of the power attribution pipeline**: `Topology`, `CPUSocket`, `CPUCore`, `Domain` structs, coefficient calculation, proportional power attribution, idle/activation detection, background power, process tracking, eBPF ring-buffer draining |
| `src/sensors/powercap_rapl.rs` | Linux RAPL sensor: reads energy counters from `powercap` sysfs, discovers domains, handles counter wraparound |
| `src/sensors/utils.rs` | `ProcessTracker` — per-process CPU time tracking from eBPF or `/proc` fallback |
| `src/sensors/units.rs` | Unit definitions (MicroJoule, MicroWatt, etc.) |
| `src/exporters/` | Exporter implementations (JSON, Prometheus, stdout, Riemann, Warp10, QEMU) |

### [`scaphandre-ebpf/`](scaphandre-ebpf/) — eBPF kernel-side programs

| Path | Purpose |
|------|---------|
| `src/main.rs` | eBPF programs: `context_switch_tracker` (per-process CPU time on `sched_switch`), `sample_tick` (periodic PID_TIMES update), `cpu_state_tick` (idle/activation detection per socket), `process_exit_cleanup` (map entry cleanup on process exit) |
| `src/vmlinux.rs` | Auto-generated kernel type definitions (BPD) |
| `src/lib.rs` | Library target shim |

The eBPF subsystem uses seven maps: `PID_TIMES`, `PID_LAST`, `TID_TO_TGID`, `CPU_TIME`, `CPU_SNAPSHOT`, `CPU_STATE_EVENTS` (ring buffer), and `CPU_TO_SOCKET`.

### [`scaphandre-common/`](scaphandre-common/) — Shared types

`src/lib.rs` defines `CpuEventType` and `CpuStateEvent` used by both the eBPF program and the userspace sensor.

### Power attribution pipeline

The current implementation uses **proportional per-core attribution** scoped to the CPU RAPL domain:

1. Per-core coefficients are computed from hardware counters: `coef = (1 + IPC) × APERF × (APERF / MPERF)`
2. Coefficients are normalised to proportions that sum to 1.0 across all cores
3. Each core's power is `proportion × cpu_domain_power` (with background already subtracted)
4. Process power is the sum across cores of `core_power × process_cpu_time_share` on that core

**Idle/activation power** is tracked via the eBPF `cpu_state_tick` program (100 Hz): it classifies each socket as fully idle or exactly-one-core active, and records the current RAPL power as the idle/activation baseline. The maximum of the two (activation ≥ idle in practice) is subtracted as `background` from the raw RAPL reading before attribution.

### Key documents

| File | Content |
|------|---------|
| [`docs/overview.md`](docs/overview.md) | High-level architecture, coefficient derivation, eBPF maps, power attribution pipeline |
| [`docs/json.md`](docs/json.md) | JSON exporter output format specification |

## ⚖️ License

With the exception of eBPF code, scaphandre is distributed under the terms of either the [MIT license](LICENSE-MIT) or the [Apache License](LICENSE-APACHE) (version 2.0), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

### eBPF

All eBPF code is distributed under either the terms of the [GNU General Public License, Version 2](LICENSE-GPL2) or the [MIT license](LICENSE-MIT), at your option.
