# loom-harness

云端 Agent 服务，以 Session 保存连续对话，以 Run 承接一次交办。HTTP 客户端可以创建 Agent、提交消息、查询模型与工具调用，重连后读回原结果。[首版范围与验收标准](docs/spec/goals/v1.md) · [按用户身份验收](docs/acceptance/README.md)

## 本机运行

准备稳定版 Rust、C 编译器及 Redis 6.2 或更高版本。用 `cargo build --locked` 编译。服务默认监听 `127.0.0.1:7331`，在当前目录的 `.loom` 持久化数据。部署方通过环境注入 API 测试凭据和模型凭据。以下 PowerShell 示例使用当前环境中的 `LOOM_TEST_TOKEN` 和 `DEEPSEEK_API_KEY`。

```powershell
$env:LOOM_AUTH_JSON = @{ ($env:LOOM_TEST_TOKEN) = @{ owner = 'local' } } | ConvertTo-Json -Compress
$env:LOOM_REDIS_URL = 'redis://127.0.0.1:6379/'
cargo run --locked
```

### 浏览器工作台

启动服务后打开 [WebUI](http://127.0.0.1:7331/webui/)，用上面配置的用户令牌连接，选择 Agent 并开始对话。令牌仅保留在当前页面内存中。刷新后重新连接即可读回服务端保存的会话。

从「会话」选择 Agent 或最近的对话。输入框里敲 `/` 可以选操作，敲 `@` 可以查当前 Agent 的工具并插入它的实际调用名。空会话显示这个 Agent 能做的事，例如让已接入主机的 Agent「浏览主机 Skill」。选择示例只填入草稿，按发送才会产生消息。Run 进行中可以在原地补充、回答或停止。工具过程折在对应轮次下，展开可查参数和回执。

页面由默认启用的官方 `webui` 插件提供。管理员可以通过 `PATCH /api/plugins/webui` 设置 `{"enabled":false}` 关闭页面入口，已有会话仍保存在服务端。

侧栏的「工作台」管理 Agent、插件、工具和调度。「搜索」（Ctrl K，macOS 上为 ⌘ K）可跳到资源、会话或 OI 接入。资源详情的「与创造者讨论」会把当前对象带入有权限的创造者对话。可授权创造者通过 `agents.update` 修改自身描述，再回 Agent 详情核实新修订。Run 详情列出实际使用的模型消息、工具、回执和时序。官方 `webui@1.0.0`、`1.1.0` 和 `1.2.0` 实例会升级到 `1.3.0`，保留原有启停和访问范围。

工具可以贡献与自己相关的输入建议。在独立工具编辑器中填写建议标题和正文，或在工具定义里声明如下字段。网页从 Agent 的生效工具目录取建议；点击时复核当前授权。这个静态入口与基于上文生成的「推荐」共用草稿框。

```yaml
webui:
  suggestion:
    label: 查构建
    prompt: 请检查本次构建，列出失败步骤和对应日志。
```

Lua 插件在 `loom.register_tool({...})` 的同一张工具声明表中设置 `webui = {suggestion = {label = "查构建", prompt = "请检查本次构建。"}}` 即可。空会话优先展示六条建议，输入 `/` 可以按名称检索其余建议。

聊天输入框下方的会话统计显示累计 token、输出速度、自动压缩和缓存命中率。点开可看输入与输出拆分、缓存命中与未命中的输入 token，以及最近一次组装的上下文距离压缩阈值还有多远。回复旁标有该 Run 的 token 和总耗时。工具等待、审批和交付等待计入 Run 的总耗时。

客户端可分别读取 `GET /api/sessions/{id}/usage` 和 `GET /api/runs/{id}/usage`。会话统计计入模型续跑、自动压缩和输入框推荐采样。推荐单独列出；同一回复的推荐采样不会改变这次 Run 的用量。供应商没有回传完整计数、或旧模型调用缺少计时字段时，页面标明可用范围。

### 接入 Loom OI

在 OIS 发布目录 Service 和工作 Service 后，管理员打开「工作台 → OI 连接」，也可以按 Ctrl K 搜索「接入 OI」。点「连接 Loom OI」，填入 OIS 服务地址、目录 Service 和已配置的凭据引用，保存后即可查看发现的 Service。选择一个 Service，点「连接到 Agent」完成授权与工具集绑定，再点该 Agent 的对话按钮即可试用。已连接实例直接从 Service 列表开始。例如 `office.workbench` 的 `view_on_a` 在模型请求中仍叫 `view_on_a`。读图时请使用支持图片的模型路由（`images: true`）。ToolCall 的 `oi_call_id` 和 `binding.source` 可定位目标主机上的调用。[实例设置、凭据与 Agent YAML](docs/spec/reference/oi-workspace-tools.md)

### 通过 API 发送消息

从另一个终端用同一 `LOOM_TEST_TOKEN` 调用服务。

```powershell
$headers = @{Authorization = "Bearer $env:LOOM_TEST_TOKEN"}
$api = 'http://127.0.0.1:7331/api'
Invoke-RestMethod "$api/agents" -Method Post -Headers $headers -ContentType application/json -Body '{"id":"assistant","template":"standard","model":"deepseek-flash"}'
$session = Invoke-RestMethod "$api/sessions" -Method Post -Headers $headers -ContentType application/json -Body '{"agent_id":"assistant"}'
$event = Invoke-RestMethod "$api/events" -Method Post -Headers $headers -ContentType application/json -Body '{"content":"Reply with QA-715"}'
$delivery = Invoke-RestMethod "$api/events/$($event.id)/deliveries" -Method Post -Headers $headers -ContentType application/json -Body (@{target=@{session_id=$session.id}; action='start_run'} | ConvertTo-Json)
Invoke-RestMethod "$api/runs/$($delivery.run_id)" -Headers $headers
```

模型请求默认启用流式输出。取得 `run_id` 后即可订阅正文，首段到达时 Run 可以仍在执行。

```powershell
curl.exe -N -H "Authorization: Bearer $env:LOOM_TEST_TOKEN" "$api/runs/$($delivery.run_id)/stream"
```

客户端按 `model_call_id` 和 `attempt_id` 拼接 `text_delta.text`。同一模型调用出现新的 `attempt_started` 时替换旧尝试，收到 `attempt_discarded` 时清除该次草稿。断线后用 `Last-Event-ID` 请求头携带最后收到的 SSE ID 续读。收到 `run_end` 后可查询 Run 的最终结果。首段推理与首段正文的到达时间分别记在模型调用的 `timing.first_reasoning_us` 和 `timing.first_content_us`，与 `timing.request_started_us` 一样使用 UTC Unix 微秒。

Run 会从受理推进到完成或失败，重复查询同一 ID 可取得最终状态。配置 `LOOM_MODEL_ROUTES` 可以使用其他与 Chat Completions 兼容的提供方。例如 `{"local":{"base_url":"http://127.0.0.1:9000","model":"test","images":true}}`。每条路由可用 `api_key_env` 指向该提供方的凭据环境变量，多个模型可用 `LOOM_DEFAULT_MODEL` 选定默认项。设置 `--home` 或 `LOOM_HOME` 选择持久目录，`--role api`、`--role worker` 可在同一主机、同一 HOME 中分别启动入口与执行进程。默认 `all` 同时启动二者。

## 从 system prompt 追溯模型请求

Agent 创建或基础 `system_prompt` 的生效文本变化时，Loom 写入一条 `agent.system_prompt_updated` Event，`content` 保存完整指令。API 修改、Agent 自修改和 HOME 应用走同一套规则。将 A 改为 B 再改回 A，会得到三条按生效顺序排列的 Event。

Run 在受理事务中把同一 Agent 当前的基础事件固定为 `Run.system_prompt_event_id`。后续工具回执、Goal 续跑和 worker 重启都使用这个版本。该 Run 的 ModelCall 用 `base_system_prompt_event_id` 记录基础绑定，用 `system_prompt_event_id` 指向当轮完整指令。插件段落或 Goal 等运行时指令使完整文本发生变化时，Loom 写入当前 Run 的 `run.system_prompt_updated`。连续使用同一文本的调用复用同一事件。显式配置为空且没有附加指令时，事件正文为空，发给模型的消息列表省略 system 角色。

| 想查什么 | GET 路径 |
| --- | --- |
| 某个 Agent 的基础指令历史 | `/api/events?type=agent.system_prompt_updated&source=loom&agent_id={agent_id}` |
| 某版基础指令影响的 Run | `/api/runs?system_prompt_event_id={event_id}` |
| 某轮的完整系统指令 | 从 `/api/model-calls/{call_id}` 取 `system_prompt_event_id`，再读 `/api/events/{event_id}` 的 `content` |
| 同一 Run 使用某条完整指令的所有调用 | `/api/runs/{run_id}/model-calls?system_prompt_event_id={event_id}` |

升级已有 HOME 时，Loom 按 Agent 当时生效的修订创建基线。升级前已受理的 Run 继续按原快照工作，已有 ModelCall 可从其 `messages` 回看原文。

## 工具回执和推荐权限

单个 HTTP JSON 请求体上限为 12 MiB。文本工具结果默认超过 8 KiB 就写入 HOME 下的 Note，可用 `LOOM_NOTE_THRESHOLD` 调整。读 Note 的模型单次正文预算由 `LOOM_NOTE_READ_BUDGET` 设置，默认 64 KiB；超出时返回正文大小与行数，调用方可缩小行号范围。

CI 通过 `POST /api/tool-calls/{id}/results` 提交回执，使用来源凭据并携带 `Idempotency-Key`。同一租户有多个 CI 来源时，在 `LOOM_AUTH_JSON` 的来源身份上设置 `source: true` 和 `source_id`，并在对应的 MCP 连接或导入的工具上设置相同的 `receipt_source`。宿主在派发时固定该绑定，其他来源即使知道调用 ID 和作业号也无法完成等待。无来源名称的凭据适用于采用租户级回执权限的旧连接。

输入框推荐可用 `PATCH /api/settings/composer-suggestions` 配置采样。以身份的 `owner` 和 `subject` 为索引，后者默认取 `owner`。例如下列设置允许审查员取用私有材料，每分钟最多采样三次，独立的请求超时为两秒。

```json
{"principals":{"team":{"reviewer":{"enabled":true,"read_private":true,"max_samples_per_minute":3,"timeout_ms":2000}}}}
```

## 验证

启动专用的 Redis 6.2+，安装 Python 3.11+ 和 Node.js 20+。完整验收会启动本机 API 和 worker 进程，调用真实 DeepSeek，并在 CI、模型和下游交付夹具上注入故障。每次运行使用新的证据目录。若使用自定义 Cargo target 目录，以 `LOOM_TEST_BIN` 指向编译得到的可执行文件。

```powershell
$env:LOOM_TEST_REDIS_URL = 'redis://127.0.0.1:6379/'
cargo build --locked
cargo test --locked -- --include-ignored
python tests/run_acceptance.py --evidence-dir "$env:TEMP/loom-acceptance-$(New-Guid)"
```

`DEEPSEEK_API_KEY` 由测试终端的环境提供。测试产物包括每个断言组的结论、可执行文件摘要和脱敏的 HTTP 取证记录。输入框客户端例子在 [examples/composer.mjs](examples/composer.mjs)，上层只在用户点击发送后走正常投递。详细结果见[第一期验收记录](docs/acceptance/results-v1.md)。

本期跨仓联验使用独立 HOME、本机模型夹具、真实 OIS、OIC 和四个已构建的 App。Windows 上从可截图的用户会话运行。将下列环境变量指向对应二进制、App 清单和本地 Playwright 安装目录。测试会在临时目录管理 Service、读取 Skill、截图并验证图片已经送达模型，最后撤下 Service。带 `--browser` 时还会用 Chromium 在页面绑定工具和核对 ToolCall。

```powershell
python tests/oi_workspace_e2e.py --ois $env:OIS_BIN --oic $env:OIC_BIN --management $env:OI_MANAGEMENT_MANIFEST --skills $env:OI_SKILLS_MANIFEST --capture $env:OI_COMPUTER_USE_MANIFEST --filesystem $env:OI_FILESYSTEM_MANIFEST --browser
```

浏览器测试读取 `LOOM_PLAYWRIGHT_MODULE`、`LOOM_BROWSER_EXECUTABLE`，服务测试沿用 `LOOM_TEST_BIN` 与 `LOOM_TEST_REDIS_URL`。可另加 `--live-text` 并设置 `DEEPSEEK_API_KEY`，让 DeepSeek 在单独会话里查找并读取合成 Skill。截图和视觉请求在本机模型夹具内验证。单独运行 `python tests/workbench_e2e.py` 可验收工作台管理、聊天修改自身、审批与 Note，以及窄屏交互。[跨仓验收脚本](tests/oi_workspace_e2e.py) · [工作台浏览器用例](tests/workbench_browser.cjs)
