# 投递、回执与恢复实测

2026-11-24 在 Windows 单机使用 Rust 服务、SQLite WAL 和真实 Redis 8.10.3 执行。普通用户与另一所有者通过 HTTP 操作，管理员安装插件，来源身份递交 CI 报告，审批身份单独授权。模型、MCP CI 和通知接收方是独立的本地 HTTP 夹具，可控制其响应时序，并记录真正收到的请求。以下对象均来自隔离测试 HOME。[用例与预期](recovery-and-dispatch.md)

执行基线为 `99f942ce968b` 加本轮工作树。投递脚本本轮的服务二进制 SHA-256 为 `86e94fcfc09dae1a988520106614e34fef6e0a1aadd8dfeda2b2c12dd28b77f9`。11 个投递与恢复 HTTP 测试一轮通过，用时 38.628 秒。另行执行了 CI 首回执、3 个 Redis 故障与订阅 HTTP 测试（15.297 秒）、固定窗口负载测试，以及含真实 Redis 回收用例的 17 个 Rust 测试。不同测试独立创建数据。[投递脚本](../../tests/acceptance_recovery.py) · [CI 脚本](../../tests/acceptance_catalog.py) · [总线脚本](../../tests/acceptance_bus.py)

| 用例 | 实测事实与可核对标识 |
| --- | --- |
| R01 直接投递 | 事件 `evt_f5bb685e632442598c8c85e98b644176` 按各自原键重投后有 2 笔 Delivery。归档 `del_19e036df2864435290b40e21fe0e88a7` 新增 0 个 Run，交办 `del_6c6c536a7fb449bc94c1f7d7c53cf7ad` 产生 1 个完成的 Run。向另一所有者的 Session 投递被拒，对方见到 0 条收件。立即执行的 Dispatch 重复提交也沿用同一个安排，只投递一次。 |
| R02 定时投递 | HTTP 创建上海工作日 09:00 安排 `dispatch_74c7b7d928b746008ee306d81b5e12e4`。受控调度时钟在 11 月 30 日周一 08:59 留下 0 笔投递，09:00 产生原 Run。停服务、把测试库游标回拨到同一到期点再启动，重放仍只有 1 个 Evaluation、1 笔投递和 1 个 Run。周二暂停，独立的归档定时器证实调度器已越过 09:00，原安排不新增收件。周三 08:59 恢复，当日 09:00 第二次正常交办。两个触发键的 Unix 秒分别为 `1796000400` 和 `1796173200`，修订也可独立追溯。 |
| R03 来源、规则和权限 | 失败样例的 Lua 预览返回两项意图，收件为 0。CI 成功报告跳过，抛错留原因，失败报告重送后归档、交办各 1 笔且 Run 仅 1 个。其他来源不触发。改规则定向另一所有者后，对应求值 `eval_17a7c847a9144867932f4e94c6bb05a4` 失败，对方查到 0 条收件，原历史仍带旧修订。 |
| R04 首受理回执丢失 | 远端已建作业且本地尚无作业号时停止服务，经真实租约到期换进程。能查的适配器用原键取回作业、提交 1 次；只支持同键重交的提交 2 次，键、参数和 ToolCall 相同；无保证的普通 Run `run_48f513b7661c46488aa5e82e0deb91b1` 停在 `unknown`，等迟到首回执。三种普通 Run 各只建 1 个作业。另测已保存作业号、显式 Goal 与保证已过期。六组各以不同键重送 E421 最终报告，各唤醒一次、分析一次、交付一次。[T12 和 R04 的逐项 Run 与外部计数](results-tools.md#t12-与-r04-的接收方计数) |
| R05 回执早到或并发 | 两次均为 Code Mode 的一个父调用加配置、CI 两个子调用。首组在首次提交 HTTP 响应尚未返回时已递交最终失败报告。次组换进程后由 CI 的授权适配器实际 GET 作业 T42，与直接 webhook 同时递交，再重送一次。每组均建 1 个 CI 作业，父调用完成 1 次，后续分析 1 次；出站消息含配置和 E421。次组 ToolCall 为 `tool_call_fc5eacc341704d9e9f22148661834f43`。 |
| R06 已落库、尚未发布 | 对 `evt_54c338546d0c46d6bc04f4fbafed0eb5`，RESP 代理在首次 XADD 前截住发布，校验 SQLite 已提交、outbox 的 published=0、Redis 对应条目 0、Run 仍等待，然后停服务。重启后使用原 Event，报告事实、外部作业、后续分析、审计各 1 次。 |
| R07 已消费、尚未确认 | 事件 `evt_1925907fa0c24e26afa254c89c83de9d` 的首次 Stream ID 为 `1795510182734-0`。运行时已提交，代理在 XACK 前阻断，原条目仍 pending。重启后先放行审计，再补不同 ID `1795510183711-0`。最终两组的 pending 和 lag 都为 0，各记 1 次业务处理。CI 作业、后续分析和审计结果各 1。 |
| R08 执行池 | 默认单 worker 下，A 等 CI 时 B 先执行完成，释放报告后原 A 继续。拆开入口与执行进程后，不同 worker 的独立步骤有重叠。重复所有回执后每组仍只有 8 次后续分析。双连接同步争抢同一可领版本的 Rust 测试也只得到 1 个新租约，旧租约提交被拒。实测指标见下表。 |
| R09 补充与取消 | 对等待中的原 Run 补充“只分析本次失败”，报告前仍等待，真正的分析输入含该补充和 E421，只建原作业 1 个。错 Session 取消返回 404；正确取消后发两份迟到报告，`tool_call_552f3b7bef4d4b6f9e7668a8047b8396` 可查到 E421，模型和 CI 侧没有新增派发。在另一用例中，已开始的模型响应迟到也无法越过取消。 |
| R10 审批 | 三个待审批 Run 都在外部执行为 0 时重启。改参数的审批返回 409，无权审批返回 403。相同批准重投只执行 1 次，ToolCall 为 `tool_call_ba37362579ca4e08882eb02b70b924ae`；拒绝与取消后旧批准均执行 0 次，后者返回 409。拒绝分支的原 ToolCall 为 failed，后续实际模型输入收到 approval denied。另一个存储测试覆盖批准先到、等待稍后登记的时序。 |
| R11 外发送达 | 一份通过的 CI 报告以结构化成功结果结束，通知为 0。三份失败报告分别另投给归档 Session，那里新增 Run 为 0。每次均在通知接收方已入账、发送方尚未存确认时停进程。能查时 POST 1、GET 1；只能幂等时同标识 POST 2；无保证时 POST 1、GET 0，`reply_b01283badc95491b992f722e358a24b1` 保持 `waiting_delivery` 且送达结果为 `unknown`。每条路径有效通知 1、CI 作业 1，恢复交付没有再次调用模型。 |
| R12 插件订阅 | 实际安装的 Lua 插件观察 `evt_8ad57d95dcb34e6da2c3805e630f6bd7`。同一事实的两个 Stream 条目仅产生 1 条审计结果。同类型但来源越界为 0，停用后新增结果为 0，业务 Delivery 与 Run 均为 0。Rust 测试将相同的总线契约替换为进程内适配器，验证重复发布、丢确认和授权过滤。 |
| R13 上下文插件替换 | 同一个模型和审计订阅分别跑两个真实 Lua 构建器。原交办在两次出站请求中都存在，只有第二个构建器选入 CI-CTX-42。模型端保存的消息与对应 ModelCall 相同。每次各 1 个模型请求、1 个完成审计、1 份最终答复。第二次 ModelCall 为 `model_call_511508d44e3a48cda1b044aa1f07d4ba`。 |

## R08 负载记录

每组同一个 API 地址、一个独立入口进程、1/2/4 个单 worker 程序，共用该组 HOME。先让 8 个独立 Run 等待 CI，再用 8 路并发释放 8 份回执并各重送 1 次。外部模型延迟固定为 40 毫秒。固定窗口从该组第一份回执获宿主接纳起算，纳入随后 500 毫秒内成功提交的独立步骤。时延按全部 8 份回执的接纳时刻配对同一 Run 步骤的启动时刻，采用最近秩 p95。

| worker 进程 | 固定窗口秒数 | 窗口内独立提交 | 窗口内步骤/秒 | 完成全批秒数 | 全批独立步骤 | 时延样本 | p95 毫秒 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 0.500 | 4 | 8 | 0.787 | 8 | 8 | 648 |
| 2 | 0.500 | 6 | 12 | 0.633 | 8 | 8 | 480 |
| 4 | 0.500 | 6 | 12 | 0.572 | 8 | 8 | 385 |

该表是一轮单机、短突发负载的记录。8 个样本的最近秩 p95 为其中的最大值。[执行池指标定义](../spec/architecture/runtime.md#worker-pool)

## 待验证分支

- R12 的部署适配器只有 Redis Streams。进程内替身已通过契约测试，换用第二种可部署总线后重新跑 R06、R07 仍待可用适配器。[替换条件](../spec/architecture/runtime.md#event-distribution)

## 复跑

准备隔离 Redis 6.2 或更新版本，把 `LOOM_TEST_REDIS_URL` 设为该实例。从仓库根目录运行以下 PowerShell。脚本各自启动 HTTP 服务并清理测试 HOME，输出业务标识和接收方计数。R02 指定部署侧 `LOOM_DISPATCH_CLOCK_FILE`，以原子替换写入 UTC Unix 秒。该文件只向调度器提供当前时间，租约、持久化时间戳和超时继续使用正常时钟。没有配置该变量时使用系统时间。

```powershell
cargo build --locked --target-dir target/verified
$env:LOOM_TEST_BIN = (Resolve-Path target/verified/debug/loom-harness.exe).Path
$env:LOOM_TEST_BINARY = $env:LOOM_TEST_BIN
python tests/acceptance_recovery.py
python tests/acceptance_bus.py
Push-Location tests
python -m unittest -v acceptance_catalog.CatalogAcceptance.test_06_t12_lost_first_receipt_after_worker_replacement
Pop-Location
cargo test --locked --target-dir target/verified -- --include-ignored
```
