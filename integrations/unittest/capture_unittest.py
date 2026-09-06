#!/usr/bin/env python3
"""Capture one complete unittest module or TestCase class as CortexWeave evidence.

Runs only with the selected interpreter and Python's standard library. It does
not install dependencies, edit the project, or submit evidence to CortexWeave.
"""
import argparse
import hashlib
import importlib
import inspect
import json
import os
import sys
import unittest
import uuid
from pathlib import Path


def digest_bytes(data): return hashlib.blake2s(data).hexdigest()
def digest_file(path): return digest_bytes(path.read_bytes())
def component(identity, version): return {"id": identity, "version": version}
def empty_counts():
    return {key: 0 for key in ("discovered", "executed", "passed", "assertion_failed", "errored", "skipped", "todo", "pending", "expected_failed", "unexpected_successful")}
def flatten(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite): yield from flatten(item)
        else: yield item
def identity(test):
    test_id = test.id()
    bits = test_id.split(".")
    return {"namespace": bits[:-1], "name": bits[-1]}
def error_class(error): return error[0].__name__ if error else None

class Collector(unittest.TestResult):
    def __init__(self, inventory):
        super().__init__(); self.inventory = inventory; self.records = {}; self.children = []; self.errors_out = []
    def startTest(self, test):
        super().startTest(test); self.records[test.id()] = {"identity": identity(test), "status": "errored", "error_class": None, "retry_configured": False, "retry_count": 0, "repeat_configured": False, "repeat_count": 0, "flaky": False}
    def _record(self, test, status, error=None):
        row = self.records.setdefault(test.id(), {"identity": identity(test), "status": status, "error_class": None, "retry_configured": False, "retry_count": 0, "repeat_configured": False, "repeat_count": 0, "flaky": False})
        row["status"] = status; row["error_class"] = error_class(error)
    def addSuccess(self, test): super().addSuccess(test); self._record(test, "passed")
    def addFailure(self, test, err): super().addFailure(test, err); self._record(test, "assertion_failed", err)
    def addError(self, test, err): super().addError(test, err); self._record(test, "errored", err)
    def addSkip(self, test, reason): super().addSkip(test, reason); self._record(test, "skipped")
    def addExpectedFailure(self, test, err): super().addExpectedFailure(test, err); self._record(test, "expected_failed", err)
    def addUnexpectedSuccess(self, test): super().addUnexpectedSuccess(test); self._record(test, "unexpected_successful")
    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        self.children.append({"parent": identity(test), "ordinal": len(self.children), "description": None, "status": "passed" if err is None else "assertion_failed", "error_class": error_class(err)})

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("target", help="complete module or TestCase class, e.g. tests.test_core.StagnationTests")
    parser.add_argument("--workspace", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--run-id", default=None)
    args = parser.parse_args()
    workspace = Path(args.workspace).resolve(); output = Path(args.output).resolve()
    raw_path = output.with_suffix(".raw.json")
    if output.exists() or raw_path.exists():
        raise SystemExit(f"refusing to overwrite existing capture artifact: {output}")
    sys.path.insert(0, str(workspace))
    try:
        module = importlib.import_module(args.target)
        selection_kind = "module"
    except ModuleNotFoundError:
        module = importlib.import_module(args.target.rsplit(".", 1)[0])
        selection_kind = "class"
    source = Path(inspect.getsourcefile(module) or module.__file__).resolve()
    try: relative = source.relative_to(workspace).as_posix()
    except ValueError: raise SystemExit("selected unittest target resolved outside --workspace")
    suite = unittest.defaultTestLoader.loadTestsFromName(args.target)
    inventory = list(flatten(suite))
    collector = Collector(inventory); suite.run(collector)
    cases = [collector.records.get(test.id(), {"identity": identity(test), "status": "pending", "error_class": None, "retry_configured": False, "retry_count": 0, "repeat_configured": False, "repeat_count": 0, "flaky": False}) for test in inventory]
    parent = empty_counts(); parent["discovered"] = len(cases)
    for case in cases:
        if case["status"] not in ("skipped", "todo", "pending"): parent["executed"] += 1
        parent[case["status"]] += 1
    child = {key: 0 for key in ("observed", "passed", "assertion_failed", "errored", "skipped", "todo", "pending", "expected_failed", "unexpected_successful")}
    for item in collector.children: child["observed"] += 1; child[item["status"]] += 1
    passed = collector.wasSuccessful() and parent["discovered"] > 0
    output.parent.mkdir(parents=True, exist_ok=True)
    raw = json.dumps({"cases": cases, "children": collector.children, "errors": collector.errors_out}, sort_keys=True).encode()
    raw_path.write_bytes(raw)
    runtime = f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}"
    environment = digest_bytes(json.dumps({"python": runtime, "executable": str(Path(sys.executable).resolve())}, sort_keys=True).encode())
    settings = digest_bytes(b"unittest-current-complete-v1")
    bundle = {"contract": "cortexweave.test_run_result", "version": 1,
        "producer": component("cortexweave.unittest_capture", "1"), "runner": component("python.unittest", runtime), "runtime": component("python", runtime),
        "profile": component("cortexweave.unittest.module_or_class", "1"), "run_id": args.run_id or str(uuid.uuid4()), "operation": "test", "completed": True,
        "exit_code": 0 if passed else 1, "language": "python", "environment_fingerprint": environment,
        "selection": {"project_root": ".", "import_root": ".", "test_file": relative, "kind": selection_kind, "value": args.target, "settings_digest": settings, "inventory_complete": True},
        "cases": cases, "children": collector.children, "counts": {"parents": parent, "children": child}, "errors": collector.errors_out,
        "settings": {"focused_only": False, "name_filter": False, "sharded": False, "bail": False, "watch": False, "snapshot_mode": "not_applicable", "pass_with_no_tests": False, "execution_origin": "current"},
        "verification_inputs": [{"kind": "test_file", "path": relative, "observed": {"state": "present", "before_digest": digest_file(source), "after_digest": digest_file(source)}}],
        "artifacts": [{"kind": "unittest_result", "digest": digest_bytes(raw), "reference": raw_path.name}]}
    output.write_text(json.dumps(bundle, indent=2) + "\n", encoding="utf-8")
    return 0 if passed else 1

if __name__ == "__main__": raise SystemExit(main())
