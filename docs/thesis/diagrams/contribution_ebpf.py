"""eBPF programs and maps interaction diagram.

Layout: Programs on the left, maps on the right, arrows left-to-right
for readability. Userspace consumers shown below the maps.

Usage:
    pip install graphviz
    python contribution_ebpf.py

Output: contribution_ebpf.png
"""

import os
from graphviz import Digraph

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contribution_ebpf")

dot = Digraph(
    name="contribution_ebpf",
    format="png",
    graph_attr={
        "rankdir": "LR",
        "label": "eBPF Programs and Maps",
        "labelloc": "t",
        "fontsize": "20",
        "fontname": "Arial",
        "dpi": "300",
        "splines": "ortho",
        "ranksep": "1.2",
    },
)

# ── Subgraph: Kernel-space ──
with dot.subgraph(name="cluster_kernel") as ker:
    ker.attr(
        label="Kernel (eBPF)",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )

    # Programs (left column)
    ker.node("ss", "context_switch_tracker\n(sched_switch)", shape="box", style="filled", fillcolor="#e1f5fe")
    ker.node("pe", "process_exit_cleanup\n(sched_process_exit)", shape="box", style="filled", fillcolor="#fff3e5")
    ker.node("st", "sample_tick\n(200 Hz per CPU)", shape="box", style="filled", fillcolor="#e8f5e9")
    ker.node("cst", "cpu_state_tick\n(100 Hz per socket)", shape="box", style="filled", fillcolor="#f3e5f5")

    # Maps (right column)
    ker.node("pt", "PID_TIMES\nPerCpuHashMap<u32, u64>", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("pl", "PID_LAST\nPerCpuHashMap<u32, u64>", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("ttg", "TID_TO_TGID\nHashMap<u32, u32>", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("ct", "CPU_TIME\nArray<u64>", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("cs", "CPU_SNAPSHOT\nArray<u64>", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("cse", "CPU_STATE_EVENTS\nRingBuf", shape="cylinder", style="filled", fillcolor="#fce4ec")
    ker.node("cts", "CPU_TO_SOCKET\nArray<u16>", shape="cylinder", style="filled", fillcolor="#fce4ec")

    # Program to Map edges (left to right)
    ker.edge("ss", "pt", label="delta", fontsize="9")
    ker.edge("ss", "pl", label="timestamp", fontsize="9")
    ker.edge("ss", "ttg", label="seed", fontsize="9")
    ker.edge("ss", "ct", label="delta", fontsize="9")

    ker.edge("pe", "pt", label="cleanup", fontsize="9")
    ker.edge("pe", "pl", label="cleanup", fontsize="9")
    ker.edge("pe", "ttg", label="cleanup", fontsize="9")

    ker.edge("st", "pt", label="periodic", fontsize="9")
    ker.edge("st", "pl", label="timestamp", fontsize="9")
    ker.edge("st", "ct", label="periodic", fontsize="9")

    ker.edge("cst", "ct", label="compare", fontsize="9")
    ker.edge("cst", "cs", label="snapshot", fontsize="9")
    ker.edge("cst", "cts", label="lookup", fontsize="9")
    ker.edge("cst", "cse", label="push event", fontsize="9")

# ── Subgraph: Userspace ──
with dot.subgraph(name="cluster_userspace") as us:
    us.attr(
        label="Userspace (Rust)",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )

    us.node("topo", "Topology", shape="box", style="filled", fillcolor="#e0f7fa")
    us.node("proct", "ProcessTracker", shape="box", style="filled", fillcolor="#b2dfdb")

    us.edge("topo", "cts", label="populate\nat startup", fontsize="9", style="dashed")

# Cross-boundary reads (maps → userspace)
dot.edge("ct", "topo", label="read each cycle", fontsize="9")
dot.edge("cse", "topo", label="drain each cycle", fontsize="9")
dot.edge("pt", "proct", label="drained each cycle", fontsize="9")

dot.render(filename=_OUT, cleanup=True)
print(f"Generated {_OUT}")
