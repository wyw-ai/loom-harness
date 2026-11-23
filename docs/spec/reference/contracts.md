# 对外接口与插件契约

本页给出首版设计中集成方可以据以开发的行为要求，以及需要在接口定版时核实的细节。当前仓库交付的是设计。归档中的 REST 方法、工具名、事件名和 Rust 组件名为评审方案，尚无可供客户端联调的运行版本。

## 资源管理和事件投递

| 调用方的意图 | 使用的入口 | 结果如何认定 |
| --- | --- | --- |
| 定义或查询 Agent、插件、MCP 连接 | 对应资源的 REST 服务。插件可以登记自己的资源路由，生效接口随安装和启用情况变化 | 读回该资源及生效配置。MCP 连接的配置由 MCP 插件解释 |
| 配置插件 | 统一的 `/api/settings/{plugin}` 资源。宿主负责按插件声明校验配置，并把本次生效值交给插件的 `ctx` | 插件以生效版本工作；应能区分配置已提交与运行侧已应用 |
| 上报、留档或交办 | 上报 Event，再按意图向 Agent 或 Session 定向投递。投递安排支持直接、定时、按条件三种方式，用户可通过 API 配置，也可授权 Agent 管理同一份安排 | 入箱、接收方已处理、Run 已完成是三个可区分的结果。只将资料投给 Session 时可以没有 Run |
| 给运行中的工作补充输入或请求取消 | 通过 Event 投给所属 Session，并关联目标 Run | 收件方核验该 Run 的归属、调用权限及运行状态。调用方按实际处理结果判断是否生效 |
| 使用插件注册的功能 | 插件可开放 REST 服务，也可登记供模型调用的工具。第三方资源可使用自己的路径 | 能力按宿主授予的作用域生效。比如提供任务资源的插件可以在启用后公开任务管理接口 |

投递规则允许使用 Lua 根据本次触发决定是否投、投给谁以及携带什么。用于评审的路径包括 `POST /api/events/{id}/deliveries` 和 `/api/dispatches`。实现时应明确资源所在作用域、可重试操作和已受理工作的查询方式。具体方法和路径见[REST 候选目录](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#rest-api)。

## 插件、工具与上下文

Rust 宿主通过 `PluginProvider` 加载和调用扩展，首版使用 `LuaPluginProvider`。官方与第三方插件共用扩展机制。宿主定义每个扩展点的输入、输出和错误，构造本次调用上下文。插件可通过统一入口订阅获准的事件。一个观察结果的插件和一个负责提交本次模型响应的提供方承担不同职责，宿主须能选定谁的返回值驱动 Run。

| 扩展能力 | 首版契约 |
| --- | --- |
| 推理和输入组装 | 是否发起模型调用、哪些材料进入本轮 message list 是两个独立策略。处理本轮请求时先取得推理决定，再构建输入并使用选定的模型实现 |
| Goal 续跑 | 调用方可在新交办中选择官方 Goal 插件并给出目标和完成条件。插件在交付前可请求原 Run 继续推理，模型核对证据后显式标记目标完成。宿主按 Run 的等待、取消、预算和交付条件执行决定。具体字段和操作名留待接口评审。 |
| 活跃 Goal 的模型输入 | 主模型每次推理都能读到本轮有效的目标和完成条件，使用压缩历史时同样适用。只有 Goal 主动续跑才加入角色为 `user`、来源为 `goal` 的说明；工具回执唤醒时带入对应结果或可回读的引用。消息封装按提供方协议适配，在模型输入和调用记录中标明续跑说明的来源。见[输入组装与留档](../architecture/runtime.md#goal-续跑时模型收到什么)和[压缩规则](../architecture/runtime.md#活跃-goal-与上下文压缩)。 |
| 工具执行 | 可用工具默认经 Code Mode 调用，只有显式选择直接调用的工具进入模型原生工具清单。直接工具仅通过原生入口执行。Code Mode 自身有模型可调用的入口。内置、Lua 和 MCP 来源遵循同一规则 |
| MCP 和 Skill | MCP 由 Lua 插件接入，将远端操作登记为 Tool。支持 Skill 的插件可将名称、简介贡献为 section，并自行提供正文读取入口。首版暂缓官方 `read_skill` |
| 系统指令 | Agent 持有基础 system prompt。工具、工具集及其他插件可注册、刷新、撤出 section。Session 创建时形成初始指令，后续每次模型调用前核对变化 |
| 压缩和分支 | 官方能力覆盖上下文压缩和 `branch`。压缩需要保留对所覆盖历史的追溯；分支从指定历史位置产生独立 Session，可引用原 Event。父 Run 中已经派发的工具及其迟到结果仍按原调用关联 |

Tool 和 Toolset 的具体登记字段、Code Mode 脚本语言、扩展点名称、官方组件的独立分包均可在实现设计中细化。分支切点与压缩复用可对照[归档中的分支契约](../../goal-shaping/archive/loom-agent/2026-11-23-initial/design.md#session-branch)和[压缩流程](../../goal-shaping/archive/loom-agent/2026-11-23-initial/design.md#context-compaction)。

## 模型与交付

首版包含模型选择、按配置进行故障切换、用户图片实际进入模型，以及按交办方要求校验结构化最终结果。建议用同一个样例贯穿接口评审：用户上传失败截图，交办要求返回指定字段的 JSON，首选模型临时不可用。备选应满足图片和输出约束，运行记录应能说明实际模型、尝试情况和交付结果。最终值的校验针对本次交办；一次中间工具调用自身的返回 schema 单独处理。附件上传形式、可切换的故障类别、重试次数与 schema 校验失败后的处理契约仍待定版。[候选接口与可观察结果](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#model-io)

## Agent 操作 harness 的候选接口

下表用于衔接后续接口设计。名称以及权限、包版本和热启用的具体契约均待评审。

| 能力 | 候选入口 | 应能演示的行为 |
| --- | --- | --- |
| 查实例和读指南 | `harness.inspect`、`harness.docs` | 先查当前版本可见的工具或扩展点，再按引用读匹配的示例。官方和第三方插件可在 manifest 登记随包文档，安装时建索引，查阅文档独立于是否已开启该工具集 |
| 按需开启工具 | `tool_access.activate`，Agent 绑定上的 `activation` | Agent 声明可常开或按需开的 Tool/Toolset；获准的临时开启只作用于本 Run。下一次模型请求同步目录、直接工具声明和相关 section，换 worker 后仍可继续 |
| 管理和验证扩展 | `plugins.get/install/set_enabled`、`context.preview` | 读回插件源码与版本，修订、安装到获准范围并停用。可拿历史样本只读试算组装结果，再通过正式运行记录核实实际出站输入。安装包本身不会扩大调用权限 |
| 会话、设置与工具结果 | `sessions.create/branch`、`agents.get/update`、`settings.get/update`、`tool_results.read` | 复用已有资源操作。创建独立 Session 后可以选择是否交办；大型工具结果可按引用分段读取 |

## 完整评审索引

下列链接指向同一次设计快照。八项清单保留了逐项的已定职责、候选名称、可选消费者和分工理由。

| 要核对的问题 | 归档入口 |
| --- | --- |
| 每项数据归属谁，哪些概念需要持久记录 | [领域模型](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#domain-model) |
| REST 资源、方法和候选路径有哪些 | [REST API](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#rest-api) |
| 哪些能力由官方提供，分包是否已定 | [官方插件](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-plugins) |
| 哪些位置支持注册、增强、替换或订阅 | [扩展点](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#extension-points) |
| 原生工具、Code Mode 脚本接口和管理工具分别有哪些 | [官方工具](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-tools) |
| 各事件代表什么事实，由谁发布和消费 | [32 个候选事件名及用途](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-events) |
| 哪个插件订阅什么，为何有些插件由工具或扩展点直接调用 | [插件订阅与原因](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#plugin-subscriptions) |
| Rust 宿主的哪个逻辑位置处理入箱、等待、模型、工具和最终答复 | [宿主消费位置与原因](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#host-subscriptions) |
