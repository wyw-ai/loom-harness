"""Exercise the official WebUI plugin from the public HTTP entry point."""

import unittest
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import urlopen

from harness import Instance


class WebUi(unittest.TestCase):
    def test_lua_tool_suggestion_limits(self):
        with Instance() as loom:
            for suggestion in (
                "{label=' ',prompt='ok'}",
                "{label='ok',prompt=12}",
                "{label=string.rep('😀',33),prompt='ok'}",
                "{label='ok',prompt=string.rep('😀',601)}",
            ):
                with self.subTest(suggestion=suggestion), self.assertRaises(HTTPError) as invalid:
                    loom.api("POST", "plugins/hints/versions", {
                        "version": "1",
                        "code": "register_tool({id='read',webui={suggestion=" + suggestion
                            + "},handler=function() return true end})",
                    }, token="admin-token")
                self.assertEqual(invalid.exception.code, 400)
                invalid.exception.close()

    def test_session_previews_after_reload(self):
        with Instance() as loom:
            for agent in ("reader", "writer"):
                loom.api("POST", "agents", {"id": agent})

            def session(agent="reader", **fields):
                return loom.api("POST", "sessions", {"agent_id": agent, **fields})["id"]

            def attach(sid, eid):
                loom.api("POST", f"events/{eid}/deliveries", {
                    "target": {"session_id": sid}, "action": "attach",
                })

            def message(sid, content, token="user-token"):
                attach(sid, loom.api("POST", "events", {"content": content}, token=token)["id"])

            untitled, named, empty = session(), session(title="自定义标题"), session()
            legacy = session("writer")
            foreign = loom.api("POST", "sessions", {}, token="other-token")["id"]
            internal = loom.api("GET", "events?type=agent.system_prompt_updated&agent_id=reader")[0]
            attach(untitled, internal["id"])
            message(untitled, [{"type": "attachment", "attachment_id": "ignored"}])
            message(untitled, " \t\u3000")
            message(untitled, [
                {"type": "text", "text": "  规划\n"},
                {"type": "image_url", "image_url": {"url": "ignored"}},
                {"type": "text", "text": "\u3000执行　\t" + "😀" * 56},
            ])
            message(untitled, "后来补充的信息")
            message(named, "不能取代手写标题")
            # Older/admin-authored user history may have no explicit event.source.
            message(legacy, " 老会话\n原文 ", token="admin-token")

            loom.stop()
            loom.start()
            self.assertTrue(all("first_user_preview" not in item for item in loom.api("GET", "sessions")))
            by_id = {s["id"]: s for s in loom.api("GET", "sessions?agent_id=reader&include=preview")}
            self.assertEqual(set(by_id), {untitled, named, empty})
            self.assertEqual(by_id[untitled]["first_user_preview"], ("规划 执行 " + "😀" * 56)[:48])
            self.assertEqual(by_id[named]["title"], "自定义标题")
            self.assertNotIn("first_user_preview", by_id[named])
            self.assertNotIn("first_user_preview", by_id[empty])
            all_visible = {s["id"]: s for s in loom.api("GET", "sessions?include=preview")}
            self.assertEqual(all_visible[legacy]["first_user_preview"], "老会话 原文")
            self.assertNotIn(foreign, all_visible)

    def test_public_assets_follow_plugin_state_and_data_requires_a_token(self):
        with Instance() as loom:
            assets = {
                "/webui/": "text/html",
                "/webui/app.css": "text/css",
                "/webui/app.js": "text/javascript",
                "/webui/desk.js": "text/javascript",
                "/webui/shell.js": "text/javascript",
                "/webui/usage.js": "text/javascript",
                "/webui/richtext.js": "text/javascript",
                "/webui/chat-flow.css": "text/css",
                "/webui/desk-workspace.css": "text/css",
            }
            for path, content_type in assets.items():
                with urlopen(loom.base + path) as response:
                    self.assertEqual(response.headers.get_content_type(), content_type)
                    self.assertEqual(
                        response.headers["X-Content-Type-Options"], "nosniff"
                    )
                    self.assertTrue(response.read())

            with self.assertRaises(HTTPError) as denied:
                urlopen(loom.base + "/api/agents")
            self.assertEqual(denied.exception.code, 401)
            denied.exception.close()

            agent = loom.api("POST", "agents", {"id": "web-reader"})
            loom.api("PATCH", "plugins/webui", {"enabled": False}, token="admin-token")
            for path in assets:
                with self.assertRaises(HTTPError) as disabled:
                    urlopen(loom.base + path)
                self.assertEqual(disabled.exception.code, 404)
                disabled.exception.close()
            self.assertEqual(loom.api("GET", "agents/web-reader")["id"], agent["id"])

            loom.api("PATCH", "plugins/webui", {"enabled": True}, token="admin-token")
            with urlopen(loom.base + "/webui") as reopened:
                self.assertEqual(reopened.headers.get_content_type(), "text/html")

    def test_management_updates_home_revisions_and_checks_identity(self):
        with Instance() as loom:

            def admin(method, path, body=None):
                return loom.api(method, path, body, token="admin-token")

            self.assertFalse(loom.api("GET", "identity")["admin"])
            self.assertTrue(admin("GET", "identity")["admin"])
            packages = admin("GET", "catalog/packages")
            self.assertTrue(
                any(p["id"] == "internal-tools" and p["official"] for p in packages)
            )
            with self.assertRaises(HTTPError) as denied:
                loom.api("POST", "catalog/toolsets", {"id": "builds", "members": []})
            self.assertEqual(denied.exception.code, 403)
            denied.exception.close()
            tool = admin(
                "POST",
                "catalog/tools",
                {
                    "id": "build.label",
                    "description": "Name this build",
                    "webui": {"suggestion": {"label": "查构建", "prompt": "请检查本次构建。"}},
                    "code": "return function(ctx,args) return 'build' end",
                    "allowed_agents": ["reader"],
                },
            )
            self.assertEqual(tool["id"], "home.build%2Elabel")
            self.assertEqual(tool["webui"]["suggestion"]["label"], "查构建")
            toolset = admin(
                "POST", "catalog/toolsets", {"id": "builds", "members": [tool["id"]]}
            )
            path = "catalog/tools/" + quote(tool["id"], safe="")
            edited = admin(
                "PATCH",
                path,
                {
                    "description": "Read its label",
                    "expected_revision": tool["revision"],
                },
            )
            with self.assertRaises(HTTPError) as stale:
                admin(
                    "PATCH",
                    path,
                    {
                        "description": "Stale edit",
                        "expected_revision": tool["revision"],
                    },
                )
            self.assertEqual(stale.exception.code, 409)
            stale.exception.close()
            with self.assertRaises(HTTPError) as malformed:
                admin("PATCH", path, {
                    "webui": {"suggestion": {"label": "缺少提示"}},
                    "expected_revision": edited["revision"],
                })
            self.assertEqual(malformed.exception.code, 400)
            malformed.exception.close()
            loom.api(
                "POST",
                "agents",
                {"id": "reader", "tools": [{"toolset": toolset["id"]}]},
            )
            loom.stop()
            loom.start()
            self.assertEqual(
                loom.api("GET", "agents/reader/tools")[0]["revision"],
                edited["revision"],
            )
            self.assertEqual(
                admin("GET", path + "/versions/" + tool["revision"])["description"],
                "Name this build",
            )
            admin("PATCH", path, {"webui": {}, "expected_revision": edited["revision"]})
            self.assertFalse(loom.api("GET", "agents/reader/tools")[0].get("webui"))
            self.assertIn(
                toolset["id"],
                [s["id"] for s in admin("GET", "catalog/toolsets") if s["editable"]],
            )
            session = loom.api("POST", "sessions", {"agent_id": "reader"})["id"]
            self.assertEqual(loom.api("GET", f"sessions/{session}/notes")["items"], [])
            with self.assertRaises(HTTPError) as hidden:
                loom.api("GET", f"sessions/{session}/notes", token="other-token")
            self.assertEqual(hidden.exception.code, 404)
            hidden.exception.close()
