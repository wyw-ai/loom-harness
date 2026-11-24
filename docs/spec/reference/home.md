# LOOM_HOME 与文件资源

一个 loom-harness 服务实例对应一个可持久挂载的 HOME，通过 `--home` 或 `LOOM_HOME` 指定。服务在这里发现定义、应用修订并管理本地持久化数据。Agent 执行工具时可访问的项目工作区由接入方和运行授权决定。

七个顶层目录及其资源归属是首版设计约定。下文的 YAML 展示完整的资源关系，具体键名、标识拼法、插件如何分包仍可在接口定版时调整。仓库当前提供设计规范。[实例目录示例](../../design-review.md#home)

<a id="layout"></a>

## 目录与归属

| HOME 内的位置 | 内容与维护方 |
| --- | --- |
| `packages/<plugin-id>/<version>/` | 发布者提供的不可变包版本，包含声明、Lua 入口，可随包提供 schema、Agent 模板、提示模板和文档。相同版本可供多个实例使用。 |
| `plugins/<instance-id>/` | 部署者选择的包版本、启用状态、实例设置及该插件解释的具名资源，例如 MCP 连接和工具管理实例的 Toolset。 |
| `agents/<agent-id>/agent.yaml` | 可独立寻址的 Agent 定义，持有角色指令、模型、工具绑定及按消费插件实例区分的扩展配置。可以直接编写或[从模板创建](#agent-templates)。 |
| `tools/<local-id>/` | 部署者添加的独立 Tool，含 `tool.yaml` 与其引用的处理函数。包自带 Tool 位于相应包版本中，MCP Tool 来自远端发现。 |
| `dispatches/<dispatch-id>/` | 一份投递安排的 `dispatch.yaml` 和可选的 `rule.lua`。触发方式、目标和规则属于该安排。 |
| `state/` | 宿主管理的运行与应用状态。单机部署可在这里打开或创建 `loom.db`。 |
| `artifacts/` | 保存本地附件、二进制结果等产物。`notes/<session-id>/<note-id>.md` 保存所属 Session 的文本 Note。宿主按来源及调用者权限解析引用。 |

可选资源尚未配置时可省略其空目录。插件通过宿主授予的接口和作用域访问资源。文件里保存凭据引用，由宿主核验用途并按授权使用对应凭据。

<a id="notes-storage"></a>

### Session 的 Note 文件

每条 Note 对应 `LOOM_HOME/artifacts/notes/<session-id>/<note-id>.md` 中的一个 Markdown 文件。下面是一条 CI 排查笔记。YAML 文件头记录简介、创建时间和保存时统计的正文总行数，结束分隔符后是正文。

```markdown
---
description: T42 全量测试的失败用例与日志
created_at: "2026-11-24T10:00:00Z"
line_count: 1
---
用例 release_tag 在校验预发布标签时失败。
```

宿主接纳超过字节阈值的文本工具结果时，为该 ToolCall 的内容块在所属 Session 中保存一次 Note。模型本轮展示多少文字由上下文预算决定。接手原 Run 的 worker 挂载同一份持久化的 `artifacts/notes/` 目录，用原引用读回同一个文件。首版直接按 Session 目录读取文件头即可检索，无需另建 Note SQLite 库。相关 Event、Session 历史条目和工具结果详情沿用同一 `note_ref`。Agent 也可通过 `write_note` 主动创建 Note。三项工具的读取范围、搜索及分页见 [Note 接口契约](contracts.md#notes)。

<a id="resources"></a>

## Agent、独立 Tool 与投递安排

下列三个 YAML 文档分别保存到注释所指的路径。发布 Agent 可以按稳定 ID 引用单个 Tool 或整个 Toolset。示例选择两个单独的工具。若希望多个 Agent 共用同一份成员表，可改为绑定[混合工具集](tool-sources.md#mixed-toolset)。工具能否出现在本轮模型请求，以及调用时能否真正派发，均受当前授权约束。

```yaml
# agents/release/agent.yaml
apiVersion: loom/v1alpha1
kind: Agent
id: release
name: 发布验收
description: 核查发布标签与 CI 报告。
system_prompt: 根据实际报告给出发布结论，写明缺失的证据。
model: {default: chat-primary, fallbacks: [chat-backup]}
tools:
  - tool: home.check_release_tag
  - tool: mcp.ci.get_report
---
# tools/check_release_tag/tool.yaml
apiVersion: loom/v1alpha1
kind: Tool
id: check_release_tag
description: 检查发布标签是否符合团队版本格式。
input_schema:
  type: object
  properties: {tag: {type: string}}
  required: [tag]
  additionalProperties: false
output_schema:
  type: object
  properties: {valid: {type: boolean}}
  required: [valid]
exposure: code
handler: handler.lua
---
# dispatches/release-check/dispatch.yaml
apiVersion: loom/v1alpha1
kind: Dispatch
id: release-check
enabled: true
trigger: {type: schedule, cron: "0 9 * * 1-5", timezone: Asia/Shanghai}
rule:
  type: fixed
  target: {agent_id: release}
  action: start_run
  message: 核查发布标签和 CI 报告。
```

`handler.lua` 与 `tool.yaml` 同目录。工具管理插件加载它，按来源登记为 `home.check_release_tag`。示例中的 `mcp.ci.get_report` 假设连接 `ci` 已发现远端 `get_report`。若安排使用脚本，可把同一文件的 `rule` 换为 `{type: lua, entrypoint: rule.lua, params: {agent_id: release}}`。规则输出投递意图，宿主负责实际投递和入箱。直接提交、定时及来源事件触发复用这项约定。[资源与 Lua 实现示例](../../design-review.md#resources)

<a id="agent-templates"></a>

## 从模板创建普通 Agent 或创造者

首版提供 `standard`（普通 Agent）和 `creator`（创造者）两份可读的 YAML 模板。第三方可以将自己的模板放在版本包的 `templates/agents/` 下，通过 `plugin.yaml` 声明。例如，提供方可在 manifest 中列出下列包内相对路径。包名、字段和工具集 ID 为示例。

```yaml
# 提供方 plugin.yaml 中的声明
agent_templates:
  - templates/agents/standard.yaml
  - templates/agents/creator.yaml
```

上层列出当前已装配且可见的模板时，可取得模板 ID、简介、来源和包版本。创建者选定具体来源与版本，并提供新 Agent 的 ID。宿主把模板的 `spec` 作为初值，沿用普通 Agent 的资源校验与创建入口，生成独立的 `agents/<agent-id>/agent.yaml` 和生效修订。源模板的 ID 用于选模板，同一模板可创建多个不同 ID 的 Agent。模板更新仅影响之后选用新版创建的 Agent；现有 Agent 可继续独立编辑。

```yaml
# templates/agents/standard.yaml
apiVersion: loom/v1alpha1
kind: AgentTemplate
id: standard
spec:
  name: 普通 Agent
  description: 处理日常交办，回看当前会话、收件与笔记。
  system_prompt: 回答涉及既有结论时，先核对相关原始记录。
  tools:
    - toolset: loom.daily
---
# templates/agents/creator.yaml
apiVersion: loom/v1alpha1
kind: AgentTemplate
id: creator
spec:
  name: 创造者
  description: 编写并验证插件，维护自身定义和获准的配置。
  system_prompt: 修改插件前查阅生效版本的契约，启用后用一次真实调用验证。
  tools:
    - toolset: loom.daily
    - toolset: plugins.authoring
    - toolset: loom.self_management
```

模板还可预填普通 Agent 支持的模型及备选（如 `model: {default: chat-primary, fallbacks: [chat-backup]}`）、单个 Tool 绑定和各插件声明的 Agent 级字段。创建的 Agent 沿用已有的模型选择、目录装配和权限规则。

| 模板 | 本期需要提供的能力 |
| --- | --- |
| 普通 Agent | 查看自己的 Agent 定义；用 `sessions.history()` 按序查看当前 Session 的消息和回执；用 `events.search(scope: current_agent_inbox)` 查看直接投递给当前 Agent 的 Event；在当前 Session 使用 `write_note`、`search_notes`、`read_note`。宿主从 `ctx` 绑定身份，按原材料权限返回结果。 |
| 创造者 | 包含普通 Agent 全部能力。可在授权范围内查运行实例和版本指南，读插件源码，提交 manifest 与 Lua 形成新版本，启用、修订或停用插件，修改自己的指令和工具绑定，读写获准的插件设置，并预览上下文。新工具在下一次 ModelCall 的目录生效后，创造者实际调用它，从 ToolCall 核实执行版本和结果。 |

**插件声明可读的配置。** 提供模板和消费配置可以由不同插件承担。消费方在自己的 manifest 声明 `agent_config_schema`。模板的 `spec.extensions` 按目标插件**实例 ID** 分组；创建时，宿主用对应包版本的 schema 校验，作为生成的 Agent 修订中的 `extensions` 保存。下面是历史查询插件及选用它的模板片段，`history-main` 是该部署启用的实例 ID。

```yaml
# 消费方 plugin.yaml 中的声明
agent_config_schema:
  type: object
  properties:
    history_page_size: {type: integer, minimum: 1}
  additionalProperties: false
---
# 某份 AgentTemplate 中的片段
spec:
  extensions:
    history-main:
      history_page_size: 20
```

普通 Agent 可以取 20，创造者可以取 50。在有对应 Agent 上下文的调用中，宿主只将该 Agent 修订中属于本插件实例的已校验值传入 `ctx.agent_config`。两个 Agent 共用同一实例仍各读各的值；实例通用设置继续由 `plugins/<instance-id>/settings.yaml` 维护。工具绑定和分页设置均受实际调用身份的权限约束。选择创造者模板不会赋予管理其他 Agent、插件或部署范围的权限。[模板与创作流程验收](../goals/v1.md#agent-template-acceptance)

<a id="packages"></a>

## 插件包、实例、设置与具名资源

每个包版本有发布者维护的声明。每个实例有自己的标识、版本选择和部署设置，实例 ID 与包 ID 可以不同。配置字段的默认值与校验约束随包版本声明，宿主将生效值提供给该实例的 `ctx`。资源有独立标识和生命周期时，由插件解释其格式，并由对应资源服务管理。[Settings 与资源接口](contracts.md#资源管理和事件投递)

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
---
# plugins/mcp/instance.yaml
apiVersion: loom/v1alpha1
kind: PluginInstance
id: mcp
package: mcp
version: "1.0.0"
enabled: true
---
# plugins/mcp/settings.yaml
timeout_ms: 8000
---
# plugins/mcp/connections/ci.yaml
apiVersion: mcp.loom/v1alpha1
kind: MCPConnection
id: ci
transport: streamable-http
url: https://ci.example.org/mcp
credential_ref: ci-read
```

再接一台文档服务器时，同一实例可增加 `connections/docs.yaml`，独立设置 `id`、地址和凭据引用。远端工具按连接发现和登记。插件作者也可随包组织 Tool、Toolset、Skill 或文档。来源插件还可定义接收外部事实的 EventSource，其身份与引用它的投递安排分开。[具名资源、包内工具集与来源示例](../../design-review.md#packages)

### 官方能力的目录索引

以下 17 行覆盖官方能力的一种**候选分包**。包内示例均相对于 `packages/<包名>/<版本>/`，省略共同的 `plugin.yaml`、`plugin.lua`。实例资源相对于 `plugins/<实例标识>/`，省略共同的 `instance.yaml`。花括号给出该实例 `settings.yaml` 的示例值。`{}` 表示使用包默认值，可省略设置文件。完整包树、设置和主备模型、Skill 来源的 YAML 见[逐包目录与示例](../../design-review.md#packages)。

| 建议包名 | 包内代表文件 | 实例设置、其他资源或运行数据的归属 |
| --- | --- | --- |
| `system-prompt` | `lua/sections.lua` | `{}`。来源插件贡献 section，宿主可按 Session 和 ModelCall 追溯实际指令。 |
| `inference-trigger` | `lua/should_invoke.lua` | `{}`。宿主在 Run 上保存推理决定与输入边界。 |
| `context-builder` | `lua/build.lua` | `{}`。宿主保存出站输入或可还原该输入的引用。 |
| `model-provider` | `lua/invoke.lua` | `{request_timeout_ms: 60000}`。由该插件登记模型时，`models/chat-primary.yaml`、`models/chat-backup.yaml` 分别提供逻辑 ID、远端部署名、凭据引用与能力。 |
| `tool-registry` | `lua/catalog.lua`、`tools/activate.lua` | `{}`。实例中的 `toolsets/release_check.yaml` 可显式组合 HOME 与 MCP 工具。长期绑定属于 Agent，临时开启属于 Run。 |
| `code-mode` | `lua/entry.lua` | `{max_parallel_calls: 4}`。父子 ToolCall、执行进度与回执由宿主关联。 |
| `mcp` | `lua/connection.lua` | `{timeout_ms: 8000}`。一个实例可管理 `connections/ci.yaml`、`connections/docs.yaml` 等多条连接。Lua 接入方式已定，是否随包默认安装待定。 |
| `skills` | `lua/discover.lua` | `{}`。可选接入。`sources/team.yaml` 可指向获准读取的 `skill-packs/team/`，各技能自行维护 `SKILL.md`、模板及其他材料。 |
| `compaction` | `lua/compact.lua`、`prompts/summarize.md` | `{summary_model: chat-primary, target_summary_tokens: 1200}`。摘要保留来源范围及内容和策略版本，已提交输入可还原。 |
| `branch` | `lua/branch.lua`、`routes/branches.lua` | `{}`。宿主为子 Session 保存来源、不可变前缀引用和幂等结果。 |
| `goal` | `lua/continue.lua`、`prompts/continue.md` | `{}`。调用方按交办显式选用。目标、完成条件、进度和续跑决定随原 Run 保存。 |
| `composer-suggestions` | `lua/suggest.lua`、`prompts/suggest.md` | `{enabled: true, model: chat-primary, timeout_ms: 1500}`。插件已装配时，该开关控制可选的只读采样。推荐绑定调用者和回复快照，实际发送才产生用户投递。 |
| `dispatch` | `lua/evaluate.lua`、`tools/dispatch.lua` | `{}`。安排位于 HOME 顶层的 `dispatches/`。宿主保存求值、投递与收件进度。 |
| `http-source` | `lua/ingress.lua` | `{max_body_bytes: 1048576}`。HTTP 适配器把请求交给 Dispatch。Webhook 来源是可扩展示例。 |
| `timer-source` | `lua/timer.lua` | `{default_timezone: Asia/Shanghai}`。安排可以声明自己的时区。一次到期发生标识关联原次求值。 |
| `tool-results` | `lua/preview.lua` | `{preview_chars: 1600}` 为候选预览设置，只影响模型本轮看到的内容。大文本 Note 由宿主在接纳结果时保存，正文按 [Note 接口契约](contracts.md#notes)读取。二进制结果沿其授权内容引用读取。 |
| `harness-admin` | `lua/inspect.lua`、`tools/context_preview.lua` | `{}`。自管理及预览复用获准的资源服务。随包文档按版本登记，可重建索引。 |

<a id="apply"></a>

## 文件如何生效

| 时点 | 宿主行为 |
| --- | --- |
| 发现与应用 | 启动扫描或显式应用时校验 schema、引用与操作权限。单个资源的 YAML 和引用的 Lua 作为同一修订应用。`apiVersion` 识别文件结构，`revision` 标识已经应用的内容。 |
| REST 写入与并发更新 | 文件和已有资源的 REST 表达读写同一份定义，经过同一校验和持久化路径。写入按期望修订检测冲突。插件实例设置走统一 Settings 服务，独立的连接等资源走所属服务。 |
| 失败与读回 | 区分待应用内容、上次生效修订及本次错误。校验或装配失败时，已有可用版本继续服务。安装、针对目标启用和模型实际可见是三个分别可查的状态。 |
| 执行与接续 | Run 保留受理时的 Agent 定义及绑定依据。ModelCall 保存实际输入和工具目录。ToolCall 固定执行者、工具版本与配置修订，同一次调用重试或换 worker 时仍可取回。后续推理按已生效的变更同步 section 与可用工具，实际派发时复核授权。 |

<a id="persistence"></a>

## 持久化与迁移边界

宿主持久保存 Event、Delivery、Inbox、Run 等待与 Goal 进度、调用、应用记录及其版本引用。多 worker 部署共享 Note 的持久卷目录，并保证后继 worker 能读取原状态、包版本、脚本和产物。附件、二进制结果等其他大对象可用本地 `artifacts/` 或对象存储，状态中保留按授权解析的稳定引用。[执行与恢复](../architecture/runtime.md)

单机 SQLite 是 `state/loom.db` 的部署选项。插件私库、按实例隔离的命名空间、带版本及执行结果的迁移均属候选。迁移或清理后，仍在处理的逻辑调用必须可接续，已发送的输入仍可按记录还原。ZIP 分发、插件迁移脚本与执行任意 SQL 的接口均待评估。简单 Lua 文本可走同一安装、版本登记和启用流程。
