"""Local black-box API acceptance environment; only uses Python's standard library."""
from __future__ import annotations

import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Instance:
    def __init__(self, env=None, home=None, start=True):
        self.temp = tempfile.TemporaryDirectory(prefix="loom-qa-") if home is None else None
        self.home = Path(home or self.temp.name)
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.env = dict(os.environ)
        self.env.update({
            "LOOM_HOME": str(self.home), "LOOM_BIND": f"127.0.0.1:{self.port}",
            "LOOM_AUTH_JSON": json.dumps({
                "user-token": {"owner": "qa"}, "other-token": {"owner": "another"},
                "admin-token": {"owner": "qa", "admin": True},
                "source-token": {"owner": "qa", "source": True},
                "approve-token": {"owner": "qa", "approve": True},
            }),
            "LOOM_REDIS_URL": os.getenv("LOOM_TEST_REDIS_URL", "redis://127.0.0.1:6379/"),
        })
        self.env.update(env or {})
        self.proc = None
        if start:
            self.start()

    def start(self):
        exe = os.getenv("LOOM_TEST_BIN") or ROOT / "target" / "debug" / ("loom-harness.exe" if os.name == "nt" else "loom-harness")
        self.home.mkdir(parents=True, exist_ok=True)
        with (self.home / "service.log").open("ab") as log:
            self.proc = subprocess.Popen([str(exe)], env=self.env, stdout=log, stderr=log)
        for _ in range(150):
            if self.proc.poll() is not None:
                raise RuntimeError("Loom exited during startup: " + (self.home/"service.log").read_text(errors="replace")[-1000:])
            try:
                if self.api("GET", "/health")["status"] == "ok":
                    return self
            except (OSError, URLError):
                pass
            time.sleep(0.04)
        self.stop()
        raise TimeoutError("Loom did not become ready")

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            self.proc.wait(timeout=10)

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.stop()
        if self.temp:
            self.temp.cleanup()

    def api(self, method, path, body=None, token="user-token", key=None):
        route = path if path.startswith("/") else "/api/" + path
        headers = {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}
        if key:
            headers["Idempotency-Key"] = key
        data = json.dumps(body, ensure_ascii=False).encode() if body is not None else None
        with urlopen(Request(self.base + route, data=data, headers=headers, method=method), timeout=100) as res:
            return json.load(res)

    def send(self, session, message, action="start_run", **fields):
        event = self.api("POST", "/api/events", {"content": message})
        return self.api("POST", f"/api/events/{event['id']}/deliveries",
                        {"target": {"session_id": session}, "action": action, **fields})

    def wait(self, run, statuses=("completed", "failed", "cancelled"), timeout=120):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self.api("GET", f"/api/runs/{run}")
            if result["status"] in statuses:
                return result
            time.sleep(0.08)
        raise TimeoutError(f"waiting for run {run} to reach {statuses}")
