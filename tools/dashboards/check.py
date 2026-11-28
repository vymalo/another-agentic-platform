#!/usr/bin/env python3
"""Fails unless the checked-in dashboards are what their generators say, and every metric a panel queries is one the
services emit. Standard library only. Run it from anywhere: `python3 tools/dashboards/check.py`."""

import sys

sys.dont_write_bytecode = True

import importlib.util
import re
from pathlib import Path

import grafana

HERE = Path(__file__).resolve().parent
GENERATORS = ("operator_dashboard", "api_dashboard")
# Where the services declare the names of their series.
SOURCES = ("crates/controller/src/metrics.rs", "crates/platform-api/src/metrics.rs")
CONSTANT = re.compile(r'pub const [A-Z0-9_]+: &str =\s*"(aap_[a-z0-9_]+)";')
# Wider than the names are, so that a misspelt name is read whole and not as a shorter one.
QUERIED = re.compile(r"\baap_\w*")


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def emitted(root):
    names = set()
    for source in SOURCES:
        found = CONSTANT.findall((root / source).read_text(encoding="utf-8"))
        if not found:
            sys.exit(f"{source}: no series name found: the pattern changed, so this check would prove nothing")
        names.update(found)
    return names


def drift(generators):
    """`generators` maps a dashboard's NAME to the script that writes it and what it builds."""
    failures = []
    for name, (script, board) in generators.items():
        path = grafana.OUTPUT_DIR / f"{name}.json"
        shown = path.relative_to(grafana.ROOT)
        if not path.is_file():
            failures.append(f"{shown}: missing")
        elif path.read_bytes() != grafana.render(board):
            failures.append(f"{shown}: differs from what tools/dashboards/{script}.py generates")
    for path in sorted(grafana.OUTPUT_DIR.glob("*.json")):
        if path.stem not in generators:
            failures.append(f"{path.relative_to(grafana.ROOT)}: no generator writes it")
    if failures:
        failures.append("run `python3 tools/dashboards/operator_dashboard.py` and `python3 tools/dashboards/api_dashboard.py`")
    return failures


def unknown_metrics(boards, names):
    failures, queried, expressions = [], set(), 0
    for name, board in boards.items():
        before = expressions
        for panel in board["panels"]:
            for query in panel.get("targets", []):
                expr = query["expr"]
                where = f"{name}.json, panel \"{panel['title']}\": {expr}"
                found = QUERIED.findall(expr)
                expressions += 1
                if not found:
                    failures.append(f"{where}: queries no series of the services")
                for metric in found:
                    if metric in names:
                        queried.add(metric)
                    else:
                        failures.append(f"{where}: unknown metric {metric}")
        if expressions == before:
            failures.append(f"{name}.json: no query found, so nothing was checked")
    return failures, queried, expressions


def main():
    loaded = {script: load(script) for script in GENERATORS}
    generators = {module.NAME: (script, module.build()) for script, module in loaded.items()}
    boards = {name: board for name, (_, board) in generators.items()}
    names = emitted(grafana.ROOT)
    unknown, queried, expressions = unknown_metrics(boards, names)
    failures = drift(generators) + unknown
    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print(
        f"{len(boards)} dashboards are what their generators say; {expressions} queries name only emitted series "
        f"({len(queried)} of {len(names)} series are on a panel)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
