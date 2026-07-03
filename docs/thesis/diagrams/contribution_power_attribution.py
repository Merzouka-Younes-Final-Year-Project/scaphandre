"""Power attribution flow diagram.

Shows how perf counters feed into per-core coefficients, which normalize
into proportional core power, then per-logical-CPU breakdown via eBPF
for final process-level attribution.

Usage:
    pip install graphviz
    python contribution_power_attribution.py

Output: contribution_power_attribution.png
"""

import os
from graphviz import Digraph

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contribution_power_attribution")

dot = Digraph(
    name="contribution_power_attribution",
    format="png",
    graph_attr={
        "rankdir": "TB",
        "label": "Power Attribution Flow",
        "labelloc": "t",
        "fontsize": "20",
        "fontname": "Arial",
        "dpi": "300",
        "splines": "ortho",
        "ranksep": "0.8",
    },
)

# ── Subgraph: Perf Counters ──
with dot.subgraph(name="cluster_counters") as cnt:
    cnt.attr(
        label="Perf Counters (per core)",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    cnt.node("aperf", "APERF (MSR 0xE8)", shape="box", style="filled", fillcolor="#e1f5fe")
    cnt.node("mperf", "MPERF (MSR 0xE7)", shape="box", style="filled", fillcolor="#e1f5fe")
    cnt.node("inst", "INST_RETIRED", shape="box", style="filled", fillcolor="#e1f5fe")
    cnt.node("ucc", "UCC (cycles)", shape="box", style="filled", fillcolor="#e1f5fe")
    cnt.node("ipc", "IPC = INST / UCC", shape="box", style="filled", fillcolor="#fff3e0")

    cnt.edge("inst", "ipc", label="numerator", fontsize="9")
    cnt.edge("ucc", "ipc", label="denominator", fontsize="9")

# ── Subgraph: Socket (per-core split) ──
with dot.subgraph(name="cluster_socket") as soc:
    soc.attr(
        label="Socket (per-core split)",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    soc.node("rapl", "RAPL Core Domain", shape="cylinder", style="filled", fillcolor="#e1f5fe")
    soc.node("sub", "Subtract Background", shape="box", style="filled", fillcolor="#f3e5f5")
    soc.node("coef", "Coefficient\n(1+IPC) x APERF²/MPERF", shape="diamond", style="filled", fillcolor="#fff3e0")
    soc.node("norm", "Normalize across cores\n→ proportion_i", shape="box", style="filled", fillcolor="#f3e5f5")
    soc.node("mul", "proportion_i × active_power", shape="box", style="filled", fillcolor="#e0f7fa")
    soc.node("c0", "Core 0 Power", shape="box", style="filled", fillcolor="#b2dfdb")
    soc.node("c1", "Core 1 Power", shape="box", style="filled", fillcolor="#b2dfdb")
    soc.node("cn", "Core N Power", shape="box", style="filled", fillcolor="#b2dfdb")

    soc.edge("rapl", "sub", label="power_delta", fontsize="9")
    soc.edge("sub", "mul", label="active_power", fontsize="9")
    soc.edge("coef", "norm", label="normalize", fontsize="9")
    soc.edge("norm", "mul", label="proportion_i", fontsize="9")
    soc.edge("aperf", "coef", style="dashed", fontsize="9")
    soc.edge("mperf", "coef", style="dashed", fontsize="9")
    soc.edge("ipc", "coef", style="dashed", fontsize="9")
    soc.edge("mul", "c0", label="core_i", fontsize="9")
    soc.edge("mul", "c1", label="core_i", fontsize="9")
    soc.edge("mul", "cn", label="core_i", fontsize="9")

# ── Subgraph: Process Attribution ──
with dot.subgraph(name="cluster_process") as prc:
    prc.attr(
        label="Process Attribution",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    prc.node("ebpf", "PID_TIMES + CPU_TIME", shape="cylinder", style="filled", fillcolor="#fce4ec")
    prc.node("cpu00", "CPU 0.0 Power", shape="box", style="filled", fillcolor="#ffe0b2")
    prc.node("cpu01", "CPU 0.1 Power", shape="box", style="filled", fillcolor="#ffe0b2")
    prc.node("cpu10", "CPU 1.0 Power", shape="box", style="filled", fillcolor="#ffe0b2")
    prc.node("total", "Total Process Power", shape="box", style="filled", fillcolor="#e8f5e9")

    prc.edge("ebpf", "cpu00", label="per-CPU ratio", fontsize="9")
    prc.edge("ebpf", "cpu01", label="per-CPU ratio", fontsize="9")
    prc.edge("ebpf", "cpu10", label="per-CPU ratio", fontsize="9")

# Cross-subgraph edges
dot.edge("c0", "cpu00", label="x cpu_time_proportion", fontsize="9")
dot.edge("c0", "cpu01", label="x cpu_time_proportion", fontsize="9")
dot.edge("c1", "cpu10", label="x cpu_time_proportion", fontsize="9")

dot.edge("cpu00", "total", label="sum", fontsize="9")
dot.edge("cpu01", "total", label="sum", fontsize="9")
dot.edge("cpu10", "total", label="sum", fontsize="9")

dot.render(filename=_OUT, cleanup=True)
print(f"Generated {_OUT}")
