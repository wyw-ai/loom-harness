# MCP 与自定义 Lua 工具的接入

MCP 连接、HOME 独立 Lua 工具和插件自带 Lua 工具通过同一注册与派发契约进入 Loom。Agent 可以绑定一个 Tool，也可以绑定跨来源的 Toolset。每个 Tool 有唯一执行者，每次 ToolCall 都能查回所用的来源、配置、参数和结果。首版覆盖 MCP 与 Lua，内部工具插件的 OI Service 自动接入另有独立规范。协议组合、配置键名和实例级 Toolset 的物理路径均保留为候选。[HOME 资源规范](home.md) · [工具接入设计归档](../../goal-shaping/archive/loom-agent/2026-11-24-tool-sources/design.md) · [OI Service 接入目标](../goals/oi-workspace-tools.md)

## 工具身份与注册

| 来源 | 谁登记，怎样标识 | 一次调用固定的执行依据 |
| --- | --- | --- |
| HOME 独立 Lua 工具 | 工具管理插件装载 `tools/<local-id>/tool.yaml` 及其同目录处理函数。`home` 是保留的来源命名空间，例如 `home.check_release_tag`。 | 工具定义、处理函数及其共同的代码修订。 |
| 插件自带 Lua 工具 | 插件入口登记工具，身份包含插件实例和本地名，例如 `release-tools.report`。包的多个实例可以分别装配。 | 所属实例、包版本、处理函数及实例配置修订。 |
| MCP 工具 | MCP 插件按连接发现并转换。身份包含插件实例、连接和区分大小写的远端原名，例如 `mcp.ci.get_report`。 | 来源内部身份、连接修订、实例配置版本、远端原名和发现时的签名。 |
| OI Service 成员（已封板，待实现） | 官方内部工具插件按实例、Service 和成员定位，例如 `oi-office.workbench.capture_a`。模型直接调用 `capture_a`。 | Service 成员的生效绑定、对应远端签名和当次实际传输，本期为 MCP。[OI 同步与命名契约](oi-workspace-tools.md) |

宿主保存标识的原始分量，可读 ID 的编码须能区分分量内的点号等分隔符。不同服务都叫 `search` 时，注册表可按各自的实例和连接定位。MCP 的展示标题变化不改变工具身份，远端原名变化则撤下旧名并登记新名。删除并以同名重建连接产生新的来源身份，历史调用仍指向原来源。模型供应商限制函数名时，ModelCall 保存可逆的名称映射。[MCP 工具命名](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#tool-names)

每个 Tool 登记描述、输入 schema、可选的输出 schema、调用入口及版本化执行绑定。Toolset 可以登记简介、成员 Tool ID 及共用 section。包作者可通过插件入口登记 Tool 和 Toolset，各来源共用注册机制，`register_tool` 与 `register_toolset` 为候选名称。集合只持有成员引用，撤下集合只影响通过它建立的绑定和说明。

一次包启用、一次独立工具应用或一批 MCP 发现结果按来源提交。宿主校验所有者、标识冲突、schema 与执行绑定后，原子发布该来源的完整修订。并发刷新须核对来源配置修订和刷新顺序。过时结果、未完成的分页及相互矛盾的候选条目不能替换已生效目录。只在有效目录实际变化时更新目录版本，订阅者可据通知刷新视图。

## MCP 发现与 Lua 装载

**MCP 连接。** 部署者将地址、凭据引用和可选导入策略写入 `plugins/<instance-id>/connections/<id>.yaml`。MCP 插件负责转换远端工具，宿主向它提供获准的协议客户端能力、调用身份与请求期限。建议首批验证 Streamable HTTP 与 MCP `2026-07-28` 的组合。其他协议版本由相应适配器处理并公开兼容状态，stdio 接入可独立排期。

1. 确认服务声明工具能力后，在同一远端授权上下文逐页调用 `tools/list`。保留远端 `name`、描述、`inputSchema` 与可选 `outputSchema`。候选 `imports` 策略为省略时导入已发现的全部工具、`[]` 导入零项、明确列举时按远端原名精确匹配。部署者可为某个导入项选择直接入口。远端注解仅作说明，权限、调用入口和副作用恢复策略由 Loom 独立核验。[MCP 发现与工具描述](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
2. 目录缓存按实例、连接修订及实际远端授权上下文隔离。尊重 `ttlMs` 和 `cacheScope`，也可显式刷新。在上述协议版本，服务支持清单变更时，客户端可通过 `subscriptions/listen` 订阅 `toolsListChanged`。收到 `notifications/tools/list_changed` 后使缓存失效，重新列举。分页跨页没有一致性保证；末页失败、游标失效或发现冲突时保留诊断并从头重取。宿主只对自己发布的完整修订负责。[缓存与分页](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching#interaction-with-pagination) · [清单变更通知](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#list-changed-notification)
3. schema 方言不受支持或引用无法安全解析的工具须给出诊断。待发现、失联和撤权分别反映在来源状态中。旧清单可用于获准的诊断，当前授权失效、连接已停用或地址被替换时，立即阻止依赖旧绑定的新派发。其他身份不能借缓存看到该来源的工具名或签名。
4. 实际派发时，插件向原连接发送 `tools/call`，传递远端原名和已校验的业务参数。保留有序 `content` 及可选的 `structuredContent`，声明输出 schema 时校验成功结果的结构化值。工具报告的 `isError: true`、JSON-RPC 错误、结果格式错误和通信中断应可区分。超限文本结果由宿主自动保存为 Note，Session 和结果 Event 均可按原调用回读。收到格式错误的结果时保留诊断，避免为修正格式而再次提交业务操作。[MCP 结果与错误](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#error-handling) · [Note 读取契约](contracts.md#notes)

**自定义 Lua。** 对 HOME 工具，工具管理插件解析资源内的相对处理函数，校验定义、编译 Lua 并确认取得可调用的处理函数。定义与代码一同形成不可变修订，只修改 `handler.lua` 也生成新修订。编译或校验失败时保留原生效版本。包内 Lua 由该包的 `plugin.lua` 显式登记。两种 Lua 工具均先校验模型提交的参数，再执行绑定的 `handler(ctx, args)`。宿主构造受限 `ctx`，按实际调用身份授予能力，加载器的管理权限不会传给工具。返回值按声明校验，超时、执行异常及结果不符合 schema 分别诊断。[HOME 的应用与版本规则](home.md#apply)

<a id="mixed-toolset"></a>

## 一个混合 Toolset 如何绑定和调用

发布 Agent 需要按团队规则核对标签，再向 CI 读取报告。前者使用 `tools/check_release_tag/tool.yaml` 及同目录的 `handler.lua`，后者通过 `plugins/mcp/connections/ci.yaml` 从 CI 导入 `get_report`。对于部署者维护的集合，建议放在工具管理实例的 `toolsets/` 中。包作者自己的成员表仍由包入口解释。下面两个 YAML 文档分别写入注释所示文件。[本地工具的参数与结果定义](home.md#resources) · [含连接导入、本地工具与处理函数的完整配置](../../goal-shaping/archive/loom-agent/2026-11-24-tool-sources/design.md)

```yaml
# plugins/tool-registry/toolsets/release_check.yaml
apiVersion: loom/v1alpha1
kind: Toolset
id: release_check
description: 检查发布标签并读取 CI 报告。
members:
  - home.check_release_tag
  - mcp.ci.get_report
---
# agents/release/agent.yaml
apiVersion: loom/v1alpha1
kind: Agent
id: release
name: 发布验收
description: 按团队规则验收发布。
system_prompt: 根据实际标签与 CI 报告给出结论，写明缺失的证据。
model: {default: chat-primary}
tools:
  - toolset: tool-registry.release_check
```

假设 CI 的 `get_report` 声明了字符串参数 `build_id`，按构建 `B42` 执行时，两项业务调用如下。远端的实际签名以该来源发现并生效的版本为准。

| Tool ID | 业务参数示例 | 执行与返回 |
| --- | --- | --- |
| `home.check_release_tag` | `{"tag":"v1.3.0"}` | 固定修订的 Lua 处理函数收到 `args.tag`，返回 `{"valid":true}`。 |
| `mcp.ci.get_report` | `{"build_id":"B42"}` | 对 `ci` 执行 `tools/call`，发送 `{"name":"get_report","arguments":{"build_id":"B42"}}`，报告或可读引用归入该次 ToolCall。 |

Agent 只绑定这个集合时，两项默认工具都可以在 Code Mode 中发现。一次程序分别调用它们，会形成一个外层 ToolCall 和两个拥有独立标识的子 ToolCall。子结果先交给父程序，模型收到外层汇总。另一个 Agent 可以只绑定 `mcp.ci.get_report`，两项绑定各自按权限解析。

## 目录快照与派发

- **装配。** 先装载来源，再解析 Toolset 成员和 Agent 绑定。明确不存在的成员或引用拼写错误使候选配置应用失败。已配置的连接仍在首次发现时，标记相关引用待解析。两种情况都保留已有生效修订。已生效集合中的远端成员后来失联或消失时，保留该成员引用和原因，让其他获准成员继续可用。[混合工具集验收](../goals/v1.md#mixed-toolset-acceptance)
- **可见性。** 展开已开启的集合和单项绑定，按 Tool ID 去重，再与当前身份、授权、来源健康及启用状态取交集。显式成员表只吸收部署者选定的工具。候选绑定方式 `on_demand` 将临时开启记录在对应 Run 上。开启或集合修订的变化随下一次模型调用刷新，共用 section 遵循集合的开启和可见范围。[按需开启](../../goal-shaping/archive/loom-agent/2026-11-23-initial/tools.md#on-demand-tools)
- **调用入口。** 默认 `code` 工具通过 Code Mode 发现与调用，显式 `direct` 工具只暴露给模型原生工具接口。两者使用相同的 ToolCall 授权与派发规则。ModelCall 固定实际目录、签名、名称映射和绑定版本。派发前复核该快照、当前权限与所需批准，校验参数并持久登记派发意图。已撤权或已知远端签名变化时返回原绑定失效，后续推理再读取新版本。Loom 可固定已发现的契约与远端身份，远端代码版本由服务方控制。
- **可查性。** 面向 Agent 的目录仅显示当前可用的工具。获准的配置诊断可进一步说明来源、绑定路径、可见入口、修订和不可用原因。ToolCall 记录实际调用者、可选父调用、来源绑定、参数、状态和回执。[目录与操作契约](contracts.md)

## 等待、撤回与跨 worker 恢复

新调用可以使用已经应用的新修订，已登记调用继续引用原来源、原配置及原参数。连接删除、换地址、同名重建或工具撤回后，旧回执仍关联原调用。接手的 worker 从持久记录取回 Lua 修订或 MCP 绑定、外部作业号与等待条件，在当前授权下接续。

对于需要最终报告的工作，首个作业号仅更新进度。MCP 返回 `input_required` 表示调用仍需补充输入，客户端只声明实际支持的能力，无法接续时给出明确诊断。MCP Tasks 属可选扩展，支持时可把服务端给出的任务句柄关联至原等待，再按扩展查询。业务工具自己的作业号按该工具约定核查。[MCP 补输入](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#input-required-tool-results) · [MCP Tasks](https://tasks.extensions.modelcontextprotocol.io/specification/2026-07-28/tasks)

如果首次受理回执丢失，先用原句柄或首次派发前持久保存、远端可查的键核实。只有目标服务明确保证有效期内同键同参重交仍对应原作业时，才允许按原绑定重交。否则保留“结果待核实”，等待可关联的证据。JSON-RPC 请求 ID、供查询的本地调用 ID 和旧 Lua 代码都不足以证明外部操作可以重放。重复或迟到回执至多满足原等待一次。[外部工具恢复契约](contracts.md#外部工具的恢复能力)
