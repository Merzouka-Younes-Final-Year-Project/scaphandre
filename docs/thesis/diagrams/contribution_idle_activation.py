"""Socket-level idle/activation detection flowchart.

Compact three-phase view of cpu_state_tick, suitable for print:
  1. Scan CPUs, count active per socket
  2. Emit events per socket
  3. Update CPU_SNAPSHOT for all CPUs

Usage:
    pip install graphviz
    python contribution_idle_activation.py

Output: contribution_idle_activation.png
"""

import os
from graphviz import Digraph

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contribution_idle_activation")

dot = Digraph(
    name="contribution_idle_activation",
    format="png",
    graph_attr={
        "rankdir": "TB",
        "label": "Per-Socket Idle / Activation Detection (cpu_state_tick)",
        "labelloc": "t",
        "fontsize": "16",
        "fontname": "Arial",
        "dpi": "300",
        "splines": "ortho",
        "ranksep": "0.8",
    },
)

dot.attr("node", fontname="Arial", fontsize="11")

# Three compact phase nodes summarising the eBPF logic.
dot.node(
    "phase1",
    "Phase 1 — Scan CPUs\n"
    "For cpu in 0 .. MAX_CPU:\n"
    "  read CPU_TO_SOCKET[cpu]\n"
    "  if socket == 0: break (no more CPUs)\n"
    "  delta = CPU_TIME[cpu] - CPU_SNAPSHOT[cpu]\n"
    "  if delta != 0: active_cpus[socket]++\n",
    shape="box",
    style="filled",
    fillcolor="#e1f5fe",
)

dot.node(
    "phase2",
    "Phase 2 — Emit events\n"
    "For socket in 0 .. MAX_SOCKET:\n"
    "  if active_cpus[socket] == u32::MAX: break\n"
    "  0 active  → IdleEvent\n"
    "  1 active  → ActivationEvent\n"
    "  >1 active → no event\n"
    "  push to CPU_STATE_EVENTS ring buffer",
    shape="box",
    style="filled",
    fillcolor="#fff3e0",
)

dot.node(
    "phase3",
    "Phase 3 — Update snapshots\n"
    "For cpu in 0 .. MAX_CPU:\n"
    "  CPU_SNAPSHOT[cpu] := CPU_TIME[cpu]",
    shape="box",
    style="filled",
    fillcolor="#e8f5e9",
)

dot.edge("phase1", "phase2")
dot.edge("phase2", "phase3")

dot.render(filename=_OUT, cleanup=True)
print(f"Generated {_OUT}")