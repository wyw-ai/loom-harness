"""Trace each admitted Run and each provider request to its immutable system prompt."""

import json
import sqlite3
import unittest
from contextlib import closing
from urllib.error import HTTPError

from acceptance_context import Provider, answer, tool
from harness import Instance


class PromptEvents(unittest.TestCase):
    def setUp(self):
        self.provider = Provider()
        self.addCleanup(self.provider.stop)
        route = {
            "model": "fixture",
            "base_url": f"http://127.0.0.1:{self.provider.http.server_port}/v1",
        }
        self.app = Instance({
            "LOOM_DEFAULT_MODEL": "fixture",
            "LOOM_MODEL_ROUTES": json.dumps({"fixture": route}),
        })
        self.addCleanup(self.app.__exit__, None, None, None)
        self.api = self.app.api

    def base_events(self, agent_id, token="user-token"):
        return self.api("GET", f"events?type=agent.system_prompt_updated&agent_id={agent_id}",
                        token=token)

    def test_unchanged_system_message_reuses_its_event_across_tool_rounds(self):
        self.api("POST", "agents", {"id": "reader", "tools": ["agents.get"]})
        base, = self.base_events("reader")
        self.assertEqual(base["content"], "You are a helpful assistant.")
        for fields in ({}, {"result_schema": {"type": "string"}}):
            with self.subTest(fields=fields):
                responses = iter([
                    tool("code_exec", {"code": "return tools.call('agents.get', {})"}),
                    answer('"done"'),
                ])
                self.provider.respond = lambda _: next(responses)
                sid = self.api("POST", "sessions", {"agent_id": "reader"})["id"]
                run_id = self.app.send(sid, "read the current agent", **fields)["run_id"]
                run = self.app.wait(run_id, timeout=15)
                self.assertEqual(run["status"], "completed", run.get("error"))
                first, second = self.api("GET", f"runs/{run_id}/model-calls")
                effective_id = first["system_prompt_event_id"]
                self.assertEqual(second["system_prompt_event_id"], effective_id)
                effective = self.api("GET", f"events/{effective_id}")
                for request in self.provider.calls[-2:]:
                    self.assertEqual(request["messages"][0]["content"], effective["content"])
                changes = self.api("GET", f"events?type=run.system_prompt_updated&run_id={run_id}")
                if fields:
                    self.assertNotEqual(effective_id, base["id"])
                    self.assertEqual([event["id"] for event in changes], [effective_id])
                else:
                    self.assertEqual(effective_id, base["id"])
                    self.assertEqual(changes, [])
        self.assertFalse(self.provider.errors)

    def test_explicit_empty_prompt_keeps_a_link_when_system_role_is_absent(self):
        self.api("POST", "agents", {"id": "empty", "system_prompt": "", "tools": []})
        base, = self.base_events("empty")
        self.assertEqual(base["content"], "")
        sid = self.api("POST", "sessions", {"agent_id": "empty"})["id"]
        run = self.app.wait(self.app.send(sid, "ping")["run_id"], timeout=15)
        call, = self.api("GET", f"runs/{run['id']}/model-calls")
        self.assertEqual(run["status"], "completed")
        self.assertEqual(call["system_prompt_event_id"], base["id"])
        self.assertNotIn("system", [m["role"] for m in self.provider.calls[0]["messages"]])

    def test_legacy_waiting_run_resumes_from_its_snapshot_without_rewriting_history(self):
        self.api("POST", "agents", {"id": "legacy", "system_prompt": "pre-upgrade instructions"})
        sid = self.api("POST", "sessions", {"agent_id": "legacy"})["id"]
        self.provider.respond = lambda _: tool("ask_uuser", {"question": "Which ticket?"})
        rid = self.app.send(sid, "process the ticket")["run_id"]
        self.assertEqual(self.app.wait(rid, ("waiting", "failed"), timeout=15)["status"], "waiting")
        question, = self.api("GET", f"runs/{rid}/tool-calls")
        self.api("PATCH", "agents/legacy", {"system_prompt": "current instructions"})
        self.app.stop()

        # Only in this stopped, disposable HOME: reproduce the previous schema,
        # where catalog revisions and Run snapshots existed but prompt IDs did not.
        with closing(sqlite3.connect(self.app.home / "state" / "loom.db")) as db, db:
            db.execute("DROP TABLE agent_prompt_heads")
            db.execute("""UPDATE objects
                          SET doc=json_remove(doc,'$.system_prompt_event_id','$.base_system_prompt_event_id')
                          WHERE id=? OR parent=? OR json_extract(doc,'$.run_id')=?""", (rid, rid, rid))
            events = """SELECT id FROM objects WHERE kind='event' AND json_extract(doc,'$.type')
                        IN ('agent.system_prompt_updated','run.system_prompt_updated')"""
            db.execute(f"DELETE FROM outbox WHERE event_id IN ({events})")
            db.execute(f"DELETE FROM objects WHERE kind='event' AND id IN ({events})")

        self.app.start()
        current, = self.base_events("legacy")
        self.assertEqual(current["content"], "current instructions")
        archived, = self.api("GET", f"runs/{rid}/model-calls")
        self.assertIsNone(archived.get("system_prompt_event_id"))
        self.assertEqual(self.api("GET", f"events?type=run.system_prompt_updated&run_id={rid}"), [])
        self.provider.respond = lambda _: answer("resumed")
        self.app.send(sid, "T42", action="add_input", run_id=rid, question_id=question["id"])
        resumed = self.app.wait(rid, timeout=15)
        self.assertEqual(resumed["output"], "resumed")
        self.assertIsNone(resumed.get("system_prompt_event_id"))
        historical, continuation = self.api("GET", f"runs/{rid}/model-calls")
        self.assertEqual(historical, archived)
        self.assertIsNone(continuation.get("base_system_prompt_event_id"))
        actual = self.provider.calls[-1]["messages"][0]["content"]
        self.assertEqual(actual, "pre-upgrade instructions")
        self.assertEqual(self.api("GET", f"events/{continuation['system_prompt_event_id']}")["content"], actual)
        self.assertEqual(self.api("GET", f"runs?system_prompt_event_id={current['id']}"), [])

    def test_effective_edits_from_api_and_home_survive_restart(self):
        agent = self.api("POST", "agents", {"id": "helper", "system_prompt": "A"})
        initial, = self.base_events("helper")
        self.assertEqual(initial["agent_revision"], agent["revision"])
        self.assertEqual(initial["content"], "A")

        self.api("PATCH", "agents/helper", {"system_prompt": "A"})
        self.api("PATCH", "agents/helper", {"description": "new description"})
        self.assertEqual(self.base_events("helper"), [initial])

        updated = self.api("PATCH", "agents/helper", {"system_prompt": "B"})
        with self.assertRaises(HTTPError) as stale:
            self.api("PATCH", "agents/helper", {
                "system_prompt": "rejected", "expected_revision": agent["revision"],
            })
        self.assertEqual(stale.exception.code, 409)
        stale.exception.close()

        # Restore A via HOME after the API accepted B. Both A events must survive.
        spec = self.app.home / "agents" / "helper" / "agent.yaml"
        spec.write_text(json.dumps({**updated, "system_prompt": "A"}), encoding="utf-8")
        self.api("POST", "catalog/apply", {}, token="admin-token")
        history = self.base_events("helper")
        self.assertEqual([row["content"] for row in history], ["A", "B", "A"])
        self.assertEqual(len({row["id"] for row in history}), 3)
        self.assertEqual(history[1]["agent_revision"], updated["revision"])
        self.api("POST", "catalog/apply", {}, token="admin-token")
        self.app.stop()
        self.app.start()
        self.assertEqual(self.base_events("helper"), history)
        trigger = self.api("POST", "events", {"content": "check the restored version"})
        dispatch = self.api("POST", "dispatches", {
            "trigger": {"type": "direct", "event_id": trigger["id"]},
            "rule": {"type": "fixed", "action": "start_run", "target": {"agent_id": "helper"}},
        })
        delivery, = self.api("GET", f"dispatches/{dispatch['id']}/deliveries")
        run = self.app.wait(delivery["run_id"], timeout=15)
        self.assertEqual(run["status"], "completed")
        self.assertEqual(run["system_prompt_event_id"], history[-1]["id"])
        self.assertEqual(self.api("GET", f"events/{initial['id']}"), initial)

    def test_old_run_pins_base_and_records_each_actual_model_prompt(self):
        for version in ("1", "2"):
            code = (
                "return function(loom) loom.register_section("
                f"{{id='guide',text='guide-{version}'}}) end"
            )
            self.api("POST", "plugins/guide/versions", {"version": version, "code": code},
                     token="admin-token")
        self.api("POST", "plugins", {
            "id": "guide", "package": "guide", "version": "1", "enabled": True,
            "allowed_agents": ["assistant"],
        }, token="admin-token")
        self.api("POST", "agents", {"id": "assistant", "system_prompt": "original-base"})
        sid = self.api("POST", "sessions", {"agent_id": "assistant"})["id"]
        responses = iter([
            tool("ask_uuser", {"question": "Which build?"}),
            answer("new Run completed"),
            answer("original Run resumed"),
        ])
        self.provider.respond = lambda _: next(responses)
        old_id = self.app.send(sid, "ask me for the build")["run_id"]
        waiting = self.app.wait(old_id, ("waiting", "failed"), timeout=15)
        self.assertEqual(waiting["status"], "waiting", waiting.get("error"))
        question, = self.api("GET", f"runs/{old_id}/tool-calls")

        self.api("PATCH", "agents/assistant", {"system_prompt": "updated-base"})
        self.api("PATCH", "plugins/guide", {"version": "2"}, token="admin-token")
        original, updated = self.base_events("assistant")
        self.app.stop()
        self.app.start()
        new_id = self.app.send(sid, "a separate request")["run_id"]
        new = self.app.wait(new_id, timeout=15)
        self.assertEqual(new["output"], "new Run completed")
        self.assertEqual(new["system_prompt_event_id"], updated["id"])

        self.app.send(sid, "T42", action="add_input", run_id=old_id, question_id=question["id"])
        old = self.app.wait(old_id, timeout=15)
        self.assertEqual(old["output"], "original Run resumed")
        self.assertEqual(old["system_prompt_event_id"], original["id"])
        old_first, old_next = self.api("GET", f"runs/{old_id}/model-calls")
        new_call, = self.api("GET", f"runs/{new_id}/model-calls")
        observations = [
            (old_first, original, "guide-1"),
            (new_call, updated, "guide-2"),
            (old_next, original, "guide-2"),
        ]
        self.assertEqual(len(self.provider.calls), len(observations))
        for request, (call, base, section) in zip(self.provider.calls, observations):
            self.assertEqual(call["base_system_prompt_event_id"], base["id"])
            effective_id = call["system_prompt_event_id"]
            effective = self.api("GET", f"events/{effective_id}")
            sent, = [m["content"] for m in request["messages"] if m["role"] == "system"]
            self.assertEqual(effective["content"], sent)
            self.assertEqual(call["messages"][0]["content"], sent)
            self.assertIn(base["content"], sent)
            self.assertIn(section, sent)
            query = f"system_prompt_event_id={effective_id}"
            affected_run, = self.api("GET", f"runs?{query}")
            self.assertEqual(affected_run["id"], call["run_id"])
            actual_call, = self.api("GET", f"runs/{call['run_id']}/model-calls?{query}")
            self.assertEqual(actual_call["id"], call["id"])
        self.assertNotEqual(old_first["system_prompt_event_id"], old_next["system_prompt_event_id"])
        for base, rid in [(original, old_id), (updated, new_id)]:
            affected = self.api("GET", f"runs?system_prompt_event_id={base['id']}")
            self.assertEqual([run["id"] for run in affected], [rid])
        self.assertFalse(self.provider.errors)

    def test_agent_and_owner_scope_and_internal_prompt_authority(self):
        self.api("POST", "agents", {"id": "first", "system_prompt": "first base"})
        self.api("POST", "agents", {"id": "second", "system_prompt": "second base"})
        first, = self.base_events("first")
        second, = self.base_events("second")
        self.assertNotEqual(first["id"], second["id"])
        for token in ("user-token", "source-token"):
            with self.assertRaises(HTTPError) as spoof:
                self.api("POST", "events", {
                    "type": "agent.system_prompt_updated", "agent_id": "first",
                    "content": "untrusted replacement",
                }, token=token)
            self.assertIn(spoof.exception.code, (400, 403))
            spoof.exception.close()

        sid = self.api("POST", "sessions", {"agent_id": "first"})["id"]
        delivery = self.app.send(sid, "execute", system_prompt_event_id=second["id"])
        run = self.app.wait(delivery["run_id"], timeout=15)
        self.assertEqual(run["status"], "completed")
        self.assertEqual(run["system_prompt_event_id"], first["id"])
        self.assertIn("first base", self.provider.calls[0]["messages"][0]["content"])
        self.assertEqual(self.base_events("first", token="other-token"), [])
        with self.assertRaises(HTTPError) as foreign:
            self.api("GET", f"events/{first['id']}", token="other-token")
        self.assertEqual(foreign.exception.code, 404)
        foreign.exception.close()
        rows = self.api("GET", f"runs?system_prompt_event_id={first['id']}", token="other-token")
        self.assertEqual(rows, [])


if __name__ == "__main__":
    unittest.main()
