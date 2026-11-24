"""Measure live A01 first-content latency on warmed connections from a local client.

Set DEEPSEEK_API_KEY, build the binary (or set LOOM_TEST_BIN), and run
python tests/benchmark_first_token.py [--samples 10] [--split-worker].
LOOM_TEST_REDIS_URL has the same meaning as in the HTTP acceptance suite.
Each invocation makes two warmup model calls and two live calls per sample.
Stdout is JSON. All durations are milliseconds.
"""

import argparse
from contextlib import closing, nullcontext
import hashlib
from http.client import HTTPConnection, HTTPSConnection
import json
import os
from statistics import median
import sys
import time

from acceptance_recovery import worker_processes
from harness import Instance


PROMPT = "本次登记号是 QA-715。请只回复登记号。"
MODEL = "deepseek-flash"
HOST = "api.deepseek.com"
WIRE_KEYS = ("role", "content", "name", "tool_calls", "tool_call_id", "reasoning_content")
LOCAL_HEADERS = {"Authorization": "Bearer user-token", "Content-Type": "application/json"}


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode()


def request(conn, method, path, headers, data=None):
    conn.request(method, path, body=data, headers=headers)
    response = conn.getresponse()
    if not 200 <= response.status < 300:
        response.close()
        raise RuntimeError(f"{method} {path} returned HTTP {response.status}")
    return response


def local_json(conn, method, path, data=None):
    with request(conn, method, path, LOCAL_HEADERS, encoded(data) if data is not None else None) as r:
        return json.load(r)


def sse(response):
    require(response.headers.get_content_type() == "text/event-stream", "expected an SSE response")
    deadline = time.monotonic() + 150
    kind, data = "", []
    for line in response:
        require(time.monotonic() < deadline, "SSE deadline exceeded")
        line = line.decode("utf-8").rstrip("\r\n")
        if not line:
            if data:
                yield kind, "\n".join(data), time.perf_counter_ns()
            kind, data = "", []
        else:
            field, _, value = line.partition(":")
            if field == "event":
                kind = value.removeprefix(" ")
            elif field == "data":
                data.append(value.removeprefix(" "))


def payload_for(call):
    payload = {
        "model": call["provider_model"],
        "messages": [{k: m[k] for k in WIRE_KEYS if k in m} for m in call["messages"]],
        "stream": True,
        "stream_options": {"include_usage": True},
    }
    if call["tools"]:
        payload["tools"] = call["tools"]
    return encoded(payload)


def direct_sample(conn, key, payload):
    headers = {"Content-Type": "application/json", "Accept": "text/event-stream, application/json",
               "Authorization": "Bearer " + key}
    start = time.perf_counter_ns()
    first = reasoning = finish = None
    done = False
    parts = []
    with request(conn, "POST", "/chat/completions", headers, payload) as response:
        for kind, data, arrival in sse(response):
            require(kind != "error" and not done, "unexpected direct stream event")
            if data.strip() == "[DONE]":
                done = True
                continue
            chunk = json.loads(data)
            require("error" not in chunk, "DeepSeek returned a stream error")
            for choice in chunk["choices"]:
                if choice.get("index", 0) != 0:
                    continue
                delta = choice["delta"]
                if delta.get("reasoning_content") and reasoning is None:
                    reasoning = arrival
                if delta.get("content"):
                    if first is None:
                        first = arrival
                    parts.append(delta["content"])
                if choice.get("finish_reason"):
                    finish = choice["finish_reason"]
    require(done and finish == "stop" and first is not None and "QA-715" in "".join(parts),
            "invalid direct A01 completion")
    return {"visible_ms": (first - start) / 1e6,
            "reasoning_ms": None if reasoning is None else (reasoning - start) / 1e6}


def loom_sample(conn, agent_id):
    session = local_json(conn, "POST", "/api/sessions", {"agent_id": agent_id})["id"]
    # Anchor the monotonic client stopwatch to the host UTC clock used by the local server.
    before = time.perf_counter_ns()
    wall_us = time.time_ns() / 1000
    start = (before + time.perf_counter_ns()) // 2
    event = local_json(conn, "POST", "/api/events", {"content": PROMPT})["id"]
    delivery_start = time.perf_counter_ns()
    delivery = local_json(conn, "POST", f"/api/events/{event}/deliveries",
                          {"target": {"session_id": session}, "action": "start_run"})
    run_path = f"/api/runs/{delivery['run_id']}"
    first = None
    terminal = None
    pieces = []
    with request(conn, "GET", run_path + "/stream", {**LOCAL_HEADERS, "Accept": "text/event-stream"}) as response:
        for kind, data, arrival in sse(response):
            frame = json.loads(data)
            require(kind != "attempt_discarded", "Loom discarded the streamed attempt")
            if kind == "text_delta" and frame["text"]:
                if first is None:
                    first = arrival
                pieces.append(frame["text"])
            if kind == "run_end":
                terminal = frame["status"]
    require(terminal == "completed" and first is not None, f"invalid Loom run_end: {terminal}")
    run = local_json(conn, "GET", run_path)
    calls = local_json(conn, "GET", run_path + "/model-calls")
    require(len(calls) == 1 and calls[0]["status"] == "completed" and
            calls[0]["provider_model"] == MODEL and calls[0]["finish_reason"] == "stop" and
            run["output"] == "".join(pieces) and "QA-715" in run["output"], "invalid Loom A01 completion")
    timing = calls[0]["timing"]
    admitted, claimed = run["admitted_us"], run["worker_claimed_us"]
    sent, content = timing["request_started_us"], timing["first_content_us"]
    first_us = wall_us + (first - start) / 1000
    require(content is not None and wall_us <= admitted <= claimed <= sent <= content <= first_us,
            "invalid timing order: run all clients and servers on a common host clock")
    ingress = (admitted - wall_us) / 1000
    queue = (claimed - admitted) / 1000
    prepare = (sent - claimed) / 1000
    relay = (first_us - content) / 1000
    return {
        "run_id": run["id"], "model_call_id": calls[0]["id"],
        "visible_ms": (first - start) / 1e6, "delivery_to_visible_ms": (first - delivery_start) / 1e6,
        "event_api_ms": (delivery_start - start) / 1e6, "ingress_ms": ingress, "queue_ms": queue,
        "prepare_ms": prepare, "relay_ms": relay, "self_ms": ingress + queue + prepare + relay,
        "upstream_visible_ms": (content - sent) / 1000,
        "upstream_reasoning_ms": None if timing.get("first_reasoning_us") is None
                                 else (timing["first_reasoning_us"] - sent) / 1000,
        "timestamps_us": {"client_start": round(wall_us), "admitted": admitted, "worker_claimed": claimed,
                          **timing, "client_first_content": round(first_us)},
    }, payload_for(calls[0])


def summarize(rows):
    result = {}
    for name in rows[0]:
        if name.endswith("_ms"):
            values = [r[name] for r in rows if r[name] is not None]
            result[name] = ({"n": len(values), "min": min(values), "median": median(values), "max": max(values)}
                            if values else {"n": 0})
    return result


def benchmark(count, split):
    key = os.getenv("DEEPSEEK_API_KEY")
    require(key and key.strip(), "set DEEPSEEK_API_KEY before running the live benchmark")
    env = {"LOOM_ROLE": "api" if split else "all", "LOOM_WORKERS": "1",
           "LOOM_DEFAULT_MODEL": MODEL, "LOOM_MODEL_ROUTES": "{}",
           "NO_PROXY": "*", "no_proxy": "*"}
    pairs = []
    with (Instance(env=env) as app, worker_processes(app, 1) if split else nullcontext(),
          closing(HTTPConnection("127.0.0.1", app.port, timeout=150)) as loom,
          closing(HTTPSConnection(HOST, timeout=150)) as direct):
        templates = local_json(loom, "GET", "/api/agent-templates")
        template = next(t for t in templates if t["id"] == "standard")
        agent = local_json(loom, "POST", "/api/agents",
                           {"id": "benchmark", "model": {"default": MODEL},
                            "template": {k: template[k] for k in ("id", "source", "version")}})
        _, canonical = loom_sample(loom, agent["id"])
        direct_sample(direct, key, canonical)
        for i in range(count):
            direct_first = i % 2 == 0
            if direct_first:
                direct_row = direct_sample(direct, key, canonical)
            loom_row, payload = loom_sample(loom, agent["id"])
            require(payload == canonical, "provider payload differs between Loom runs")
            if not direct_first:
                direct_row = direct_sample(direct, key, payload)
            pairs.append({"sample": i + 1, "order": "direct-loom" if direct_first else "loom-direct",
                          "direct": direct_row, "loom": loom_row})
    return {"model": MODEL, "deployment": "api + worker" if split else "all", "worker_count": 1,
            "unit": "ms", "samples": count, "warmup_pairs_excluded": 1, "prompt": PROMPT,
            "payload_sha256": hashlib.sha256(canonical).hexdigest(),
            "summary": {k: summarize([p[k] for p in pairs]) for k in ("direct", "loom")}, "pairs": pairs}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--samples", type=int, default=10, help="number of measured A01 pairs (default: 10)")
    parser.add_argument("--split-worker", action="store_true",
                        help="run API and one worker as separate local processes sharing LOOM_HOME")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be at least one")
    try:
        print(json.dumps(benchmark(args.samples, args.split_worker), ensure_ascii=True, indent=2))
    except (RuntimeError, OSError, ValueError, KeyError) as e:
        sys.exit(f"benchmark failed: {e}")
