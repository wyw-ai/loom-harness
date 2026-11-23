# loom-harness 首版规范

loom-harness 是一个云端 Agent 与上下文管理服务。外部应用可定义 Agent、向 Agent 或 Session 投递交办，并在异步工具返回后取回同一项工作的结果。Rust 宿主维护执行状态，Lua 插件扩展推理、工具和上下文行为。

这里记录首版已经确认的设计要求，作为后续开发的基线。2026-11-23 首次归档时，仓库只包含设计文档，运行能力仍待实现和验收。技术参考中标有“候选”的名称、接口形状和分工供实现评审。

| 读者要解决的问题 | 从哪里开始 |
| --- | --- |
| 怎样验收普通交办、Goal 续跑、图片输入、故障切换及结构化结果？ | [目标与验收](goals/v1.md) |
| Session、Run、Event 分别属于谁？不同调用怎样关联？ | [领域概念](concepts/domain.md) |
| 哪些状态必须能恢复？哪些能力适合做成插件？ | [设计原则](principles/design.md) |
| Run 何时交付、等待或主动续跑？历史压缩后，工具回执如何唤醒原 Run？ | [运行架构与时序](architecture/runtime.md) |
| 外部应用和插件作者可以依赖哪些契约？接口与事件名确定到什么程度？ | [接口、工具和事件参考](reference/contracts.md) |

需要追溯设计取舍时，阅读[当前设计共识](../goal-shaping/current/loom-agent/design.md)。[首轮归档总览](../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md)保留了首轮接口和运行职责清单。[首轮讨论笔记](../goal-shaping/archive/loom-agent/2026-11-23-initial/notes.md)记录采纳、暂缓和未决事项。后续确认的普通交办结束条件及 Goal 插件职责见[2026-11-24 补充归档](../goal-shaping/archive/loom-agent/2026-11-24-goal-continuation/design.md)。
