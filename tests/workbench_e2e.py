"""Run the workbench in Chromium against a fresh HOME and a local model fixture.

Build loom-harness, start Redis (LOOM_TEST_REDIS_URL), and install Playwright
with Chromium. Run python tests/workbench_e2e.py. LOOM_PLAYWRIGHT_MODULE and
LOOM_BROWSER_EXECUTABLE optionally select an existing module and browser.
"""

import json
import os
import subprocess
from pathlib import Path

from acceptance_context import Provider, answer, tool
from harness import Instance


def model(payload):
    messages = payload["messages"]
    text = json.dumps(messages, ensure_ascii=False)
    if any(m["role"] == "user" and m.get("content") == "测试表格渲染" for m in messages):
        return answer("| 步骤 | 产物 |\n| --- | --- |\n| 复核 | **记录** |\n\n```txt\n<b>原文</b>\n```")
    if "Suggest one short next message" in text:
        return answer("再检查一次记录")
    if not any(m["role"] == "tool" for m in messages):
        if "审批笔记测试" in text:
            name = next(
                t["function"]["name"]
                for t in payload["tools"]
                if t["function"]["description"] == "Workstation audit"
            )
            return tool(name, {})
        if "目标验收" in text:
            return tool("goal_complete", {"evidence": "完成目标验收"})
        if "修改自己的描述" in text:
            return tool(
                "code_exec",
                {
                    "code": (
                        "return tools.call('agents.update', "
                        "{id='author',patch={description='聊天已更新描述'}})"
                    )
                },
            )
    return answer("收到图片" if "image_url" in text else "完成")


def run():
    provider = Provider()
    provider.respond = model
    try:
        route = {
            "base_url": f"http://127.0.0.1:{provider.http.server_port}/v1",
            "model": "fixture",
            "images": True,
        }
        with Instance({"LOOM_MODEL_ROUTES": json.dumps({"fixture": route})}) as app:
            app.api("POST", "plugins/inspection/versions", {
                "version": "1",
                "code": """return function(loom)
                    loom.register_tool({id='status', description='Read system status',
                        webui={suggestion={label='  查看 <em>状态</em>  ',
                            prompt='  请列出 <strong>当前状态</strong>。  '}},
                        handler=function() return 'ready' end})
                    loom.register_toolset({id='system', members={'status'}})
                end""",
            }, token="admin-token")
            app.api("POST", "plugins", {
                "id": "inspection", "package": "inspection", "version": "1",
                "enabled": True, "allowed_agents": ["desktop"],
            }, token="admin-token")
            app.api(
                "POST",
                "catalog/tools",
                {
                    "id": "audit",
                    "description": "Workstation audit",
                    "input_schema": {"type": "object", "properties": {}},
                    "exposure": "direct",
                    "requires_approval": True,
                    "allowed_agents": ["desktop"],
                    "code": r"return function(ctx,args) return string.rep('note audit row\n',1000) end",
                },
                token="admin-token",
            )
            app.api(
                "POST",
                "agents",
                {
                    "id": "desktop",
                    "name": "工作助手",
                    "model": "fixture",
                    "tools": [{"toolset": "loom.daily"}, {"tool": "home.audit"}, {"toolset": "inspection.system"}],
                },
            )
            app.api(
                "POST",
                "agents",
                {
                    "id": "author",
                    "name": "创造者",
                    "model": "fixture",
                    "template": {"id": "creator", "source": "loom"},
                    "grants": {"operations": ["agents.update"]},
                },
                token="admin-token",
            )
            subprocess.run(
                ["node", str(Path(__file__).with_name("workbench_browser.cjs"))],
                env={**os.environ, "LOOM_BROWSER_BASE": app.base},
                check=True,
            )
        assert not provider.errors, provider.errors
    finally:
        provider.stop()


if __name__ == "__main__":
    run()
