# DSH 与 Loom 的网页交互对照

2026-11-25，在 Windows 本机用 Chromium 操作官方 `@deepseek-ai/dsh@0.2.2-rc.5` 和 Loom WebUI。DSH 使用独立的 `DSH_HOME` 与演示工作区，界面显示 `DeepSeek-V4.1-Flash · High`。Loom 使用同一把 DeepSeek API key 和 `deepseek-flash` 路由，分别检查 `webui@1.1.0` 和 `1.2.0`。观察对象是从输入、执行到回看的完整操作，耗时受路由、上下文长度及缓存影响。

## 用同一道题看回复

在两端的新会话分别发送「请用一个两列 Markdown 表格，列出规划、执行、复核三个步骤和各自的产物。最后写一句建议。」

| 浏览器中看到的结果 | DSH | Loom 1.1.0 | Loom 1.2.0 |
| --- | --- | --- | --- |
| 表格和正文 | 表头、两列内容及最后的建议可以分别阅读 | 回复中能看到 Markdown 管道符，消息正文的 `table` 元素数量为 0 | 消息正文生成 1 个 `table`，三项产物可以逐行对照，建议在表格之后 |

这个用例查的是最终消息的 DOM。模型生成了正确文本，页面仍可能把结构丢掉。新版 Loom 有一次在回答表格题前额外调用了 `list_skills`，可在同一条回复上方展开核对。两端的 Agent 和工具配置各异，单次耗时不具有可比性。Loom 的浏览器用例也用固定的模型回复复查渲染，使日后的检查可以稳定重跑。[工作台浏览器用例](../../tests/workbench_browser.cjs)

可逐张对照同一道题在三个网页上的实际显示。[DSH 网页](evidence/webui/dsh-answer.png) · [Loom 1.1 网页](evidence/webui/loom-1.1-answer.png) · [Loom 1.2 网页](evidence/webui/loom-1.2-answer.png)

## 一次工具调用怎样呈现

在 DSH 默认工作区放入只有一行标记的 `comparison-note.txt`，从网页发出「请读取当前工作区中的 comparison-note.txt，仅回复其中的标记。」发送约 350 ms 后的截图里，刚发的消息已经出现在对话中，侧栏把这段会话标为「进行中」，同一轮的下方开始显示活动。工具实际读回文件中的标记。结束后答案单独成段，前面只留「已完成，用时 1秒」。展开后看到「已读取文件」，还可以沿该项查看细节。该轮在 DSH 页面上记为 1 轮 2 步。

Loom 1.2.0 把执行过程按 Run 放在对应的消息旁。读者无需先去「运行」列表猜哪一次调用属于这条答复。要核实时，可以从那一轮进入 ToolCall 与原文。多轮聊天时，先前一轮的过程仍归在原处。OI 用例沿这个入口核对 ToolCall 的固定来源、OI 调用号和图片结果。[跨仓浏览器用例](../../tests/oi_workspace_browser.cjs)

## 输入框和插件怎样配合

| 实际操作 | DSH 的处理 | Loom 的改动与验收 |
| --- | --- | --- |
| 在输入框键入 `/` | 在光标附近出现文件、Goal、Skill 等编入当前产品的动作。选定后继续当前工作 | `/` 查找工作台动作，`@` 查找当前 Agent 已获授的工具。选择工具把名字填到草稿，用户仍能编辑并决定是否发送。键盘用例分别选择工具和 Goal。 |
| 打开一个已经接上 OI 的 Agent | DSH 的输入框就近展示所选工作区、Agent 模式、模型及访问权限 | 1.1.0 中即使 Agent 已能读取远端 Skill，空会话还只给出「回顾会话」「查找笔记」等通用示例。1.2.0 按该 Agent 的实际工具生成入口。工具还可声明 `webui.suggestion.label` 和 `prompt`。点击建议填入草稿。 |
| 打开插件页，再使用能力 | DSH 的「添加插件」可选择「安装第三方插件」或「让 Agent 创建插件」。同一个任务入口既能进入安装表单，也能进入创造模式。清单用功能名称和一句说明展示，实验功能可在行内启用 | Loom 将 OI 发现结果、工具集绑定和进入目标 Agent 对话连在一起。管理员可从已经发现的 Service 选择 Agent，一个流程完成授权、绑定并尝试对话。复查时确认仍保有 Agent 原来的授权，普通用户访问同一管理接口仍收到 403。 |

在 Loom 已经聊了两轮时，从第一条回复可以复制原文、打开当轮 Run，或点「从此处分支」。新会话保留截至所选回复的完整历史。用户从「工作台 → OI 连接」进入已授权 Agent，用同一把 DeepSeek Key 调用 `list_skills` 和 `read_skill`，读回了测试主机上的原文「织机已就绪」。展开工具后先看到实际文本，再点「查看原始结果」可核对 OI 回执。[已发现的 Service](evidence/webui/loom-oi-services.png) · [实际读取 Skill 的过程](evidence/webui/loom-oi-skill.png)

Loom 的目录负责决定当前用户和 Agent 可见的工具。插件提供的提示文字用于生成界面草稿，实际派发仍走原 ToolCall 权限检查。安装页、工具目录和会话因而可以复用同一份生效数据。[工作台验收](../../tests/workbench_e2e.py) · [OI 工具映射与授权](../spec/reference/oi-workspace-tools.md)

## 复现入口与验证范围

1. 在空工作目录设置独立的 `DSH_HOME`，从测试进程环境提供 `DEEPSEEK_API_KEY`，启动 `npx --yes @deepseek-ai/dsh@0.2.2-rc.5 web --host 127.0.0.1 --port 7308 --no-open`。用该进程给出的本地登录地址打开网页。首次会显示预览版说明，进入默认工作区后即可发送上述题目。本次未安装 Office 预览和离线语音的可选组件。[官方运行说明](https://github.com/deepseek-ai/deepseek-harness)
2. 按仓库启动步骤打开 Loom 的 `/webui/`，选择 Agent，在新会话中发送同一表格题。要复查工具提示，先为该 Agent 授予并绑定一个包含 Skill 工具的 OI Toolset，再试空会话、`@`、选择建议和发送。撤权后重新进入，核对可用工具范围。[本机运行与验证](../../README.md)
3. `python tests/workbench_e2e.py` 用真实 Harness 和 Chromium 配合本地模型夹具，复查富文本、草稿、快捷动作、审批、会话切换及 Agent 自修改。跨仓脚本的 `--browser` 使用真实 OIS、OIC、App 与浏览器，验证绑定、读图和撤下 Service 后的目录及历史。另加 `--live-text` 会用真实 DeepSeek 自行选择工具读取演示 Skill。私有 Service 凭据失败、目录分页失败、刷新竞争等可重复故障由单独的 HTTP 夹具注入。[跨仓运行参数](../../tests/oi_workspace_e2e.py) · [内部工具故障用例](../../tests/acceptance_internal_tools.py)
