"""Run the workbench in Chromium against a fresh HOME and a local model fixture.

Build loom-harness and install Playwright with Chromium. Run python
tests/workbench_e2e.py. LOOM_PLAYWRIGHT_MODULE and
LOOM_BROWSER_EXECUTABLE optionally select an existing module and browser.
Set LOOM_WORKBENCH_SCREENSHOTS to a directory to capture the desktop console,
conversation, and mobile navigation using the same isolated test accounts.
"""

import json
import os
import subprocess
from http.server import ThreadingHTTPServer
from pathlib import Path
from threading import Event, Thread

from acceptance_context import Provider, answer, tool
from acceptance_stream import GatedModel
from harness import Instance


class ReleaseCheck(GatedModel):
    first_chunk = "已读取发布队列。"
    last_chunk = "\n\n当前批次的检查均已结束，可以开始发布。"
    release_timeout = 75

    def do_POST(self):
        if self.path == "/release":
            self.server.release.set()
            self.send_response(204)
            self.end_headers()
        else:
            super().do_POST()


def model(payload):
    messages = payload["messages"]
    text = json.dumps(messages, ensure_ascii=False)
    last_user = next((m.get("content") for m in reversed(messages) if m["role"] == "user"), None)
    if last_user == "整理本次发布的交接事项":
        result = answer(
            "发布前有三项交接事项。\n\n"
            "| 检查项 | 交接内容 |\n| --- | --- |\n"
            "| 版本 | 确认计划发布的提交 |\n| 验证 | 汇总已完成的冒烟测试 |\n"
            "| 值班 | 确认发布后的观察人 |\n\n"
            "交接完成后，可以在运行记录中查看执行详情。"
        )
        result["usage"] = {
            "prompt_tokens": 834, "completion_tokens": 168, "total_tokens": 1002,
            "prompt_cache_hit_tokens": 320, "prompt_cache_miss_tokens": 514,
        }
        return result
    if last_user in ("统计第一轮", "统计第二轮"):
        first = last_user == "统计第一轮"
        result = answer("第一轮已完成" if first else "第二轮已完成")
        result["usage"] = {
            "prompt_tokens": 80 if first else 120,
            "completion_tokens": 20 if first else 30,
            "total_tokens": 100 if first else 150,
            "prompt_cache_hit_tokens": 40 if first else 60,
            "prompt_cache_miss_tokens": 40 if first else 60,
        }
        return result
    if any(m["role"] == "user" and m.get("content") == "测试表格渲染" for m in messages):
        return answer("| 步骤 | 产物 |\n| --- | --- |\n| 复核 | **记录** |\n\n```txt\n<b>原文</b>\n```")
    if "Suggest one short next message" in text:
        result = answer("再检查一次记录")
        result["usage"] = {"prompt_tokens":24, "completion_tokens":6, "total_tokens":30}
        return result
    if not any(m["role"] == "tool" for m in messages):
        if "审批笔记测试" in text or last_user == "核对生产发布的审计记录":
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
    stream = ThreadingHTTPServer(("127.0.0.1", 0), ReleaseCheck)
    stream.release = Event()
    serving = Thread(target=stream.serve_forever, daemon=True)
    serving.start()
    try:
        route = {
            "base_url": f"http://127.0.0.1:{provider.http.server_port}/v1",
            "model": "fixture",
            "images": True,
        }
        stream_url = f"http://127.0.0.1:{stream.server_port}"
        routes = {"fixture": route, "stream": {"base_url": stream_url, "model": "stream"}}
        with Instance({"LOOM_MODEL_ROUTES": json.dumps(routes), "LOOM_DEFAULT_MODEL": "fixture"}) as app:
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
            app.api("POST", "agents", {"id": "monitor", "name": "巡检助手", "model": "stream"})
            subprocess.run(
                ["node", str(Path(__file__).with_name("workbench_browser.cjs"))],
                env={**os.environ, "LOOM_BROWSER_BASE": app.base, "LOOM_BROWSER_RELEASE": stream_url + "/release"},
                check=True,
            )
        assert not provider.errors, provider.errors
    finally:
        stream.release.set()
        stream.shutdown()
        stream.server_close()
        serving.join()
        provider.stop()


if __name__ == "__main__":
    run()
