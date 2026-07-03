"""Data flow pipeline diagram -- sensor to exporter.

Usage:
    pip install graphviz
    python contribution_data_flow.py

Output: contribution_data_flow.png
"""

import os
from graphviz import Digraph

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contribution_data_flow")

dot = Digraph(
    name="contribution_data_flow",
    format="png",
    graph_attr={
        "rankdir": "LR",
        "label": "Power Attribution Data Flow",
        "labelloc": "t",
        "fontsize": "20",
        "fontname": "Arial",
        "dpi": "300",
        "splines": "ortho",
    },
)

# Data sources
dot.node("rapl", "RAPL\nsysfs", shape="box3d", style="filled", fillcolor="#e1f5fe")
dot.node("perf", "Perf counters\nAPERF/MPERF/INST", shape="box3d", style="filled", fillcolor="#e1f5fe")
dot.node("ebpfk", "eBPF kernel\nprograms", shape="box", style="filled", fillcolor="#f3e5f5")

# Processing
dot.node("sensor", "PowercapRAPL\nSensor", shape="box", style="filled", fillcolor="#bbdefb")
dot.node("topology", "Topology\nrefresh()", shape="box", style="filled", fillcolor="#e0f7fa")
dot.node("ebpf_data", "eBPF maps\nreader", shape="box", style="filled", fillcolor="#f3e5f5")
dot.node("proc", "/proc/stat\n/proc/sysinfo", shape="box3d", style="filled", fillcolor="#fff9c4")

dot.node("attribution", "Core to Process\nPower Attribution", shape="box", style="filled", fillcolor="#fff3e0")
dot.node("metricgen", "MetricGenerator\ngen_all_metrics()", shape="box", style="filled", fillcolor="#e0f2f1")
dot.node("exporter", "Exporter\npop_metrics()\nto output", shape="box", style="filled", fillcolor="#e8f5e9")

# Edges
dot.edge("rapl", "sensor", label="energy_uj", fontsize="10")
dot.edge("sensor", "topology", label="Topology", fontsize="10")
dot.edge("perf", "topology", label="core metrics", fontsize="10")
dot.edge("ebpfk", "ebpf_data", label="maps", fontsize="10")
dot.edge("ebpf_data", "topology", label="CPU_TIME / events", fontsize="10")
dot.edge("proc", "topology", label="CPU / process stats", fontsize="10")

dot.edge("topology", "attribution", label="per-core powers", fontsize="10")
dot.edge("attribution", "topology", label="per-process powers", fontsize="10")

dot.edge("topology", "metricgen", label="refreshed state", fontsize="10")
dot.edge("metricgen", "exporter", label="Metrics[]", fontsize="10")

dot.render(filename=_OUT, cleanup=True)
print(f"Generated {_OUT}")
