# 对外接口与插件契约

本页给出首版设计中集成方可以据以开发的行为要求，以及需要在接口定版时核实的细节。当前仓库交付的是设计。归档中的 REST 方法、工具名、事件名和 Rust 组件名为评审方案，尚无可供客户端联调的运行版本。

## 资源管理和事件投递

| 调用方的意图 | 使用的入口 | 结果如何认定 |
| --- | --- | --- |
| 定义或查询 Agent、插件、MCP 连接 | 对应资源的 REST 服务。插件可以登记自己的资源路由，生效接口随安装和启用情况变化 | 读回该资源及生效配置。MCP 连接的配置由 MCP 插件解释 |
| 从 HOME 或客户端维护同一份定义 | Agent 等可编辑资源经文件应用或相应的 JSON REST 服务写入。两条路径使用相同的稳定 ID、校验和修订语义 | 成功后可读回同一份已应用定义。校验失败可查原因，旧修订继续生效。独立 Tool 的管理与显式应用的 HTTP 路径待定。[HOME 资源规范](home.md) |
| 按模板创建 Agent | 插件可以声明提供的模板 YAML，也可以声明自己消费的 Agent 配置 schema（候选键 `agent_config_schema`）。创建时由宿主按目标插件实例校验配置、工具绑定及调用者权限 | 生效配置随新 Agent 的修订保存，对应实例通过 `ctx.agent_config` 读取自己的配置。插件实例的 Settings 独立管理。[模板与 Agent 修订](home.md#agent-templates) |
| 配置插件 | 统一的 `/api/settings/{plugin}` 资源。宿主负责按插件声明校验配置，并把本次生效值交给插件的 `ctx` | 插件以生效版本工作；应能区分配置已提交与运行侧已应用 |
| 上报、留档或交办 | 上报 Event，再按意图向 Agent 或 Session 定向投递。投递安排支持直接、定时、按条件三种方式，用户可通过 API 配置，也可授权 Agent 管理同一份安排 | 入箱、接收方已处理、Run 已完成是三个可区分的结果。只将资料投给 Session 时可以没有 Run |
| 给运行中的工作补充输入或请求取消 | 通过 Event 投给所属 Session，并关联目标 Run | 收件方核验该 Run 的归属、调用权限及运行状态。调用方按实际处理结果判断是否生效 |
| 使用插件注册的功能 | 插件可开放 REST 服务，也可登记供模型调用的工具。第三方资源可使用自己的路径 | 能力按宿主授予的作用域生效。比如提供任务资源的插件可以在启用后公开任务管理接口 |

投递规则允许使用 Lua 根据本次触发决定是否投、投给谁以及携带什么。用于评审的路径包括 `POST /api/events/{id}/deliveries` 和 `/api/dispatches`。实现时应明确资源所在作用域、可重试操作和已受理工作的查询方式。具体方法和路径见[REST 候选目录](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#rest-api)。

## 插件、工具与上下文

Rust 宿主通过 `PluginProvider` 加载和调用扩展，首版使用 `LuaPluginProvider`。官方与第三方插件共用扩展机制。宿主定义每个扩展点的输入、输出和错误，构造本次调用上下文。一个观察结果的插件和一个负责提交本次模型响应的提供方承担不同职责，宿主须能选定谁的返回值驱动 Run。

插件启用时通过统一入口 `plugin:subscribe` 声明事件类型和范围。宿主按实例授权分发，以实例内稳定的订阅 ID 分别记录消费进度，持久保存处理结果及待执行动作后确认消费。宿主默认使用 SQLite 本地事件总线，选择 Redis Streams 时插件沿用同一入口。向 Agent 或 Session 定向投递另行登记 EventDelivery，各目标的收件结果与各订阅的消费进度分别查询。[事件分发约定](../architecture/runtime.md#event-distribution) · [中断与重送验收](../goals/v1.md#event-distribution-acceptance)

| 扩展能力 | 首版契约 |
| --- | --- |
| 推理和输入组装 | 是否发起模型调用、哪些材料进入本轮 message list 是两个独立策略。处理本轮请求时先取得推理决定，再构建输入并使用选定的模型实现 |
| Goal 续跑 | 调用方可在新交办中选择官方 Goal 插件并给出目标和完成条件。插件在交付前可请求原 Run 继续推理，模型核对证据后显式标记目标完成。宿主按 Run 的等待、取消、预算和交付条件执行决定。具体字段和操作名留待接口评审。 |
| 活跃 Goal 的模型输入 | 主模型每次推理都能读到本轮有效的目标和完成条件，使用压缩历史时同样适用。只有 Goal 主动续跑才加入角色为 `user`、来源为 `goal` 的说明；工具回执唤醒时带入对应结果或可回读的引用。消息封装按提供方协议适配，在模型输入和调用记录中标明续跑说明的来源。见[输入组装与留档](../architecture/runtime.md#goal-续跑时模型收到什么)和[压缩规则](../architecture/runtime.md#活跃-goal-与上下文压缩)。 |
| 工具执行 | 可用工具默认经 Code Mode 调用，只有显式选择直接调用的工具进入模型原生工具清单。直接工具仅通过原生入口执行。Code Mode 自身有模型可调用的入口。独立 HOME Tool、插件随包工具、内置和 MCP 来源共用注册、授权、版本绑定及回执语义 |
| 混合工具集 | 部署者可在工具管理实例登记一份成员表，引用自定义 Lua 与 MCP 工具。Agent 绑定该集合后，宿主按 Tool ID 去重、按成员授权并固定本轮目录。集合编辑或连接下线后，在途调用仍按原绑定关联回执。[来源、配置与派发契约](tool-sources.md) |
| 外部操作恢复 | 工具适配器声明首次受理回执丢失后，外部服务支持哪些查询或幂等重试方式。宿主对原生和 Code Mode 调用使用同一套[恢复规则](#外部工具的恢复能力)。 |
| MCP 与自定义 Lua | MCP 插件按连接发现并登记远端操作，独立及随包 Lua 工具按来源登记处理函数。[工具来源契约](tool-sources.md) |
| OI Service 接入（已封板，待实现） | 内部工具插件按 Service 登记 Toolset，按成员登记直接工具。模型沿用成员发布名，本期由共用 MCP 客户端执行。[接入契约](oi-workspace-tools.md) · [目标与验收](../goals/oi-workspace-tools.md) |
| 系统指令 | Agent 持有基础 system prompt。工具、工具集及其他插件可注册、刷新、撤出 section。Session 创建时形成初始指令，后续每次模型调用前核对变化 |
| 压缩和分支 | 官方能力覆盖上下文压缩和 `branch`。压缩需要保留对所覆盖历史的追溯；分支从指定历史位置产生独立 Session，可引用原 Event。父 Run 中已经派发的工具及其迟到结果仍按原调用关联 |

Tool 和 Toolset 的具体登记字段、Code Mode 脚本语言、扩展点名称、官方组件的独立分包均可在实现设计中细化。分支切点与压缩复用可对照[归档中的分支契约](../../goal-shaping/archive/loom-agent/2026-11-23-initial/design.md#session-branch)和[压缩流程](../../goal-shaping/archive/loom-agent/2026-11-23-initial/design.md#context-compaction)。

<a id="notes"></a>

### Note 的检索、读取与历史回看

一篇 Note 属于一个 Session，保存为 `LOOM_HOME/artifacts/notes/<session-id>/<note-id>.md`。Markdown 文件的 YAML 头包含简短描述 `description`、创建时间 `created_at` 和正文行数 `line_count`，结束分隔符后的内容为正文。文件名和宿主路径只用于存储，调用方持有稳定的 `note_ref`。云端各 worker 挂载同一持久目录，文件本身足以支持检索和读取。[HOME 目录约定](home.md#notes-storage)

| 工具 | 调用契约 |
| --- | --- |
| `write_note(description, body)` | 在当前 Session 新建一篇 Note，描述必填，正文完整保存，成功后返回 `note_ref`。 |
| `search_notes(keyword?, cursor?, limit?)` | 在当前 Session 可见且有权读取的 Note 中分页检索。省略关键词或传空串时按 `created_at` 从新到旧列出。传入关键词时，仅对 `description` 做区分大小写的字面子串匹配（`grep -F`），命中项沿用相同排序。返回引用、描述、创建时间和正文总行数。总行数在保存时统计并写入文件头，列出 Note 只需读取文件头。 |
| `read_note(note_ref, start_line?, end_line?)` | 省略行号时返回完整正文。行号从 1 开始，起止行均包含。仅给起始行就读到文末，仅给结束行就从首行读起。YAML 头和两个分隔符均不占正文行号。 |

自动保存按工具结果的内容块处理。宿主接纳超出存储阈值的文本结果时，流式保存完整正文，取工具名、作业号和状态等已知信息生成可检索的描述。分块返回的结果保持原有顺序和类型，图片、二进制附件沿用已有产物读取入口。若上游只交付了部分日志，描述及预览应标明已留存的范围。正文可读后，宿主在提交原 ToolCall 结果时一并固定内容块与 `note_ref` 的关联，并登记待发布的结果 Event。相同回执重投或换 worker 恢复时复用该引用。自动保存属于接收结果的步骤，无需模型另调 `write_note`。

**从历史回看。** 上层可从 `GET /api/sessions/{id}/context/items` 找到原始消息或工具回执，也可通过 `GET /api/events` 找到结果 Event 后用 `GET /api/events/{event_id}` 打开。短原文可内联。已自动写入 Note 的结果在两个入口都能沿同一 ToolCall 和内容块取到状态、预览和同一 `note_ref`。调用详情仍可提供授权下载。宿主在每次查询和回读时核验权限，分支继承引用后同样适用。保留期内，该引用始终读回已接纳的那份原文。压缩历史、裁剪预览、同名笔记或再次推理均不改写它。

**模型本轮看到的内容。** `context.build` 按本轮预算选择全文或预览，`ModelCall` 记录实际采用的表示。这与按字节决定是否写 Note 分别判断。模型已获回读工具时可以按引用继续阅读。成功的 `read_note` 必须返回请求的全部文本或完整行区间。若本次模型输入容不下所请求的内容，应明确返回超限、正文大小和总行数，供调用方缩小范围。该读取结果不会再次触发自动写 Note。[大结果的接纳与回读](../../goal-shaping/archive/loom-agent/2026-11-23-initial/tools.md#large-tool-results)

### 输入框推荐

输入框推荐由可选 Lua 插件提供，沿用插件 Settings 启停。聊天应用展示一条已交付的回复、输入框仍为空时，可以按需请求一句后续消息。调用方从已展示的回复取得回复标识与会话版本。批处理等无需草稿的客户端可跳过此步骤。

候选入口为 `POST /api/sessions/{session_id}/suggestions`。以下请求基于会话版本 `v18` 中已展示的回复 `r42`。

```json
{"after_reply_id":"r42","expected_version":"v18"}
```

服务端核对调用者使用推荐额度和读取回复的权限，确认该回复属于指定 Session 且与预期版本匹配。Session 受理新用户消息后，先前的版本即已过期。下例中，响应带回实际取材的版本和一条候选文本。

```json
{"based_on_version":"v18","suggestion":"把刚才这个提交推到 origin/main 吧。"}
```

没有合适的下一句时，`suggestion` 为 `null`。请求已过期或权限已撤销时，服务端不返回旧候选文本。响应到达客户端时，只有原会话、回复和版本仍匹配且输入框仍为空才展示。用户切换会话、关闭推荐或发送消息时，客户端立即作废相关的在途请求与已展示推荐，防止输入框在发送后清空时又出现旧建议。

采纳只将文本填入可编辑草稿。用户点击发送时，上层才把最终文本通过现有 Event 和定向投递入口发送。推荐请求、展示或采纳都不产生 EventDelivery。宿主的权限和执行约束见[只读采样](../architecture/runtime.md#输入框推荐的只读采样)，竞态验收见[输入框推荐](../goals/v1.md#输入框推荐)。

### 外部工具的恢复能力

派发可能产生外部副作用的调用前，宿主在原 ToolCall 下持久保存目标服务、作用域、原参数或可还原的引用。服务支持调用方提供稳定键时，宿主先保存键，再随首次请求发送。适配器说明该键支持查询、幂等提交或两者兼有。支持幂等提交时，还需声明服务保证去重的范围和有效期。仅在 Loom 内部记录的调用 ID 无法让外部服务识别重复提交。

| 恢复时掌握的依据 | 宿主可执行的动作 |
| --- | --- |
| 已保存作业号，服务支持按号查询 | 用原作业号查询进度或结果。 |
| 首个受理回执丢失，服务支持按首次请求携带的稳定键查询 | 用原键查询，找回作业号或结果后关联到原 ToolCall。若作业可能尚未出现在查询结果中，保留“结果待核实”，稍后仍按原键查询。 |
| 无法按键查询，但服务保证同键重提可取回原受理结果 | 仅在去重仍有效时，向同一服务、在同一作用域内用原键和原参数重提，取得原作业号或结果。 |
| 只有作业号能查，而作业号未知，或现有依据已不足以安全恢复 | 保留“结果待核实”，停止自动重提这次操作。可等待能关联到原调用的迟到回执，或由人核实后决定。 |

## 模型与交付

首版包含模型选择、按配置进行故障切换、用户图片实际进入模型，以及按交办方要求校验结构化最终结果。建议用同一个样例贯穿接口评审：用户上传失败截图，交办要求返回指定字段的 JSON，首选模型临时不可用。备选应满足图片和输出约束，运行记录应能说明实际模型、尝试情况和交付结果。最终值的校验针对本次交办；一次中间工具调用自身的返回 schema 单独处理。附件上传形式、可切换的故障类别、重试次数与 schema 校验失败后的处理契约仍待定版。[候选接口与可观察结果](../../goal-shaping/archive/loom-agent/2026-11-23-initial/overview.md#model-io)

## Agent 操作 harness

普通模板（`standard`）的日常查询、创造者模板（`creator`）的获准自管理及真实调用验证属于首版要求。模板内的工具绑定由宿主核验权限。下表同时列出其他资源操作和 Run 内按需开启工具的候选方案。模板相关的工具名、Toolset 名、键名和具体 HTTP 路由仍可细化。

| 能力 | 候选入口 | 应能演示的行为 |
| --- | --- | --- |
| 普通模板的日常工具 | `agents.get`、`sessions.history()`、`events.search(scope: current_agent_inbox)` 和三个 Note 工具 | 查自身定义、读当前 Session 已获准的历史、查直接投给当前 Agent 的事件，并能按权限写、搜、读当前 Session 的 Note。查询身份和作用域由宿主绑定，历史复用上下文条目读取。[Note 契约](#notes) |
| 创造者模板的自管理 | 在日常工具上增加下列自检、文档和插件管理工具，以及 `agents.update`、`settings.get/update`、`context.preview` | 在获准范围提交含 manifest 和 Lua 的插件新版本并启停，修改自身指令与工具绑定，读写获准的实例设置，试算上下文。真实工具调用仍经过派发时的授权检查。 |
| 查实例和读指南 | `harness.inspect`、`harness.docs` | 先查当前版本可见的工具或扩展点，再按引用读匹配的示例。官方和第三方插件可在 manifest 登记随包文档，安装时建索引，查阅文档独立于是否已开启该工具集 |
| 按需开启工具（候选） | `tool_access.activate`，Agent 绑定上的 `activation` | Agent 声明可常开或按需开的 Tool/Toolset；获准的临时开启只作用于本 Run。下一次模型请求同步目录、直接工具声明和相关 section，换 worker 后仍可继续 |
| 管理和验证扩展 | `plugins.get/install/set_enabled`、`context.preview` | 读回插件源码与版本，修订、安装到获准范围并停用。可拿历史样本只读试算组装结果，再通过正式运行记录核实实际出站输入。安装包本身不会扩大调用权限 |
| 会话、设置与 Note | `sessions.create/branch`、`agents.get/update`、`settings.get/update`、`write_note`、`search_notes`、`read_note` | 创建独立 Session 后可选择是否交办。授权的 Agent 可写笔记、按描述检索，或从工具结果的引用读回原文。[Note 契约](#notes) |

例如，用户要求创造者给自己添加提取 CI 错误码的工具。创造者先查实例和匹配版本的开发文档，提交并启用插件版本，再绑定到自身。下一轮从可用目录调用新工具，对已有 Note 实际提取 `E421`，以 ToolCall 核对执行版本及结果。停用后下一轮目录撤下该入口。[模板配置](home.md#agent-templates) · [真实工具调用验收](../goals/v1.md#agent-template-acceptance)

## 完整评审索引

[整体设计总览](../../design-review.md)把接口、订阅、HOME 目录及各插件的 YAML 放在同一份导航中。下列八项清单可追溯首轮归档的职责、候选名称、消费者和分工理由。

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
