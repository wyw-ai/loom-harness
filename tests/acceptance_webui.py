"""Exercise the official WebUI plugin from the public HTTP entry point."""

import unittest
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import urlopen

from harness import Instance


class WebUi(unittest.TestCase):
    def test_public_assets_follow_plugin_state_and_data_requires_a_token(self):
        with Instance() as loom:
            assets = {
                "/webui/": "text/html",
                "/webui/app.css": "text/css",
                "/webui/app.js": "text/javascript",
                "/webui/desk.js": "text/javascript",
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
                    "code": "return function(ctx,args) return 'build' end",
                    "allowed_agents": ["reader"],
                },
            )
            self.assertEqual(tool["id"], "home.build%2Elabel")
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
