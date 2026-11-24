# 流式首 token 与 Loom 调度耗时

含工具调用的两轮对话另有[逐消息时间线与每轮 token 用量](tool-timeline.md)。

2026-11-24 从本机直连 `deepseek-flash` 测了 20 次。请求发出到收到第一段非空正文的中位数为 **834 毫秒**，范围为 477–1,408 毫秒。把推理分片也算作输出时，第一段模型输出的中位数为 **507 毫秒**。其中 19 次收到了推理分片。面向用户的 Run 流只推送正文。[逐次测量](evidence/first-token.csv)

| 指标（毫秒，中位数） | API 和 worker 同进程 | API 和 worker 分进程 |
| --- | ---: | ---: |
| 用户发消息到客户端收到首段正文 | 815 | 1,048 |
| 用户发消息到 Loom 向模型发出请求 | 14.9 | 240.6 |
| 其中，用户发消息到 Run 入队 | 3.6 | 3.3 |
| 其中，Run 入队到 worker 领取 | 2.3 | 130.0 |
| 其中，worker 领取到发出模型请求 | 9.0 | 17.1 |
| 模型请求发出到 Loom 收到首段正文 | 759 | 723 |
| Loom 收到首段正文到客户端读到 | 13.8 | 22.8 |
| **Loom 自身开销，含首段回传** | **30.6** | **318.4** |

每次从客户端发起 `POST /api/events` 开始计时，经过 Delivery 和 Run，直到客户端从 SSE 读到第一段 `text_delta`。每次 Loom 自身开销等于发出模型请求前的时间加首段回传时间，然后才在 10 次结果中取中位数。各段中位数不能直接相加。同进程自身开销范围为 25–78 毫秒，分进程为 74–480 毫秒。分进程的 worker 空闲时每 150 毫秒检查一次队列，本轮领取前等待的中位数为 130 毫秒。另有四次领取后到发出请求用了 235–305 毫秒，当前计时只能将这部分定位到请求准备阶段。

## 复跑条件

测试使用本机 Windows 上的 release 构建、SQLite 和 Redis Streams 8.10.3，每种部署各有一个 worker，客户端串行发送。每次在新 Session 问同一个简短问题“本次登记号是 QA-715。请只回复登记号。”。每组先预热两个连接，再做 10 对直连和经 Loom 的请求，逐对交换先后顺序。直连复用真实 ModelCall 里的消息、工具定义和模型名，两组的请求体摘要相同。所有测量均排除首次建连和创建 Session 的时间。[基准脚本](../../tests/benchmark_first_token.py)

```powershell
cargo build --release --locked
$env:LOOM_TEST_BIN = (Resolve-Path target/release/loom-harness.exe).Path
python tests/benchmark_first_token.py --samples 10
python tests/benchmark_first_token.py --samples 10 --split-worker
```

在测试终端提供 `DEEPSEEK_API_KEY` 和 `LOOM_TEST_REDIS_URL`。脚本输出每次运行的原始时间戳、耗时和请求摘要。本次测量的 release 二进制 SHA-256 为 `a8df1a8d774253e21adf297e9e154a852d35bcd9882ebfbb4ac10bab762ed512`。
