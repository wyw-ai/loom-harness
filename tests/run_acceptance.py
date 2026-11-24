"""Run all Phase 1 API cases; optionally write sanitized HTTP and test evidence."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import unittest

from harness import ROOT


class Results(unittest.TextTestResult):
    def startTest(self, test):
        os.environ["LOOM_TEST_CASE"] = test.id()
        self.started = time.monotonic()
        super().startTest(test)

    def stopTest(self, test):
        super().stopTest(test)
        self.checks.append(
            {
                "test": test.id(),
                "seconds": round(time.monotonic() - self.started, 3),
                "passed": self.wasSuccessfulFor(test),
            }
        )

    def wasSuccessfulFor(self, test):
        return all(
            item is not test
            for entries in (self.errors, self.failures, self.skipped)
            for item, _ in entries
        )

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.checks = []


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    for name in ("DEEPSEEK_API_KEY", "LOOM_TEST_REDIS_URL"):
        if not os.getenv(name):
            parser.error(f"{name} is required for the complete run")
    default = (
        ROOT
        / "target"
        / "debug"
        / ("loom-harness.exe" if os.name == "nt" else "loom-harness")
    )
    binary = Path(os.getenv("LOOM_TEST_BIN", default)).resolve()
    if not binary.is_file():
        parser.error(f"build the server first: {binary}")
    os.environ["LOOM_TEST_BIN"] = os.environ["LOOM_TEST_BINARY"] = str(binary)
    if args.evidence_dir:
        dest = args.evidence_dir.resolve()
        dest.mkdir(parents=True, exist_ok=True)
        log = dest / "http.jsonl"
        if log.exists():
            parser.error("choose a fresh evidence directory for each run")
        os.environ["LOOM_TEST_EVIDENCE"] = str(log)
    suite = unittest.defaultTestLoader.discover(
        str(ROOT / "tests"), pattern="acceptance_*.py"
    )
    results = unittest.TextTestRunner(verbosity=2, resultclass=Results).run(suite)
    ui = subprocess.run(
        ["node", "--test", "tests/composer.test.mjs"], cwd=ROOT, check=False
    )
    if args.evidence_dir:
        with binary.open("rb") as exe:
            digest = hashlib.file_digest(exe, "sha256").hexdigest()
        (dest / "results.json").write_text(
            json.dumps(
                {
                    "binary_sha256": digest,
                    "tests": results.checks,
                    "skipped": len(results.skipped),
                    "composer_passed": ui.returncode == 0,
                },
                indent=2,
            ),
            encoding="utf-8",
        )
    return (
        0
        if results.wasSuccessful() and not results.skipped and ui.returncode == 0
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
