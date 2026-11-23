# 普通交办与 Goal 主动续跑 · 讨论笔记

2026-11-24，依用户要求将本次结论作为 11 月 23 日首版设计的补充归档。讨论起点是 loom-harness 提交 `d302303`。当时已确定原 Run 可等待真实回执、按交付约定结束，独立 outcome 验收插件暂缓。[上轮设计总览](../2026-11-23-initial/overview.md)

## 对照 Codex 的两个结束条件

核对了 `openai/codex` 提交 `8b02797a2e`。在普通回合中，工具调用可以要求下一次推理。没有待跟进的调用或输入时，宿主进入结束处理。配置的 Stop hook 可以给出继续理由，让同一回合再运行一次。这解释了宿主怎样决定何时交回控制权。[普通回合的判断](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/session/turn.rs#L751-L758) · [Stop hook 的继续分支](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/session/turn.rs#L872-L913)

Codex 将 Goal 按 thread 持久保存。当前回合结束、线程空闲且目标仍有效时，宿主可以发起新回合。模型按续跑提示检查测试、命令输出和产物等证据，确认目标达成后显式更新状态。[新回合的启动条件](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/src/runtime.rs#L466-L501) · [完成前的证据检查](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/templates/goals/continuation.md#L31-L42) · [状态更新工具](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/src/tool.rs#L63-L65)

## 此次采纳的边界

loom-harness 保留普通交办按约交付的默认行为。等待一个已提交作业的回执是 Run 的基本职责。只有交办方在发起时显式启用官方 Goal 插件，Agent 才会为尚未达到的目标主动续跑并依据证据标记完成。由于同一 Session 可以承接多项独立交办，这里将 Goal 的归属定在单个 Run。

宿主继续负责等待、取消、预算与交付。独立 outcome 验收仍暂缓。本轮归档记录设计决定，运行实现与验收留待开发。[本次设计](design.md)

## 续跑请求与模型输入

继续核对同一 Codex 提交。构造续跑请求时，Codex 将当前目标文本作为附加上下文传入，将续跑说明标记为来源 `goal` 的内部片段。该片段进入模型输入时使用 `user` 角色。模型所见的角色与说明的实际来源可以分别记录。[续跑请求与片段来源](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/src/steering.rs#L59-L76) · [内部片段的角色](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/context/internal_model_context.rs#L81-L95)

本轮采纳了 loom-harness 的接续方式。复用原 Run 的内部推理请求 Event，不创建新的 EventDelivery。由 Goal 主动续跑触发推理时，在所选历史之后追加来源为 `goal`、角色为 `user` 的内部说明。该说明只在本次 ModelCall 的输入中可查，不写入用户聊天，也不为其另发 Event。

## 对照压缩与回执后的接续

Codex 的出站请求测试表明，自动续跑可沿用已有的目标上下文。压缩时，宿主按条目标识恢复仍有效的附加上下文，保留下来的是已受输入长度限制的内容。续跑模板提示模型在目标缺失或被截断时回查。对于压缩等原因导致的不可重试或重试耗尽的回合错误，Goal 扩展停止自动推进。[续跑请求中的目标](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/app-server/tests/suite/v2/thread_resume.rs#L4113-L4141) · [恢复有效上下文](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/state/additional_context.rs#L132-L161) · [目标回查说明](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/templates/goals/continuation.md#L1-L8) · [回合出错后的处理](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/ext/goal/src/extension.rs#L422-L439)

Codex 还覆盖了压缩与用户改口同时发生的情况。测试先固定压缩输入，再受理用户清除 Goal，最后核对替换后的历史和持久化记录中仍然保留这次清除。[并发清除 Goal 的测试](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/session/tests.rs#L6060-L6094) · [持久化断言](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/core/src/session/tests.rs#L6136-L6149)

loom-harness 采纳的约定是每次为有效 Goal 构建主模型请求时都从原 Run 取回完整目标和完成条件。压缩后沿用这项要求。若回执唤醒原 Run，模型直接依据回执与目标判断。仅 Goal 主动续跑的轮次追加内部说明。压缩期间受理的改口或取消须在派发前核对，不可恢复的失败停止自动续跑并留下原因。[本次设计](design.md)

## CI 首个受理回执丢失

这次反例把中断提前到 CI 已创建作业、worker 还没保存作业号的时候。核对的 Codex 远端命令路径在启动确认前断线会报告错误，并尝试按进程标识清理。loom-harness 的设计按各外部服务实际支持的查询和幂等能力核实原调用。已将四种恢复分支与迟到、重复回执的验收补入工具层归档，同时明确 Goal 不会因缺少受理回执而新建提交。[Codex 处理启动回包](https://github.com/openai/codex/blob/8b02797a2ea1feaca69242db7e13581553c0da70/codex-rs/exec-server/src/client.rs#L1230-L1261) · [loom-harness 的恢复分支](../2026-11-23-initial/tools.md#lost-acceptance-receipt)
