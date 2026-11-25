# loom-harness 整体设计评审稿

这份总览从一个 Agent 的配置出发，说明谁接收交办、发起推理、执行工具和接回结果，以及定义与运行记录各放在哪里。仓库目前交付的是设计。HOME 的文件归属、资源修订和装载规则见 [HOME 资源规范](spec/reference/home.md)，各主题的权威入口见[规范阅读索引](spec/README.md)。

| 阅读标记 | 含义 |
| --- | --- |
| **已定职责** | 已写入现行 spec 的行为、归属或边界。 |
| **归档候选** | 已有设计稿给出的名称、路由、扩展点或组件分工。依然可以调整。 |
| **HOME 设计与示例** | 服务实例目录、各类资源的归属、独立工具和配置生效语义已归档。下文给出可供评审的 YAML 键名、插件分包、内部文件和配置值。 |

**按问题查阅**

| 问题 | 入口 |
| --- | --- |
| 有哪些领域对象，怎么关联？ | [领域模型](#domain) |
| 普通 Agent 能查哪些资料？创造者如何给自己添加工具并验证？ | [Agent 模板与配置](spec/reference/home.md#agent-templates) · [真实调用验收](spec/goals/v1.md#agent-template-acceptance) |
| 客户端和插件能调用哪些 HTTP 接口？ | [REST API](#rest) |
| 官方提供哪些插件或能力，每个包和实例什么样？ | [官方能力清单](#official) · [逐插件文件布局与配置](#packages) |
| 开发者在哪一步扩展宿主？ | [扩展点](#extensions) |
| 模型和 Code Mode 可以使用哪些官方工具？ | [工具与脚本接口](#tools) |
| CI 报告太长或历史已压缩时，怎样找回原文？ | [Session Note 的三个工具](spec/reference/contracts.md#notes) · [Note 在 HOME 中的位置](spec/reference/home.md#notes-storage) |
| 哪些事实有事件，各自有什么作用？ | [事件类型](#events) |
| 哪个插件、哪个宿主组件接什么事件，为什么？ | [插件订阅](#plugin-consumers) · [宿主消费位置](#host-consumers) |
| HOME 根目录放什么，哪些东西需要持久保存？ | [HOME 目录与配置生效](#home) |
| 怎样把本地 Lua 和 CI 的 MCP 工具组合成同一 Toolset？ | [配置与调用](spec/reference/tool-sources.md#mixed-toolset) · [混合工具集验收](spec/goals/v1.md#mixed-toolset-acceptance) |
| Agent、独立 Tool、Dispatch 的 YAML 长什么样？ | [资源示例](#resources)。包声明、实例、Settings、模型和 MCP 连接等见[插件示例](#packages)。 |

**从一次交办看全局。** 用户要检查发布标签并读取 CI 报告。客户端先上报用户输入，再把它投给发布 Agent。宿主受理交办后创建 Run。推理触发策略决定是否调用模型，上下文构建器选择材料，模型执行器调用符合约束的模型。模型既能用团队放在 HOME 的本地工具，也能用 MCP 插件导入的 CI 工具。两者走同一套 ToolCall、授权、回执和恢复规则。普通交办在所需结果齐备后按约定交付。显式启用 Goal 时，原 Run 可以跨轮继续，直至满足完成条件或有明确停止原因。答复送达后，上层还可单独请求一条输入框建议。

```text
配置    Agent 定义 → Tool / Toolset 绑定 → 本次推理实际可用工具
输入    Event → EventDelivery → Inbox → 按意图创建或接续 Run
执行    Run → 推理决定 → ModelCall → ToolCall → 回执 → 原调用方
交付    Run → 校验并保存答复 → 按约定交付 → Run 终态
```

设计依据见[领域规范](spec/concepts/domain.md)、[运行与恢复](spec/architecture/runtime.md)、[接口契约](spec/reference/contracts.md)、[初始归档](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md)和[Goal 补充归档](goal-shaping/archive/loom-agent/2026-11-24-goal-continuation/design.md)。

<a id="domain"></a>

## 领域模型

例如，CI 构建失败时，来源上报一个 **Event**。同一事件可以投给资料 **Session** 留档，再投给值班 **Agent** 排查。两个目标各有自己的收件记录，只有排查产生 **Run**。排查跨越几轮推理、几个工具或几台 worker，都沿用这个 Run 的身份。[领域概念](spec/concepts/domain.md) · [投递与消费](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#event-consumer)

| 对象 | 归属、关系与可观察结果 |
| --- | --- |
| Agent、Session、Run | Agent 定义承担工作的身份、简介、基础 system prompt 和能力。Session 是连续交办的会话身份，也是可独立收件的地址。每个 Run 只属于一个 Session，由选定的 Agent 执行一项交办，保存当前等待与交付约定。同一 Session 可积累多个 Run，也可只收资料。 |
| Goal、等待与交付 | 交办方可在创建交办时启用官方 Goal 插件，指定目标与完成条件。目标归原 Run，模型依据证据显式标记完成，宿主处理取消、预算与最终交付。普通交办也能等工具回执、审批或外部作业。登记等待后可释放 worker。跨 Run 独立管理目标留待后续。 |
| Event、EventSource、Dispatch | Event 表示一次已经发生的事实。EventSource 是已配置的数据来源。Dispatch 保存直接、定时或来源触发的投递安排及其规则。一次触发可跳过、失败或产生多项投递，求值记录保留对应的输入及规则版本。 |
| EventDelivery、Inbox、插件订阅 | 每个 EventDelivery 将一个 Event 投给一个目标 Agent 或 Session。Inbox 保存目标已收件的记录。目标根据意图留档、发起交办或接续原 Run。插件可各自观察同一事实，独立记录消费进度。EventSource、Dispatch、EventDelivery 均为评审用名。 |
| ModelCall、ToolCall、外部回执 | ModelCall 关联一次推理决定、当轮实际输入、模型尝试和结果。每个 ToolCall 有自己的身份。Code Mode 外层调用和内部子调用分别记录。工具返回受理回执时，把作业号等依据归到原调用。最终回执满足该 Run 的等待后才继续。 |
| 上下文、分支、压缩、附件 | 每轮 message list 由获准使用的 Event、图片、工具结果等材料按序组装。子 Session 从指定切点引用已冻结的历史前缀，后续独立交办。原 Run 保留在途调用与等待。压缩产物记录覆盖范围及来源，各次 ModelCall 留下实际采用的版本。 |
| Note | 属于一个 Session 的文本材料，保存简介、创建时间和正文。宿主接纳大文本工具结果时为原 ToolCall 的内容块保存一份 Note，相关事件、会话历史条目和结果详情共用同一引用。Agent 也可主动记下需要回读的文字。[Note 契约](spec/reference/contracts.md#notes) |
| Tool、Toolset、Skill、Section | Tool 有参数与结果，Toolset 组织工具及其共用说明。默认工具经 Code Mode 调用，显式直接调用的工具通过模型原生入口执行。Skill 可先提供简介，再按需读取正文。Section 由贡献方注册、刷新或撤出，与 Agent 基础指令共同形成当轮生效的 system prompt。 |
| Plugin、Settings、ctx | Rust 宿主通过 PluginProvider 加载扩展，首个实现为 LuaPluginProvider。插件可管理自己的资源，MCP 连接属于 MCP 插件。Settings 是插件声明的配置，ctx 是宿主在一次调用时提供的身份、作用范围和生效配置。插件包安装、在目标范围启用、某个 Run 能看见哪些工具，是可分别核对的状态。 |

领域对象与 REST 资源可以有不同的粒度。例如，输入框推荐只读采样用户有权查看的会话材料。用户采纳后可以改写，只有实际发送才产生 EventDelivery。一次模型调用的输入可供授权查询，调用方无需为每份 message list 单独创建资源。[首版行为与验收](spec/goals/v1.md) · [接口与插件契约](spec/reference/contracts.md)

领域模型落到文件时，新增的设计对象是包版本、部署实例、可编辑资源和生效修订。它们与运行对象的存储对应关系见 [HOME 目录](#home)。

<a id="rest"></a>

## REST API

当前仓库交付设计，尚无可联调的服务。下表合并了历史 REST 目录和后续确认的输入框推荐设计。**首版职责**指已经确认需要支持的行为。表列 HTTP 方法、路径与字段仍是**候选契约**。Settings 的统一资源路径已确定。MCP 和任务资源另标为插件示例，实际实例能提供哪些路由取决于插件启用和授权。[对外契约](spec/reference/contracts.md#资源管理和事件投递) · [原始 REST 目录](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#rest-api)

### 实例、Agent 与插件管理

| 资源 | HTTP 方法和路径 | 职责 |
| --- | --- | --- |
| Agent | `GET /api/agents`、`POST /api/agents`、`GET /api/agents/{id}`、`PATCH /api/agents/{id}` | 列表、创建、详情、更新。定义可包含模型选择与长期工具绑定。 |
| 插件 | `GET /api/plugins`、`POST /api/plugins`、`GET /api/plugins/{id}`、`PATCH /api/plugins/{id}` | 登记和查询插件，修改启用状态或选择包版本。 |
| 插件版本 | `GET /api/plugins/{id}/versions`、`POST /api/plugins/{id}/versions`、`GET /api/plugins/{id}/versions/{version}` | 列出、提交或读取确定版本，以及获准读取的源码。 |
| 配置及 schema | `GET /api/settings/{plugin}`、`PATCH /api/settings/{plugin}`、`GET /api/settings/{plugin}/schema` | 查询生效值、提交修改、发现字段约束。资源路径已定，方法及 schema 子路径待定版。需区分保存成功与运行侧应用成功。 |
| 模型、文档 | `GET /api/models`、`GET /api/docs`、`GET /api/docs/{doc_ref}` | 发现获准使用的模型和能力，检索可见指南，按引用读取匹配版本。 |
| 实例接口发现 | `GET /api/openapi.json` | 候选发现接口，描述实例已生效的路由。插件路由若发生冲突，保留已有绑定。 |
| 插件资源示例 | `POST /api/mcp/connections`、`POST /api/task`、`GET /api/task/{id}` | MCP 插件定义连接配置，任务插件示例创建、查询任务。归档示例使用单数路径 `/api/task`。 |

### 事实、来源、投递与运行控制

| 资源 | HTTP 方法和路径 | 职责 |
| --- | --- | --- |
| 事件 | `POST /api/events`、`GET /api/events`、`GET /api/events/{event_id}` | 上报、按条件和游标读取、查单条。提交事实可暂时没有目标。打开关联大文本结果的事件，可按权限看到原内容块的状态、预览与 Note 引用。 |
| 事件的定向投递 | `POST /api/events/{event_id}/deliveries`、`GET /api/events/{event_id}/deliveries` | 给一个 Agent 或 Session 投递已知事件，或按事件查询各目标的送达和处理结果。 |
| 单次投递、重试 | `GET /api/event-deliveries/{delivery_id}`、`POST /api/event-deliveries/{delivery_id}/attempts` | 查询收件与消费结果，针对可重试的失败重新尝试原投递。 |
| 来源类型与实例 | `GET /api/event-source-types`、`GET /api/event-sources`、`POST /api/event-sources`、`GET /api/event-sources/{id}`、`PATCH /api/event-sources/{id}` | 发现获准配置的类型，创建、查询、修改或停用来源。 |
| 投递安排 | `POST /api/dispatches`、`GET /api/dispatches`、`GET /api/dispatches/{id}`、`PATCH /api/dispatches/{id}`、`GET /api/dispatches/{id}/deliveries` | 创建、查询、更新安排并追查实际投递。候选设计用状态字段暂停、恢复或取消安排。已发生的投递有各自记录。 |
| 安排的预览与求值 | `POST /api/dispatches/{id}/previews`、`GET /api/dispatches/{id}/evaluations` | 以样例只读试算，或查询正式触发时采用的规则及其结果。预览不会写入目标 Inbox。 |

**交办和控制。** 新交办优先经事件投递，以候选意图 `start_run` 授权创建工作。给已有 Run 补充输入或取消只能走事件投递，目标为所属 Session，并关联原 `run_id`，候选意图分别为 `add_input`、`request_cancel`。宿主核验调用权限、归属及状态。查询接口只读。入箱、收件方处理成功、Run 最终完成各有独立结果。[控制边界](spec/reference/contracts.md#资源管理和事件投递)

**一次实际调用的候选写法。** 上报 CI 报告取得 `evt_42` 后，对 `POST /api/events/evt_42/deliveries` 发送下列正文。此处 `attach` 仅归档。归档建议用 `Idempotency-Key` 绑定此次意图，受理时返回 `202 Accepted` 和 `delivery_id`。若还需值班 Agent 排查，可以用同一个 Event 向该 Agent 再投递一次，分别追查两份结果。[事件投递示例](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#event-delivery)

```json
{"target":{"session_id":"session_review"},"action":"attach"}
```

### 会话、上下文、运行与调用记录

| 资源 | HTTP 方法和路径 | 职责 |
| --- | --- | --- |
| 会话 | `POST /api/sessions`、`GET /api/sessions`、`GET /api/sessions/{id}` | 创建或查询会话。创建会话、向会话交办分别授权。 |
| 会话历史和分支 | `GET /api/sessions/{id}/context/items`、`POST /api/sessions/{id}/branches` | 从已提交的历史条目读取不透明切点，按切点创建子 Session。例如 `{"at":"cursor_42"}` 表示包含该位置。含 Note 的条目保留原引用，回读时仍校验来源权限。 |
| 上下文预览 | `POST /api/sessions/{id}/context/previews` | 用授权样本只读试算输入组装。正式调用实际使用的输入从 ModelCall 查询。 |
| 输入框推荐 | `POST /api/sessions/{session_id}/suggestions` | 可选 Lua 插件的只读采样。候选请求 `{"after_reply_id":"r42","expected_version":"v18"}`。返回 `based_on_version` 及字符串或 `null` 的 `suggestion`。过期、撤权时不交付旧文本。用户发送才按普通输入投递。 |
| Run 及其结果 | `GET /api/sessions/{id}/runs`、`GET /api/runs/{id}` | 查某会话的工作，或追踪一次交办的状态、交付结果和失败原因。 |
| 工具可见性 | `GET /api/agents/{id}/tools`、`GET /api/runs/{id}/tools` | 分别查 Agent 绑定和默认可用性、当前 Run 有效的工具。 |
| 模型与工具调用记录 | `GET /api/runs/{id}/model-calls`、`GET /api/model-calls/{id}`、`GET /api/runs/{id}/tool-calls` | 查询历次模型调用、某轮可披露的真实输入或引用，以及外层工具和子调用明细。 |
| 工具结果与回执 | `GET /api/tool-calls/{id}/results/{part}/content`、`POST /api/tool-calls/{id}/results` | 结果详情可携带文本 Note 的稳定引用。前者供客户端按授权读取已保存的结果块，包括二进制内容。后者是外部工具协议适配的候选写入口。也可上报关联原调用的 Event，经核验后才接回等待方。 |
| 用户附件 | `POST /api/attachments`、`GET /api/attachments/{id}/content` | 上传或按权限读取附件。输入 Event 可引用图片，宿主在模型调用时装配。上传格式仍待定。 |

**待定与后续。** 独立创建 Run、通用对外订阅、分支历史的其他选材和编辑操作，以及 Run 临时开启工具的通用 REST 入口仍待评审。`tool_access.activate` 是候选的 Run 内工具名。Goal 的启用和完成操作、模型重试与故障切换、附件上传形式和结构化结果校验失败后的处理，均有行为要求，字段或协议细节留待协商。独立工具管理和显式应用资源的 HTTP 方法与路径仍待定。[归档目录与待定项](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#rest-api) · [模型与交付](spec/reference/contracts.md#模型与交付) · [输入框推荐契约](spec/reference/contracts.md#输入框推荐)

<a id="official"></a>

## 官方插件与能力

读下面这张表时要区分三件事：产品承诺什么能力、运行时由谁负责、发布时拆成几个包。下面 17 个名称用来把能力映射到后面的目录例子。正式发行可以合包。

| 成熟度 | 建议包 ID | 主要职责 |
| --- | --- | --- |
| 官方职责已明确 | `system-prompt` | 组合 Agent 基础指令与可用 section，随会话初始化和刷新。 |
| 两个可独立替换的默认策略 | `inference-trigger`、`context-builder` | 前者决定已经有一个 Run 时是否需要推理。后者为该次决定组装输入。 |
| 默认能力，分包待定 | `model-provider`、`tool-registry`、`code-mode` | 适配模型请求与切换、登记和解析工具、执行模型提交的工具程序。Rust 宿主掌握授权、尝试接纳和调用派发。 |
| 官方能力已有设计 | `compaction`、`branch` | 压缩已选历史、按切点派生独立 Session。 |
| 可选官方插件 | `goal`、`composer-suggestions` | Goal 在显式启用的工作上请求原 Run 继续。输入框推荐应客户端请求读取快照，为用户拟一条草稿。 |
| 接入契约已有设计，交付形态待定 | `mcp`、`skills` | 连接 MCP Server 并导入工具。按需展示 Skill 简介和正文。 |
| 官方实现分工候选 | `dispatch`、`http-source`、`timer-source` | 管理投递安排和求值、适配外部请求、提供时间触发。Rust 宿主负责持久入箱及推进。 |
| 结果预览插件候选 | `tool-results` | 选择工具结果的上下文预览。文本 Note 的持久化由宿主负责。 |
| 创造者自管理能力已定，分包候选 | `harness-admin` | 提供实例自检、随包文档和获准的管理工具，支持创造者为自己增加工具并实际调用。 |

Agent 定义、Run 状态机、存储、权限、模型执行器、工具分派器与回执路由均有宿主职责。独立 HOME Tool 可以由工具管理插件装载。Task 与用量统计是演示扩展机制的例子，Webhook 是可扩展的来源示例，独立判断目标是否正确的 outcome 插件暂缓。[官方能力归档](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-plugins) · [插件与宿主边界](spec/architecture/runtime.md#rust-宿主与-lua-插件)

<a id="extensions"></a>

## 扩展点

| 扩展位置 | 插件提供什么、宿主何时调用 | 组合方式和约束 |
| --- | --- | --- |
| 插件加载、依赖与具名服务 | 包声明入口、依赖的服务和提供的契约。宿主启用时解析，首版以 `LuaPluginProvider` 装入 worker。 | 按作用域和版本装配，依赖缺失、路由或执行绑定冲突时报告。 |
| 事件订阅 | 声明类型、来源或作用范围，宿主向每条匹配订阅提供已登记的事实。 | 可有多个消费者，各自有消费进度。 |
| `model.should_invoke` | 为新交办、已受理的补充、就绪的等待给出等待或推理建议。 | 由宿主按 Run 版本采纳，同一就绪状态只推进一次。 |
| `context.build` 与上下文变换 | 以决定 ID 和材料截止位置选择历史、附件、待处理工具、分支前缀、压缩结果、当前 Goal。 | 按显式顺序变换，产出一次可追溯输入或构建错误。 |
| `model.before_invoke` / `model.invoke` / `model.after_invoke` | 发送前适配请求，选定 provider 执行，完成后供观察或记账。 | 前处理可有顺序，本次尝试只有一个执行者。结果观察可有多个。 |
| Tool / Toolset 登记、工具执行 | 插件、MCP 来源或 HOME 加载器提供 ID、描述、schema、入口、处理函数。 | 全局按命名空间登记，每次调用只绑定一个执行者。Toolset 保存成员引用。 |
| `tool.before_dispatch`、外部操作恢复 | 前者是派发前策略候选。适配器说明可按句柄或稳定键查询、同键重试的条件。 | 派发前由宿主核对当前权限、绑定和参数，可按审批结果恢复原 ToolCall。 |
| Section 注册、刷新、撤出 | 工具集、Skill 等贡献有版本的系统指令片段，System Prompt 负责组合。 | 按来源和 ID 定位，下一次主模型请求前核对适用版本。 |
| Settings、资源和 REST 注册 | 包声明配置 schema、REST 方法及路径、参数、权限与处理函数。可把同一业务操作登记为 Agent Tool。 | Settings 按实例校验并注入 `ctx`。每条生效的路由有明确所有者，REST 和 Tool 走同一资源权限。 |
| Agent 模板与配置 | 插件可声明提供的模板 YAML，也可用 `agent_config_schema` 声明自己消费的字段。 | 创建时按消费实例校验，配置随 Agent 修订保存。每个插件实例只从 `ctx.agent_config` 取得该 Agent 分给自己的值，与实例 Settings 分开。 |
| 随包文档注册 | manifest 指向指南与示例。安装或升级时宿主建立版本索引，模型可以按引用读取。 | 完整正文按需读取。查阅权限独立于工具集是否已临时开启。 |
| 来源与触发器登记 | 来源声明输入类型、身份和事件字段。时钟等触发器提供稳定的发生标识。 | 来源、手工提交、到期回调均可进入安排求值。来源可被多个安排引用。 |
| Dispatch 规则 | 固定规则或 `decide(ctx)` 根据一次触发返回零到多项投递意图。 | 宿主持久记录结果并逐项履行。脚本超时、求值失败与成功跳过可区分。 |
| Goal 交付前决策与完成标记 | 普通答复已可评估时，Goal 可请求继续。模型依据证据显式标记目标完成。 | 复用原 Run 的等待和推理机制。插件钩子名、模型提交完成标记的操作名待定。 |
| 只读采样 | 推荐插件提供提示与采样请求。宿主只开放读取获准快照和一次有额度、超时约束的模型采样。 | 返回文本或空值。工具请求直接作废。前后校验会话版本与调用者权限。 |
| HOME 资源装载（扩展点签名待定） | 资源所有者提供校验和登记逻辑。宿主在启动或显式应用时读取资源及引用文件并计算版本。 | 复用同一资源服务与现有注册契约。验证通过才发布，不开放插件任意写 HOME 或共享数据库。 |

模型后处理、工具登记等标识用于评审，实际签名可以演进。前置策略以返回值参与当次执行，订阅者处理已经发生的事实。这决定了插件应该接在哪个位置。[初始扩展点表](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#extension-points) · [外部恢复契约](spec/reference/contracts.md#外部工具的恢复能力) · [推荐采样](spec/architecture/runtime.md#输入框推荐的只读采样)

<a id="tools"></a>

## 官方工具与 Code Mode 脚本接口

以下是按面向谁、由谁实现整理的清单。业务工具默认进入 Code Mode。显式标为 `direct` 的工具只出现在本轮模型原生入口。Agent 绑定、当前授权和实际版本共同决定某轮能用什么。

| 暴露位置或所有者 | 工具或接口 | 实际作用 |
| --- | --- | --- |
| 模型原生入口 · Code Mode | `code_exec` | 运行一段调用获准工具的程序，再把程序输出交给模型。程序自身不能递归调用这个入口。 |
| Code Mode 脚本环境 | `catalog.search`、`catalog.describe` | 在本轮允许的范围发现工具、查询签名。 |
| Code Mode 脚本环境 | `tools.call`、`tools.parallel`、`wait_for` | 调用一个或多个工具、把异步等待交回 Run。各子调用有自己的调用 ID。 |
| 工具管理 | `tool_access.activate` | 在本 Run 开启已绑定且获准的按需 Tool 或 Toolset，下一轮刷新目录。 |
| Dispatch | `dispatch.submit`、`dispatch.list`、`dispatch.get`、`dispatch.update`、`dispatch.deliveries` | 提交、列出、读或改安排，包括暂停、恢复和取消，查看实际投递。 |
| Dispatch | `dispatch.preview`、`dispatch.evaluations` | 只读试算投递规则，查正式触发用的规则版本及结果。 |
| 来源管理 | `dispatch.sources.types`、`dispatch.sources.list`、`dispatch.sources.get`、`dispatch.sources.create`、`dispatch.sources.update` | 发现可配置的来源类型，管理获准的来源实例。 |
| 自管理与文档 | `harness.inspect`、`harness.docs` | 查看当前实例真正可用的能力、版本与契约，按主题或引用读取匹配版本的文档。 |
| 会话、分支与事件 | `sessions.create`、`sessions.branch`、`sessions.history()`、`events.search` | 创建空会话、派生子 Session、读当前 Session 的获准历史或查询事件。宿主将 `current_agent_inbox` 绑定到直接投给当前 Agent 的收件记录。 |
| Agent 与配置 | `agents.get`、`agents.update`、`settings.get`、`settings.update` | 复用资源服务，维护获准字段与配置值。 |
| 插件管理 | `plugins.get`、`plugins.install`、`plugins.set_enabled` | 查包及源码、安装版本、对目标启停。可归入 `plugins.authoring` 工具集。 |
| 上下文构建 | `context.preview` | 对已有获准材料试算组装结果，既不调用模型也不重执行业务工具。 |
| Session Note | `search_notes`、`read_note`、`write_note` | 按简介的字面关键词查当前 Session 中获准的 Note，按引用读全文或指定行，或以简介和正文新建一条。沿用现有 Toolset、授权与 Code Mode 调用机制。[参数与返回约定](spec/reference/contracts.md#notes) |
| Goal | 模型显式提交目标完成标记，名称待定 | 关联当前 Run 和完成证据，宿主仍检查必要结果、取消与交付条件。 |

例如 CI 报告在模型输入里只展示了一段，Agent 可用已有的 `note_ref` 调用 `read_note` 并指定起止行（从 1 计，两端都包含）。若当轮看不到引用，`search_notes` 可按简介搜索，按创建时间从新到旧分页返回。将自己的排查结论交给 `write_note` 只创建一份 Note，不触发交办。其他候选工具名和实现分工见[官方工具归档](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-tools)。

`read_skill` 的官方通用实现已暂缓，Skill 提供方可以实现自己的读取工具。CI 导入的工具和 `home.check_release_tag` 属于其各自来源。`ask_user`、查构建和读文件曾作为场景示例，尚无统一随包业务工具清单。输入框推荐走上层 REST 调用。上表中的脚本接口也无需重复暴露为模型原生工具。[工具注册与派发](goal-shaping/archive/loom-agent/2026-11-23-initial/tools.md)

<a id="events"></a>

## 官方事件类型与作用

首版规定了三种可分别核实的结果。一条 Event 记下事实，一次 EventDelivery 记下这份事实已投给哪个目标，一个 Run 承担一项交办。比如同一份 CI 报告可以投给资料 Session 留档，也可以投给值班 Agent 排查。两笔收件能分别查到，只有排查形成新的 Run。已在等这份报告的 Run 还可以按原 ToolCall 或作业号接回结果。[领域概念](spec/concepts/domain.md) · [交办与投递验收](spec/goals/v1.md#上下文与事件投递)

下面是**官方设计提出的 32 个候选事件名**。第二列指出谁核实并记录这一事实，第三列说明下一步能据此做什么。宿主负责持久发布，通过可替换的消息总线分发事件，本期默认使用 SQLite 本地事件总线，也支持 Redis Streams。公开字段、保留期限和对外托管通用订阅仍待定。来源方可以另行声明业务事件，如构建系统的 `job.finished`。候选公共字段包括 `event_id`、`type`、`source`、契约版本、时间、关联对象及数据或引用。上报业务事实的权限与登记内部推理决定、工具终态的权限分别校验。[事件分发约定](spec/architecture/runtime.md#event-distribution) · [中断与重送验收](spec/goals/v1.md#event-distribution-acceptance) · [原始事件目录](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#official-events) · [接口成熟度](spec/reference/contracts.md#完整评审索引)

| 候选事件 | 事实由谁确认 | 作用 |
| --- | --- | --- |
| `delivery.received` | 定向投递服务确认目标 Inbox 已持久收件 | 收件处理器取出投递，按意图留档、新建交办或接续已有 Run。 |
| `session.created` | 会话服务建立 Session 身份 | System Prompt 插件为新会话组装初始指令。 |
| `run.started` | Run 状态机记录首次输入和交办归属 | 触发策略评估是否需要第一轮推理。 |
| `run.input_received` | Run 状态机受理补充输入 | 触发策略取得已生效的输入及版本，评估后续动作。 |
| `run.wait_satisfied` | 等待协调器确认指定业务条件已满足，且需要重新评估推理 | 触发策略按这次等待及其材料评估一次。 |
| `run.cancel_requested` | 收件处理器核验并受理取消请求 | Run 状态机核对当前状态，接受取消后停止新的派发。取消终态另行记录。 |
| `model.inference_requested` | Run 状态机持久登记一次主推理决定 | 输入构建器按决定标识、材料截止位置和触发原因组装本轮请求。 |
| `context.compaction_ready` | 压缩插件通过宿主登记可读的压缩结果及所覆盖的历史版本 | 构建器检查结果是否适用，供同一次有效构建或后续选材使用。 |
| `model.input_prepared` / `model.input_failed` | 宿主验收本次输入构建的结果 | 就绪时把输入引用交给模型执行器。失败时由 Run 处理缺料、失效或其他错误。 |
| `model.completed` / `model.failed` | 模型执行器记录主推理输出或尝试失败 | Run 核对尝试后处理工具请求、待交付答复、重试、切换合格模型或失败。 |
| `tool.call_requested` | 宿主工具调用服务登记外层或子调用及其执行者 | 工具分派器按调用标识、父调用标识及绑定执行。 |
| `tool.accepted` / `tool.progressed` | 工具调用服务取得受理凭据或进度 | 回执分派器更新原调用。承诺最终报告的 Run 继续等报告。 |
| `tool.completed` / `tool.failed` | 工具调用服务确认结果或明确的终态错误 | 回执先交给直接调用者。首个受理回执丢失时，宿主按原记录核实外部结果。仅有超时不能认定为工具失败。 |
| `run.reply_ready` | Run 状态机保存待发答复、目标和稳定的交付标识 | 交付器按原约定发送，重试沿用同一份答复。 |
| `run.completed` / `run.failed` / `run.cancelled` | Run 状态机分别确认已履约、失败或已取消 | 供可选的状态视图刷新。约定“仅失败时通知”的工作在成功结果保存后即可完成。 |
| `section.registered` / `section.refreshed` / `section.withdrawn` | 宿主记录贡献方对指令片段的登记、更新或撤出 | System Prompt 插件同步适用的 section，模型发送前再核对版本。 |
| `settings.changed` | Settings 服务提交某插件实例的新配置版本 | 授权消费者按实例取用。例如 MCP 插件读取自己的新配置并决定是否重连。 |
| `tool.source_changed` / `tool.catalog_changed` | 来源插件报告工具定义变化，工具管理确认生效目录的新版本 | 前者触发重新发现，后者供请求准备器刷新缓存。发送时仍核对本轮实际授权和绑定。 |
| `skill.catalog_changed` | 支持动态通知的 Skill 来源报告成员或简介有变 | 可选的 Skill 适配器刷新自己的简介 section。静态目录可直接登记。 |
| `plugin.enabled` / `plugin.disabled` | 插件宿主已经完成装配或撤销 | 服务发现视图可据此刷新可用路由和插件。 |
| `branch.created` / `branch.updated` | 分支插件或适配器确认分支已创建或编辑已提交 | 携带版本引用，供压缩插件选择是否预计算。父 Run 的在途调用仍归原调用者。 |

后续确认的 Goal 复用内部推理请求。主动续跑由宿主在**原 Run** 登记一次决定，原因标为 Goal，不增加 Inbox 收件。构建器只在主动续跑那一轮追加角色为 `user`、来源为 `goal` 的说明，并将它随 ModelCall 输入留档。说明文本本身不单独发布 Event。若 Run 等的是测试报告，报告抵达后按工具回执推进，重复报告只触发一次分析。输入框推荐另由上层按需请求，展示和采纳只影响草稿，用户真正发送时才产生一次新投递。[Goal 续跑时序](spec/architecture/runtime.md#goal-续跑时模型收到什么) · [回执与压缩](spec/architecture/runtime.md#活跃-goal-与上下文压缩) · [输入框推荐契约](spec/reference/contracts.md#输入框推荐)

<a id="plugin-consumers"></a>

## 哪些插件订阅事件，为什么

插件通过统一入口 `plugin:subscribe` 订阅获准观察的事实。宿主分别记录各订阅的进度，保存处理结果及待执行动作后确认消费。向 Agent 或 Session 定向投递另有独立的收件记录。宿主在需要返回值的步骤调用选定的处理函数，例如让多个上下文变换按顺序处理材料，或让唯一的模型提供方回答一次请求。一个用量统计插件可以旁听模型完成，谁的响应能推进 Run 仍由宿主依据调用绑定确定。下表的**订阅映射为候选**，表中也列出已确认的能力及其实际入口。[插件契约](spec/reference/contracts.md#插件工具与上下文) · [原始插件订阅表](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#plugin-subscriptions)

| 插件或职责 | 建议订阅或直接入口 | 分工理由 |
| --- | --- | --- |
| 推理触发 | 订阅 `run.started`、`run.input_received`、`run.wait_satisfied` | 对首次受理、补充和满足的等待给出推进建议，由宿主按同一就绪状态登记一次决定。 |
| 上下文构建 | 订阅 `model.inference_requested`。启用压缩时可订阅 `context.compaction_ready` | 在已登记的决定下选材、排序并提交输入。摘要就绪可恢复原构建，主推理仍使用原决定。 |
| System Prompt | 订阅 `session.created` 及三种 `section.*` 事件 | 初始化会话指令，跟进贡献方的变更，发送前按所需版本同步。 |
| 工具管理 | 订阅 `tool.source_changed`。静态工具直接注册 | 对有变的来源重新读取目录。按需开启某项工具属于获准的管理调用。 |
| 投递安排、HTTP 接入和定时触发 | 来源型安排订阅选定来源和类型。即时请求、时钟到期直接进入安排求值 | 规则决定本次是否投、投给谁及消费意图，宿主保存结果并按项投递。具体插件分工待定。 |
| MCP 接入 | 可订阅本实例的 `settings.changed`。远端调用由分派器定向进入 | 配置变化可能要求重连或重新发现。执行某个远端工具时要保留唯一调用及回执关联。 |
| Skill 适配 | 动态来源可订阅 `skill.catalog_changed`。静态简介直接注册，正文按需读取 | 插件维护自己贡献的 section。正文读取入口可由接入插件提供。 |
| 上下文压缩 | 在构建选材后按预算进入。可订阅 `branch.created`、`branch.updated` 预计算 | 摘要服务于明确的历史范围。辅助模型结果回原构建，保留未决调用及判断所需证据。 |
| `branch` | 由显式创建分支请求调用 | 固定来源切点，派生独立 Session，通过宿主登记创建事实。 |
| Code Mode、默认模型调用 | 工具分派器按绑定调用 Code Mode。模型执行器按绑定调用提供方 | 一次执行有一个结果负责人。Code Mode 子工具各有调用标识，分别把结果返回给程序。 |
| 结果预览、插件自管理、文档与上下文预览 | 结果预览插件为候选。创造者通过获准的资源与工具入口自管理、查文档和预演构建。文档索引可随包安装或升级登记 | 结果展示插件可使用宿主记录的 Note 引用。读写 Note、预演构建和管理资源均按各自的调用定向处理。 |
| 官方 Goal（可选） | 宿主在交付前取得续跑建议，事件或扩展点的具体名称待评审 | Goal 跟进本 Run 的目标和进度，模型根据证据显式标记完成。宿主在必须等待时释放 worker，取消生效或约定预算耗尽时停止主动续跑。 |
| 输入框推荐（可选 Lua 插件） | 应用在回复已展示后按需调用推荐接口 | 插件提供推荐判断和提示，宿主只读采样指定会话版本。Goal 续跑、工具回执和压缩完成都不会主动触发推荐。 |

<a id="host-consumers"></a>

## Rust 宿主在哪里消费，哪些操作直接调用

Loom 对外提供一个逻辑入口，内部使用可调的执行池。默认由同一进程接入请求并运行 1 个 worker，需要扩容时可用同一程序分别启动入口和执行角色。单机压测固定工作负载、并发量及外部模型和 CI 时延，分别使用 1、2、4 个 worker，比较每秒成功提交的就绪 Run 步骤数（同一步重投只计一次），以及从宿主接纳使等待就绪的回执到对应下一步开始执行的 p95 时延。[执行池](spec/architecture/runtime.md#worker-pool) · [执行池验收](spec/goals/v1.md#worker-pool-acceptance)

当前仓库只有设计文档。表中路径按候选 `src/` 根书写，给实现者一个明确的落点，实际 crate 和文件名留待实现确定。**事件可以由多个授权观察者独立消费，执行动作和返回值则沿已登记的调用关系交给确定的接收者。** [运行架构](spec/architecture/runtime.md) · [原始宿主消费表](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#host-subscriptions)

| 建议工程位置 | 处理入口 | 这里负责核对什么 |
| --- | --- | --- |
| 事件接入与定向投递服务<br>`events/ingress.rs`、`delivery/service.rs` | 来源上报、安排交付等直接请求 | 保存事实，校验来源和目标权限，为每一笔目标收件记录结果。 |
| 投递安排执行与恢复<br>`dispatch/executor.rs` | 来源匹配、主动提交或时钟到期 | 绑定一次触发及规则版本，保存求值结果与逐项进度，失败项沿原决定恢复。 |
| 插件订阅分发器<br>`plugins/subscriptions.rs` | 与启用实例的订阅范围匹配的已提交事件 | 分别跟进每个消费者的进度与重试。一次匹配不会自动产生业务收件。 |
| 收件处理器<br>`delivery/inbox.rs` | `delivery.received` | 核对投递意图、归属、权限及当前状态，再留档、开 Run、补充或申请取消。外部审批经核验交回原等待。 |
| Run 状态机<br>`runs/state_machine.rs` | `run.cancel_requested`、`model.input_failed`、主推理的 `model.completed` 与 `model.failed`，以及选定策略返回的建议 | 在原 Run 顺序提交决定。工具派发或交付前核对新约束。普通交办中，模型正常给出最终答复、本轮无工具请求且所需结果已齐时按约定交付。启用 Goal 时还要核对目标状态。 |
| 等待协调器<br>`runs/waits.rs` | 登记或重查等待、收取已关联的回执 | 登记时先查已到事实。以原调用或作业确定满足了哪次等待，原子登记一次下一步。按时重查只检查状态。 |
| 模型请求准备器、模型执行器<br>`models/prepare.rs`、`models/executor.rs` | 准备器有缓存时可订阅 `tool.catalog_changed`。执行器消费 `model.input_prepared` | 准备器核对生效版本、section 和授权工具。执行器复核决定、按绑定调用唯一提供方，并记录模型与尝试。 |
| 工具分派器<br>`tools/dispatch.rs` | `tool.call_requested`。已核验审批定向继续原调用 | 校验授权、参数及原生或 Code Mode 入口，派发给唯一执行者。外部副作用先保存原参数、服务、作用域与可用的稳定键。 |
| 工具回执分派器<br>`tools/receipts.rs` | `tool.accepted`、`tool.progressed`、`tool.completed`、`tool.failed` | 按原调用、父调用和作业标识归位。程序的子结果先还给程序，顶层结果再交等待协调器。结果待核实时只按适配器声明的恢复能力操作。 |
| 答复交付器<br>`runs/replies.rs` | `run.reply_ready` | 用稳定标识发送已保存答复，把确认、失败或未核实的状态交回原 Run，依据原交付约定收尾。 |
| 插件宿主与文档索引<br>`plugins/host.rs`、`plugins/docs.rs` | 直接处理安装、升级、启停操作 | 校验并装配路由、订阅和工具，停用时撤回注册。采用版本化文档提案时在包变更时更新索引。 |
| 运行状态视图、服务发现视图（可选）<br>`views/runs.rs`、`views/discovery.rs` | 前者订阅三种 Run 终态，后者订阅 `plugin.enabled`、`plugin.disabled` | 展示已提交的运行结果或已生效的动态接口，也可以直接读取权威状态。 |
| 推荐请求的只读采样<br>`models/readonly_sample.rs` | 上层的推荐请求定向进入，复用模型路由 | 核验调用者、已展示回复及版本，按可见材料采样并复查权限。原生工具、Code Mode、MCP 都不能收到采样产生的工具请求。 |

例如 Code Mode 同时查两份报告，先返回的一份按子调用标识交还正在等待它的程序。等外层调用取得两份结果并完成，宿主才据原 Run 的业务等待评估是否需要模型综合结论。旁听工具终态的插件可分别看到这些事实，同时保留各自的消费进度。这样可以用原调用追查哪一份报告返回，也可以用 Run 的决定记录核对为何在此时发起下一轮推理。[并行、长等待和恢复](spec/architecture/runtime.md#并行长等待和恢复)

<a id="home"></a>

## LOOM_HOME 下面有什么

每个 loom-harness 服务实例有一个可持久挂载的 HOME。下图展示启用了部分能力的一次部署。`tools/` 允许部署者单独提供工具，空的可选目录可以省略。命令行与环境变量可分别采用 `--home` 和 `LOOM_HOME`。

```text
LOOM_HOME/
├── packages/                         # 安装的包版本，由发布者维护
│   ├── mcp/1.0.0/{plugin.yaml,plugin.lua}
│   ├── goal/1.0.0/{plugin.yaml,plugin.lua}
│   └── …                             # 各包的详细展开见下一节
├── plugins/                          # 部署者启用的实例，一包可有多实例
│   ├── mcp/
│   │   ├── instance.yaml
│   │   ├── settings.yaml
│   │   └── connections/{ci.yaml,docs.yaml}
│   ├── tool-registry/toolsets/release_check.yaml
│   ├── model-provider/models/{chat-primary.yaml,chat-backup.yaml}
│   └── skills/sources/team.yaml
├── agents/release/agent.yaml         # 每个 Agent 一份可寻址的定义
├── tools/check_release_tag/          # 部署者直接提供的 Tool
│   ├── tool.yaml
│   └── handler.lua
├── dispatches/release-check/
│   └── dispatch.yaml                 # 若使用脚本规则，可同放 rule.lua
├── state/loom.db                     # 单机持久化的一种实现选择
└── artifacts/                        # 本地部署的附件、二进制结果等产物
    └── notes/<session-id>/<note-id>.md  # Session 文本 Note
```

- **谁写什么。** 版本化包提供实现、默认值、模板和 schema。部署者在实例目录维护设置和具名资源。Agent 按稳定 ID 绑定工具，工具的 schema 与处理函数归其来源。插件通过宿主授予的资源接口访问自己的内容。数据库由宿主统一管理，插件存储和升级的可选方向是限定命名空间、记录迁移版本与执行结果。ZIP 分发、插件迁移脚本和任意 SQL 执行均未成为本期接口。简单 Lua 文本可沿用同一安装生命周期。
- **文件和 REST 如何一致。** YAML 保存可编辑定义，对应的 JSON REST 服务管理同一资源。启动扫描或显式应用时，宿主校验 schema、引用和权限，将 YAML 与所引用脚本作为一个修订应用。REST 写入也通过同一校验与持久化路径，更新同一份定义。读取应区分待应用文件、上次生效修订和错误原因。并发更新按期望修订检测冲突，失败保留已有可用版本。`apiVersion` 表示文件结构版本，`revision` 表示宿主成功应用的某份内容。
- **定义版本和运行状态。** Run 保留受理时的 Agent 定义及绑定依据，ModelCall 记录当轮真正发出的输入与目录，ToolCall 固定本次执行者、工具版本及生效配置。后续轮次可按已生效变更刷新适用的 section 和工具目录，实际派发仍复核当前授权。换 worker 后要取得相同的版本和回执记录，不能仅根据当前目录里碰巧同名的脚本恢复。
- **Note 和其他产物。** 每条 Note 保存为 `LOOM_HOME/artifacts/notes/<session-id>/<note-id>.md`，YAML 文件头存 `description`、`created_at` 和正文行数 `line_count`，正文存原文。所有 worker 共用这一份持久卷目录，首版查询 Note 列表只读文件头，无需专用 SQLite 库。附件和二进制结果仍可通过获准的内容接口读取。[HOME 目录规范](spec/reference/home.md#notes-storage)
- **运行状态。** 单机可由服务启动时打开或创建 `state/loom.db`。Event、Delivery、Inbox、Run 等待、调用、Goal 进度和配置应用记录都需要宿主持久管理。多 worker 部署要让后继 worker 读到同一份状态、包版本及产物。其他大对象可放入对象存储。凭据以引用配置，由宿主按授权使用。
- **有些能力不生成 YAML。** Goal 的完成条件属于某次交办，压缩摘要对应某段来源历史，推荐文本是基于某个会话版本的临时草稿。前两者各随其 Run 或上下文持久化。推荐只在用户实际发送后按正常输入投递。

<a id="resources"></a>

## Agent、独立 Tool、Dispatch 的 YAML

以下给的是可对照目录阅读的**格式提案**。为了让示例可以互相引用，发布 Agent 叫 `release`，主备模型叫 `chat-primary` 和 `chat-backup`，本地工具叫 `home.check_release_tag`。

### 一个 Agent 的定义

每个 Agent 单独保存在 **`agents/<agent-id>/agent.yaml`**。例如 `GET /api/agents` 从每个已生效的 Agent 生成列表，每个 Agent 可以独立更新、校验和查版本。

```yaml
# agents/release/agent.yaml
apiVersion: loom/v1alpha1
kind: Agent
id: release
name: 发布验收
description: 校验发布标签，并在需要时核查构建证据。
system_prompt: |
  根据实际报告给出发布结论。报告缺失时写明缺少哪一项。
model:
  default: chat-primary
  fallbacks: [chat-backup]
tools:
  - tool: home.check_release_tag
  - tool: mcp.ci.get_report
```

示例假设连接 `ci` 导入了远端 `get_report`。这里采用的完整 ID 拼法属于提案。Agent 应引用注册表实际返回的稳定 ID。若要将一整组工具按需绑定，已有候选形态是 `{toolset: plugins.authoring, activation: on_demand}`。Agent 绑定不能扩大当前调用者的权限。Goal 属于某一项显式启用的交办。

也可以从普通模板创建 Agent，取得查询自身定义、当前 Session 历史、直接投给自身的事件及读写 Session Note 的日常工具。创造者模板在相同日常能力上增加获准的自管理工具。例如用户要求“给你自己加一个提取 CI 错误码的工具”，创造者可以阅读当前版本文档，提交含 manifest 和 Lua 的新版本，在获准范围启用并绑定给自己。下一轮从目录实际调用工具读取 Note 并提取 `E421`，从 ToolCall 核实执行版本和结果。停用后，下一轮目录撤下该入口。[模板 YAML 与权限](spec/reference/home.md#agent-templates) · [创造者验收](spec/goals/v1.md#agent-template-acceptance)

### 一个独立工具的定义与实现

```yaml
# tools/check_release_tag/tool.yaml
apiVersion: loom/v1alpha1
kind: Tool
id: check_release_tag
description: 检查标签是否满足团队的 v主版本.次版本.修订号 格式。
input_schema:
  type: object
  properties:
    tag: {type: string}
  required: [tag]
  additionalProperties: false
output_schema:
  type: object
  properties:
    valid: {type: boolean}
  required: [valid]
exposure: code
handler: handler.lua
```

```lua
-- tools/check_release_tag/handler.lua
return function(ctx, args)
  local valid = args.tag:match("^v%d+%.%d+%.%d+$") ~= nil
  return { valid = valid }
end
```

宿主按来源命名空间把它登记为 `home.check_release_tag`。工具管理插件装载独立工具。包作者和 MCP 接入方通过同一个注册接口登记自己的工具。Agent 只需保存引用。独立工具仅获得其被授权的宿主能力。处理函数可用的 `ctx` 和调用参数由不同渠道传入。

### 一份投递安排

```yaml
# dispatches/release-check/dispatch.yaml
apiVersion: loom/v1alpha1
kind: Dispatch
id: release-check
enabled: true
trigger:
  type: schedule
  cron: "0 9 * * 1-5"
  timezone: Asia/Shanghai
rule:
  type: fixed
  target: {agent_id: release}
  action: start_run
  message: 检查本次发布标签及 CI 报告，给出验收结论。
```

这一份安排每个工作日上午九点交办一次。有自定义条件时，建议仍在**同一份资源**内把 `rule` 改成下面这段，再把 Lua 放进同目录。时钟实例提供触发能力，安排拥有自己的计划。同一次到期对应稳定发生标识，恢复时沿原次求值与投递记录继续。

```yaml
# dispatch.yaml 中替换 rule 的示例
rule:
  type: lua
  entrypoint: rule.lua
  params: {agent_id: release}
```

```lua
-- dispatches/release-check/rule.lua
function decide(ctx)
  return {{
    target = { agent_id = ctx.params.agent_id },
    action = "start_run",
    message = { text = "核查今天的发布报告。" }
  }}
end
```

Lua 规则返回投递意图，写 Event、创建 Delivery 与持久入箱仍由宿主处理。直接提交、定时触发或来源事件触发共用同一套规则契约。[投递规则与作用域](goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#dispatch-rule)

<a id="packages"></a>

## 各插件的包目录、实例目录与 YAML

下面是一种可评审的分包。表中的包名、版本、目录、配置键和数值都是例子。已有共识确定了能力和运行边界，其中若干能力仍可由同一个包提供。目录按一个 Loom 实例展示。

### 一份包可以启用为多个实例

以 MCP 为例，左边是发布者随 1.0.0 发布的内容，右边是部署者可以配置的一个实例。版本固定为字符串，供宿主精确选择。

```text
packages/mcp/1.0.0/                    plugins/mcp/
├── plugin.yaml                       ├── instance.yaml
├── plugin.lua                        ├── settings.yaml
├── lua/connection.lua                └── connections/
└── docs/connections.md                   ├── ci.yaml
                                          └── docs.yaml
```

```yaml
# packages/mcp/1.0.0/plugin.yaml
apiVersion: loom/v1alpha1
kind: PluginPackage
id: mcp
version: "1.0.0"
entrypoint: plugin.lua
settings_schema:
  type: object
  properties:
    timeout_ms: {type: integer, minimum: 1, default: 5000}
docs:
  - docs/connections.md
```

```yaml
# plugins/mcp/instance.yaml
apiVersion: loom/v1alpha1
kind: PluginInstance
id: mcp
package: mcp
version: "1.0.0"
enabled: true
```

```yaml
# plugins/mcp/settings.yaml
timeout_ms: 8000
```

这三份文件分别回答“包提供什么”“启用哪一版”“这个实例采用什么值”。宿主按 schema 校验设置，读回时区分提交版本与已应用版本。一次工具调用重试或异步接续时仍能读到原调用固定的设置版本。`enabled` 是实例装配开关，具体工具、资源和宿主接口按授权范围开放。例子里实例 ID 恰与包 ID 相同。需要两套隔离配置时可另设 `plugins/mcp-lab/`，同一个实例下接入两个 MCP Server 只需添加两份连接资源。历史示例 `mcp-ci` 也是部署者选的实例 ID，CI 指持续集成。

### 全部建议包的目录展开

`packages/<包>/<版本>/` 内统一有 `plugin.yaml` 和 `plugin.lua`。为便于查找，下面只展开 **该包另外包含的文件**。每个目录名均相对于自己的版本根。包可以用 `tools/` 放导出的 Tool，用 `routes/` 放 REST 处理函数，用 `lua/` 放策略与共享实现，具体文件名由包作者维护。

```text
packages/                                  # 每行的 1.0.0 是示例包版本
├── system-prompt/1.0.0/
│   └── lua/sections.lua
├── inference-trigger/1.0.0/
│   └── lua/should_invoke.lua
├── context-builder/1.0.0/
│   └── lua/build.lua
├── model-provider/1.0.0/
│   └── lua/invoke.lua
├── tool-registry/1.0.0/
│   ├── lua/catalog.lua
│   └── tools/activate.lua
├── code-mode/1.0.0/
│   ├── lua/entry.lua
│   └── docs/tools.md
├── mcp/1.0.0/
│   ├── lua/connection.lua
│   └── docs/connections.md
├── skills/1.0.0/
│   └── lua/discover.lua
├── compaction/1.0.0/
│   ├── lua/compact.lua
│   └── prompts/summarize.md
├── branch/1.0.0/
│   ├── lua/branch.lua
│   └── routes/branches.lua
├── goal/1.0.0/
│   ├── lua/continue.lua
│   └── prompts/continue.md
├── composer-suggestions/1.0.0/
│   ├── lua/suggest.lua
│   ├── routes/suggestions.lua
│   └── prompts/suggest.md
├── dispatch/1.0.0/
│   ├── lua/evaluate.lua
│   ├── tools/dispatch.lua
│   └── docs/rules.md
├── http-source/1.0.0/
│   └── lua/ingress.lua
├── timer-source/1.0.0/
│   └── lua/timer.lua
├── tool-results/1.0.0/
│   └── lua/preview.lua
└── harness-admin/1.0.0/
    ├── lua/{inspect.lua,docs.lua}
    ├── tools/context_preview.lua
    └── docs/index.md
```

例如 `tool-registry` 随包提供开启已获准工具的实现 `tools/activate.lua`，部署者自己写的 `home.check_release_tag` 则放在 HOME 顶层的 `tools/check_release_tag/`。两者最终都通过同一注册接口进入目录。

### 逐包实例配置与持久化

每行的包根均为 `packages/<包名>/1.0.0/`，含 `plugin.yaml`、`plugin.lua`。表中列出额外的**包内文件**。每行的实例根为 `plugins/<实例标识>/`，例中取与包相同的 ID，沿用上面的 `instance.yaml`。**设置示例**中的内容可以直接作为相应 `settings.yaml` 的 YAML。`{}` 表示该例使用包默认值，可省略这份文件。运行记录由相应宿主服务持久保存。

| 建议包名与定位 | 包内文件示例 | 实例内的文件与作用 | 设置示例 |
| --- | --- | --- | --- |
| `system-prompt`，职责已定 | `lua/sections.lua` | 来源插件注册、刷新、撤出 section。Session 的 section 版本和实际发送的 system prompt 随宿主调用记录可查。Agent 的基础指令在 Agent 定义里维护。 | `{}` |
| `inference-trigger`，默认推理触发策略 | `lua/should_invoke.lua` | 读取 Run 已受理的输入和已满足的等待。宿主保存每次决定、原因与材料位置，实例无需维护决定文件。 | `{}` |
| `context-builder`，默认输入组装策略 | `lua/build.lua` | 根据决定选材并组合插件贡献，宿主保留实际 message list 或不可变引用。实例无需保存一份聊天历史。 | `{}` |
| `model-provider`，默认模型调用，分包待定 | `lua/invoke.lua` | `models/chat-primary.yaml` 和 `models/chat-backup.yaml` 登记可选模型及能力，示例见下。Agent 可引用模型 ID、设置备选。宿主记录实际选择及各次尝试。 | `{request_timeout_ms: 60000}` |
| `tool-registry`，默认工具管理，分包待定 | `lua/catalog.lua` | `toolsets/release_check.yaml` 保存部署者选定的成员 ID，可同时引用本地 Lua 工具和 MCP 工具。登记工具来源、解析 Agent 的绑定。可见目录按作用域派生。工具定义归相应来源，Run 临时开启记录随 Run 保存。 | `{}` |
| `code-mode`，默认调用方式，分包待定 | `lua/entry.lua`、`docs/tools.md` | 脚本及父子 ToolCall、等待和回执由宿主关联。实例可限制单次并行度，仍受宿主上限约束。 | `{max_parallel_calls: 4}` |
| `mcp`，Lua 接入已定，随包安装待定 | `lua/connection.lua`、`docs/connections.md` | `connections/ci.yaml` 与 `connections/docs.yaml` 管理同一实例的两条连接。远端工具在发现后登记，可以按连接重新发现。 | `{timeout_ms: 8000}` |
| `skills`，可选适配 | `lua/discover.lua` | `sources/team.yaml` 声明可访问的技能目录，例见下。`SKILL.md` 和资料沿用技能包结构，也可由提供方随包放进 `packages/<provider>/<version>/skills/<name>/`。正文读取由具体接入方提供。 | `{}` |
| `compaction`，本期官方压缩能力 | `lua/compact.lua`、`prompts/summarize.md` | 可缓存已结束历史的摘要及来源范围、内容版本和策略版本。宿主另行保存已提交输入或稳定引用，活跃 Goal 和未决工具按原 Run 记录读取。 | `{summary_model: chat-primary, target_summary_tokens: 1200}` |
| `branch`，官方会话分支方案 | `lua/branch.lua` | 分支请求产生子 Session。宿主原子保存来源、不可变前缀、幂等结果和待发布事件，分支实例无需另建 Session 文件。 | `{}` |
| `goal`，按交办显式启用的官方能力 | `lua/continue.lua`、`prompts/continue.md` | 目标、完成条件、进度、预算和续跑决定归原 Run。每次交办独立给值，无实例级 `goals/*.yaml`。 | `{}` |
| `composer-suggestions`，可选输入框推荐 | `lua/suggest.lua`、`prompts/suggest.md` | 实例启用后，`settings.yaml` 的 `enabled` 控制是否接受推荐采样。请求绑定调用者和已交付回复的快照。推荐可为空、可过期，只在用户真正发送时进入会话。 | `{enabled: true, model: chat-primary, timeout_ms: 1500}` |
| `dispatch`，职责已定，具体分包候选 | `lua/evaluate.lua`、`docs/rules.md` | HOME 顶层 `dispatches/<id>/` 保存各安排及引用的规则。宿主保存正式求值、待投递项和收件进度。安排及脚本的完整示例见投递目录。 | `{}` |
| `http-source`，HTTP 接入分包候选 | `lua/ingress.lua` | 可受理外部提交并交给 Dispatch。服务使用宿主开放的路由与凭据，来源插件另有自定义配置时注册其资源。Webhook 属于未来来源示例。 | `{max_body_bytes: 1048576}` |
| `timer-source`，定时触发分包候选 | `lua/timer.lua` | 调度读取安排的时间表达式，按一次到期发生标识交给 Dispatch。实例可设缺省时区，每个安排仍可显式指定。 | `{default_timezone: Asia/Shanghai}` |
| `tool-results`，结果预览候选 | `lua/preview.lua` | 宿主接纳结果时，按字节阈值将大文本保存一次为 Session Note。该实例只选择本轮模型看到的预览，可附原 Note 引用。 | `{preview_chars: 1600}` |
| `harness-admin`，自管理与文档分包候选 | `lua/inspect.lua`、`lua/docs.lua`、`docs/index.md` | 通过已有资源服务查询或管理。文档按包版本登记，索引可重建。预览读取授权快照，本身不产生运行事实。 | `{}` |

任务插件和用量统计插件在归档中是扩展示例，可由第三方依同一格式编写。`LuaPluginProvider` 属于 Rust 宿主。用户附件、ToolCall 原文、Run 状态和已提交的 ModelCall 输入也由宿主按标识管理。

### 有独立身份的插件资源

**MCP 连接。** 建议由同一个连接资源服务管理两份文件。归档已有创建连接的 REST 示例，其他方法仍待定版。每个资源有独立标识、地址和凭据引用。宿主授予相应访问能力后，插件才可通过宿主使用对应凭据。删除连接时可按该 ID 撤下其登记的工具。下例使用文档示例域名。

```yaml
# plugins/mcp/connections/ci.yaml
apiVersion: mcp.loom/v1alpha1
kind: MCPConnection
id: ci
transport: streamable-http
url: https://ci.example.org/mcp
credential_ref: ci-read
```

```yaml
# plugins/mcp/connections/docs.yaml
apiVersion: mcp.loom/v1alpha1
kind: MCPConnection
id: docs
transport: streamable-http
url: https://docs.example.org/mcp
credential_ref: docs-read
```

**模型。** 若由模型插件承载部署方的模型目录，可用下面的资源定义一个逻辑模型。`remote_model` 是提供方识别的示例部署名，Agent、压缩和输入框推荐可以引用 `chat-primary`。插件向宿主模型路由登记实际支持的能力。本次决策最终绑定的提供方、模型及输入始终能沿 ModelCall 查回。另一种实现可由宿主集中维护模型目录，选定一种资源所有者即可。

```yaml
# plugins/model-provider/models/chat-primary.yaml
apiVersion: models.loom/v1alpha1
kind: Model
id: chat-primary
endpoint: https://models.example.org/v1
remote_model: chat-deployment-1
credential_ref: model-gateway
accepts: [text, image]
supports_tools: true
supports_structured_output: true
```

备选模型使用同一资源格式。Agent 的 `fallbacks: [chat-backup]` 引用第二条登记。路由会按本轮输入和输出要求检查其能力。

```yaml
# plugins/model-provider/models/chat-backup.yaml
apiVersion: models.loom/v1alpha1
kind: Model
id: chat-backup
endpoint: https://backup-models.example.org/v1
remote_model: chat-deployment-2
credential_ref: backup-model-gateway
accepts: [text, image]
supports_tools: true
supports_structured_output: true
```

**Skill 来源。** 资源 `team` 指向实例目录内的一组技能。相对目录按此插件实例根解析，需由宿主授予读取权限。也可以适配其他经授权的来源。每个技能的简介、正文、模板由技能包作者维护。适配层将简介提供给 System Prompt，供选择后读取正文。

```yaml
# plugins/skills/sources/team.yaml
apiVersion: skills.loom/v1alpha1
kind: SkillSource
id: team
directory: skill-packs/team
```

```text
plugins/skills/
├── instance.yaml
├── sources/team.yaml
└── skill-packs/team/
    └── release-review/
        ├── SKILL.md
        └── templates/report.md
```

下例是 `SKILL.md` 的内容。文件顶部的 YAML 用于发现，后面的正文及模板可按需读取。

```markdown
---
name: release-review
description: 审核构建和测试报告，给出发布结论。
---
先核对构建版本，再按 templates/report.md 汇总通过项和失败项。
```

Settings 接口读回实例设置。具名连接、模型、技能来源可经所属模块读取，具体 REST 路由见接口表或留待该插件定义。若插件后续引入私有 SQLite，可在该实例作用域提供独立迁移。例如，文档索引或压缩预计算丢失时可以按来源重建，已被 ModelCall 引用的内容则须在宿主仍能还原该次实际输入后方可清理。

### 部署者维护的混合 Toolset

部署者可以配置 `plugins/tool-registry/toolsets/release_check.yaml`，将 `home.check_release_tag` 与 `mcp.ci.get_report` 组成一个集合，供多个 Agent 按 ID 绑定。工具管理实例装载成员表，两个处理函数仍由各自的来源提供。[混合 Toolset 配置与 Agent 绑定](spec/reference/tool-sources.md#mixed-toolset)

### 插件自带的 Tool 和 Toolset

注册工具的插件可以在自己的包内组织文件。下面的 `release-tools` 是业务插件示例，用来和 HOME 独立工具对照。

```text
packages/release-tools/1.0.0/
├── plugin.yaml
├── plugin.lua                    # 按注册接口登记 Tool 与 Toolset
├── tools/report.lua              # 实现处理函数
└── toolsets/acceptance.yaml      # 这个包自己选择的可读配置
plugins/release-tools/
└── instance.yaml                 # 选择上面的包版本
```

```yaml
# packages/release-tools/1.0.0/toolsets/acceptance.yaml
id: acceptance
description: 发布前检查团队标签规范并读取构建报告。
members:
  - home.check_release_tag
  - release-tools.report
```

这个例子中，入口 `plugin.lua` 先注册包自带的 `report` 处理函数，再读取这一份成员表，通过 Toolset 契约登记 `release-tools.acceptance`。Agent 可以只绑定这个 Toolset。实现可直接以 Lua 声明静态成员。上面的 `toolsets/*.yaml` 是包作者选择的数据格式。相同原则也适用于 `sections/`、`prompts/` 和 `docs/` 等随包资产。

### 来源插件扩展出的 YAML

已配置的 EventSource 与引用它的 Dispatch 有独立身份。若未来安装提供 Webhook 类型的来源插件，可按同一 HOME 约定保存来源，例如下面这份 YAML。该例说明第三方如何接入，正式内置来源类型仍以安装清单为准。

```yaml
# plugins/build-webhook/sources/ci.yaml
apiVersion: webhook.example/v1alpha1
kind: EventSource
id: ci-builds
type: build-webhook
credential_ref: build-webhook-signature
event_types: [job.finished]
```

其中 `credential_ref` 用于验证上报者。要把失败报告送给哪个 Session 或 Agent，写在引用此来源的 `dispatch.yaml`。前面 MCP 的 `connections/ci.yaml` 是供 Loom **主动调用工具**的连接。这里的来源用于**接收外部事实**。CI 系统可以只提供其中一种能力。

## 三条可以贯穿验证的工作流

| 场景 | 应看到的结果 |
| --- | --- |
| 修改团队的标签校验规则 | 更新 `tools/check_release_tag/handler.lua`，应用时连同 `tool.yaml` 生成一个修订。Agent 仍引用 `home.check_release_tag`。从 ModelCall 能查当轮工具目录，从 ToolCall 能查实际执行的修订。脚本有错时给出诊断，既有生效版本仍可调用。 |
| CI 已经创建作业，首个回执丢失，Run 换 worker | 接手的 worker 从原 ToolCall 取出目标、原参数与稳定键。按适配器能力用原键查作业，或在保证去重的作用域与期限内同键同参恢复。缺少依据时保留待核实，等待迟到回执或人工处理。取回报告时，等待只满足一次。主模型能同时读到报告和有效 Goal，即使此前已压缩历史。这次推理由回执触发，无需 Goal 再发一次续跑。 |
| 答复已送达，推荐“推到 main”期间用户先发“先别推” | 上层用回复标识和会话版本请求推荐。服务端返回前复核版本。旧响应已在途中时客户端也会作废。只有用户实际发送的“先别推”产生新投递。推荐过程产生的业务 Run、ToolCall 和推送操作均为零。 |

配置变更的完整检查见 [HOME 资源验收](spec/goals/v1.md#home-acceptance)。长任务恢复和输入框推荐的判定条件见[首版运行验收](spec/goals/v1.md)。
