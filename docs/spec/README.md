# loom-harness 首版规范

loom-harness 是一个云端 Agent 与上下文管理服务。外部应用可定义 Agent、向 Agent 或 Session 投递交办，并在异步工具返回后取回同一项工作的结果。Rust 宿主维护执行状态，Lua 插件扩展推理、工具和上下文行为。

这里记录首版已经确认的设计要求，运行能力仍待实现和验收。首次阅读可从[整体设计总览](../design-review.md)入手，沿一项交办查看领域模型、REST、插件、工具、事件与订阅，再对照 HOME 中的目录和 YAML。技术参考中标有“候选”的名称、接口形状和分工供实现评审。

| 读者要解决的问题 | 从哪里开始 |
| --- | --- |
| 如何编写 Agent spec、添加独立 Tool，或配置同一插件实例的多个连接？ | [HOME 资源规范](reference/home.md)和[配置生效与恢复的验收](goals/v1.md#home-acceptance) |
| 怎样从官方或第三方 AgentTemplate 创建普通 Agent 或创造者？怎样验证两个角色的权限与新插件的实际效果？ | [模板、包清单与插件配置](reference/home.md#agent-templates)和[角色、授权与真实调用验收](goals/v1.md#agent-template-acceptance) |
| 怎样将一个自定义 Lua 工具和一个 MCP 工具组成同一 Toolset，如何核对实际派发？ | [混合工具集配置](reference/tool-sources.md#mixed-toolset)和[专项验收](goals/v1.md#mixed-toolset-acceptance) |
| 各官方插件的代码、配置和 YAML 分别放在哪个目录？ | [逐包目录树与设置示例](../design-review.md#packages) |
| 怎样验收普通交办、Goal 续跑、图片输入、故障切换及结构化结果？ | [目标与验收](goals/v1.md) |
| 第一期怎样算完成，验收人怎样模拟用户操作 API 并核对真实结果？ | [第一期完成标准](goals/v1.md#phase-one-acceptance)和[API 验收用例与执行说明](../acceptance/README.md) |
| Session、Run、Event 分别属于谁？不同调用怎样关联？ | [领域概念](concepts/domain.md) |
| 哪些状态必须能恢复？哪些能力适合做成插件？ | [设计原则](principles/design.md) |
| Run 何时交付、等待或主动续跑？历史压缩后，工具回执如何唤醒原 Run？ | [运行架构与时序](architecture/runtime.md) |
| 一个逻辑入口如何从默认 1 个 worker 扩容？如何比较 1、2、4 个 worker 的表现？ | [入口与可调执行池](architecture/runtime.md#worker-pool)和[单机压测验收](goals/v1.md#worker-pool-acceptance) |
| 事件发布前停机怎么补发？Redis Streams 重送同一事件时，两个独立订阅如何各自处理？ | [事件分发与替换总线](architecture/runtime.md#event-distribution)和[分发故障验收](goals/v1.md#event-distribution-acceptance) |
| 首个工具回执丢失后，换 worker 怎样找回原作业？ | [工具恢复能力](reference/contracts.md#外部工具的恢复能力) |
| 大段文本工具结果怎样保存为 Note？Session 和 Event 怎样按同一引用读取日志原文或指定行？ | [Note 的写入、检索与读取](reference/contracts.md#notes)和[日志回读验收](goals/v1.md#notes-acceptance) |
| 聊天应用怎样取得输入框推荐？用户先发了新消息时如何处理？ | [上层调用契约](reference/contracts.md#输入框推荐)、[宿主只读采样](architecture/runtime.md#输入框推荐的只读采样)和[验收场景](goals/v1.md#输入框推荐) |
| 外部应用和插件作者可以依赖哪些契约？接口与事件名确定到什么程度？ | [接口、工具和事件参考](reference/contracts.md) |

需要追溯设计取舍时，阅读[当前设计共识](../goal-shaping/current/loom-agent/design.md)。[首轮归档总览](../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md)保留了首轮接口和运行职责清单。[首轮讨论笔记](../goal-shaping/archive/loom-agent/2026-11-23-initial/notes.md)记录采纳、暂缓和未决事项。后续确认的普通交办结束条件及 Goal 插件职责见[2026-11-24 补充归档](../goal-shaping/archive/loom-agent/2026-11-24-goal-continuation/design.md)。
