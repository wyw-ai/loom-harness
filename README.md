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

## 验证

`cargo test --locked` 运行模块测试。启动专用测试 Redis 后，用 `LOOM_TEST_REDIS_URL` 指定其地址。设置 `DEEPSEEK_API_KEY`，运行 `python tests/acceptance_conversation.py -v` 会启动独立的本机实例，以真实模型完成一次对话，重启后核对答复和调用记录。用例是否通过以[实跑记录](docs/acceptance/README.md#本轮执行记录)为准。
