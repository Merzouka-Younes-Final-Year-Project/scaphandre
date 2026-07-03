"""Overall system architecture diagram.

Usage:
    pip install graphviz
    python contribution_architecture.py

Output: contribution_architecture.png
"""

import os
from graphviz import Digraph

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contribution_architecture")

dot = Digraph(
    name="contribution_architecture",
    format="png",
    graph_attr={
        "rankdir": "TB",
        "label": "Scaphandre Architecture Overview",
        "labelloc": "t",
        "fontsize": "20",
        "fontname": "Arial",
        "dpi": "300",
        "splines": "polyline",
        "compound": "true",
    },
)

# ── Subgraph: Hardware ──
with dot.subgraph(name="cluster_hardware") as hw:
    hw.attr(
        label="Hardware",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    hw.node("rapl", "RAPL\n(powercap sysfs)", shape="box3d", style="filled", fillcolor="#e1f5fe")
    hw.node("pmu", "Performance Counters\nAPERF / MPERF / INST", shape="box3d", style="filled", fillcolor="#e1f5fe")

# ── Subgraph: Kernel ──
with dot.subgraph(name="cluster_kernel") as ker:
    ker.attr(
        label="Linux Kernel",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    ker.node(
        "ebpf_progs",
        "eBPF Programs\ncontext_switch_tracker / process_exit_cleanup\nsample_tick / cpu_state_tick",
        shape="box",
        style="filled",
        fillcolor="#f3e5f5",
    )
    ker.node(
        "ebpf_maps",
        "eBPF Maps\nPID_TIMES / CPU_TIME\nCPU_STATE_EVENTS / \u2026",
        shape="cylinder",
        style="filled",
        fillcolor="#fce4ec",
    )

# ── Subgraph: Userspace ──
with dot.subgraph(name="cluster_userspace") as us:
    us.attr(
        label="Userspace",
        style="dashed",
        color="#4a4a4a",
        fontsize="14",
        fontname="Arial",
    )
    us.node("sensor", "Sensor\n(PowercapRAPLSensor)", shape="box", style="filled", fillcolor="#e1f5fe")
    us.node("topology", "Topology\nSockets / Domains / Cores", shape="box", style="filled", fillcolor="#e0f7fa")
    us.node("attribution", "Power Attribution\nCore \u2192 Process", shape="box", style="filled", fillcolor="#fff3e0")
    us.node("ebpf_user", "eBPF Userspace\nMap & RingBuf Reader", shape="box", style="filled", fillcolor="#f3e5f5")
    us.node("metricgen", "MetricGenerator", shape="box", style="filled", fillcolor="#e0f2f1")
    us.node("exporter", "Exporter\n(JSON / Prometheus / stdout)", shape="box", style="filled", fillcolor="#e8f5e9")

# ── Edges ──
# Hardware to sensor/topology
dot.edge("rapl", "sensor", label="energy_uj", fontsize="10")
dot.edge("pmu", "topology", label="perf counters", fontsize="10")

# Sensor to Topology
dot.edge("sensor", "topology", label="generate_topology()", fontsize="10")

# eBPF
dot.edge("ebpf_progs", "ebpf_maps", label="write", fontsize="10")
dot.edge("ebpf_maps", "ebpf_user", label="read", fontsize="10")
dot.edge("ebpf_user", "topology", label="CPU_TIME / events", fontsize="10")

# Topology <-> Attribution (bidirectional)
dot.edge("topology", "attribution", label="socket topology + coefs", fontsize="10")
dot.edge("attribution", "topology", label="per-core, per-process power", fontsize="10")

# Topology -> MetricGenerator -> Exporter
dot.edge("topology", "metricgen", label="refreshed state", fontsize="10")
dot.edge("metricgen", "exporter", label="gen_all_metrics()", fontsize="10")

dot.render(filename=_OUT, cleanup=True)
print(f"Generated {_OUT}")
