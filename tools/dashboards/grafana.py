"""What the dashboard generators share. Standard library only.

A generator beside this file is named for its dashboard, and one of them is `operator.py`, which is also the name of a
module of the standard library. Python puts a script's own directory first on `sys.path`, so every script of this
directory removes it (`del sys.path[0]`) before it imports anything else, and loads this file by its path.
"""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUTPUT_DIR = ROOT / "deploy" / "dashboards"

# The Prometheus to read is chosen in Grafana, from the variable of this name.
DATASOURCE = {"type": "prometheus", "uid": "${datasource}"}
WIDTH = 24


def target(expr, legend="", instant=False):
    return {"expr": expr, "instant": instant, "legendFormat": legend, "range": not instant}


def timeseries(title, targets, description="", unit="ops", width=12):
    return {
        "datasource": DATASOURCE,
        "description": description,
        "fieldConfig": {"defaults": {"unit": unit}, "overrides": []},
        "targets": targets,
        "title": title,
        "type": "timeseries",
        "width": width,
    }


def stat(title, targets, description="", width=6):
    return {
        "datasource": DATASOURCE,
        "description": description,
        "fieldConfig": {"defaults": {"decimals": 0, "unit": "short"}, "overrides": []},
        "options": {"reduceOptions": {"calcs": ["lastNotNull"], "fields": "", "values": False}},
        "targets": targets,
        "title": title,
        "type": "stat",
        "width": width,
    }


def note(title, text):
    return {"options": {"content": text, "mode": "markdown"}, "title": title, "type": "text", "width": WIDTH}


def layout(panels, heights):
    """Give each panel an id, its ref ids and a place: left to right, a new row when one does not fit."""
    x = y = row = 0
    placed = []
    for number, panel in enumerate(panels, start=1):
        panel = dict(panel)
        width = panel.pop("width")
        height = heights[panel["type"]]
        if x + width > WIDTH:
            x, y, row = 0, y + row, 0
        panel["gridPos"] = {"h": height, "w": width, "x": x, "y": y}
        panel["id"] = number
        if "targets" in panel:
            panel["targets"] = [
                dict(t, datasource=DATASOURCE, refId=chr(ord("A") + i)) for i, t in enumerate(panel["targets"])
            ]
        x, row = x + width, max(row, height)
        placed.append(panel)
    return placed


def dashboard(uid, title, description, panels):
    return {
        "description": description,
        "editable": False,
        "panels": layout(panels, {"stat": 5, "text": 4, "timeseries": 8}),
        "refresh": "1m",
        "schemaVersion": 39,
        "tags": ["another-agentic-platform"],
        "templating": {
            "list": [{"label": "Prometheus", "name": "datasource", "query": "prometheus", "type": "datasource"}]
        },
        "time": {"from": "now-6h", "to": "now"},
        "title": title,
        "uid": uid,
        "version": 1,
    }


def render(board):
    """The bytes of the file: sorted keys, two spaces, a newline at the end."""
    return (json.dumps(board, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


def write(name, board):
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    path = OUTPUT_DIR / f"{name}.json"
    path.write_bytes(render(board))
    print(f"wrote {path.relative_to(ROOT)}")
