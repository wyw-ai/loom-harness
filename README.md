# loom-harness

云端 Agent 服务，以 Session 保存连续对话，以 Run 承接一次交办。HTTP 客户端可以创建 Agent、提交消息、查询模型与工具调用，重连后读回原结果。[首版范围与验收标准](docs/spec/goals/v1.md) · [按用户身份验收](docs/acceptance/README.md)

## 本机运行

准备稳定版 Rust、C 编译器及 Redis 6.2 或更高版本。用 `cargo build --locked` 编译。服务默认监听 `127.0.0.1:7331`，在当前目录的 `.loom` 持久化数据。部署方通过环境注入 API 测试凭据和模型凭据。以下 PowerShell 示例使用当前环境中的 `LOOM_TEST_TOKEN` 和 `DEEPSEEK_API_KEY`。

```powershell
$env:LOOM_AUTH_JSON = @{ ($env:LOOM_TEST_TOKEN) = @{ owner = 'local' } } | ConvertTo-Json -Compress
$env:LOOM_REDIS_URL = 'redis://127.0.0.1:6379/'
cargo run --locked
```

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

Run 会从受理推进到完成或失败，重复查询同一 ID 可取得最终状态。配置 `LOOM_MODEL_ROUTES` 可以使用其他与 Chat Completions 兼容的提供方。例如 `{"local":{"base_url":"http://127.0.0.1:9000","model":"test","images":true}}`。每条路由可用 `api_key_env` 指向该提供方的凭据环境变量，多个模型可用 `LOOM_DEFAULT_MODEL` 选定默认项。设置 `--home` 或 `LOOM_HOME` 选择持久目录，`--role api`、`--role worker` 可在同一主机、同一 HOME 中分别启动入口与执行进程。默认 `all` 同时启动二者。

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
