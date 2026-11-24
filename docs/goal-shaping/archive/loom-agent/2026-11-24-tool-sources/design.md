# MCP 与自定义工具如何接入 Tool 和 Toolset

本次接入覆盖 MCP 工具、部署者放进 HOME 的独立 Lua 工具和作者随包登记的 Lua 工具。Skill 接入暂缓。确认的机制包括按来源登记完整目录、用显式成员组合混合 Toolset、按权限生成当轮目录，以及把结果接回原调用。下文保留协议版本和传输、导入默认值、配置路径、键名及 ID 拼写等实现候选。[工具来源规范](../../../../spec/reference/tool-sources.md#mixed-toolset) · [既有工具层契约](../2026-11-23-initial/tools.md)

## 一项发布检查

团队已有 CI 服务，要让发布 Agent 先按团队规则检查标签，再读取 CI 报告。部署者启用一个 MCP 连接、写一个本地 Lua 工具，再用一份成员表把它们组成“发布检查”。另一个 Agent 可以只选 CI 报告工具。两个 Agent 共用同一份工具定义，各自授权。

```text
HOME 的 tool.yaml + handler.lua ── 工具管理插件装载 ──┐
插件自身的 Lua 处理函数 ─────────── 插件入口注册 ──────┼─ Tool 注册表
MCPConnection ── 发现远端清单 ───── MCP 插件转换 ──────┘       │
                                                              ├─ Agent 可直接引用 Tool
                                                              └─ Toolset 引用成员 ID ← Agent 可绑定整个集合
                                                                        ↓
                                            宿主按当前身份、绑定与开启状态生成本轮目录
                                                                        ↓
                                  Code Mode 子调用或模型直接调用 → ToolCall → 唯一执行者
```

### 四份配置示例

示例的 MCP 插件实例名为 `mcp`，工具管理实例名为 `tool-registry`。前三份是独立资源，最后一份是使用方。为便于比较，四段 YAML 连写，每段顶部注明实际路径。

```yaml
# plugins/mcp/connections/ci.yaml
apiVersion: mcp.loom/v1alpha1
kind: MCPConnection
id: ci
transport: streamable-http
url: https://ci.example.org/mcp
credential_ref: ci-read
imports:
  - name: get_report
---
# tools/check_release_tag/tool.yaml
apiVersion: loom/v1alpha1
kind: Tool
id: check_release_tag
description: 校验标签是否符合团队的 v主版本.次版本.修订号 格式。
input_schema:
  type: object
  properties: {tag: {type: string}}
  required: [tag]
  additionalProperties: false
output_schema:
  type: object
  properties: {valid: {type: boolean}}
  required: [valid]
handler: handler.lua
---
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

同目录的 Lua 只维护业务逻辑。此例只读参数即可完成校验，无须申请宿主能力。

```lua
-- tools/check_release_tag/handler.lua
return function(ctx, args)
  return { valid = args.tag:match("^v%d+%.%d+%.%d+$") ~= nil }
end
```

**为什么把成员表放在工具管理实例中。** 既有设计已经允许包作者在 `plugin.lua` 中调用 `register_toolset`。新增这个实例资源，是为了让部署者也能自由组合同一目录里的工具，同时沿用 HOME 的七个顶层目录。工具管理实例负责装载和撤回这一份集合。本例中移除集合，只撤下 `tool-registry.release_check` 及其共用说明，单独选用了 CI 工具的 Agent 仍可调用获准的成员。包作者自己的 `toolsets/*.yaml` 仍由包入口解释和注册。[包内工具集示例](../../../../design-review.md#packages)

## 宿主需要记住哪些东西

| 对象 | 必要信息 | 谁使用它 |
| --- | --- | --- |
| 来源与修订 | 所有者、来源实例及授权作用域、本次完整登记的修订、是否可派发 | 宿主按来源更新或撤下登记。一个 MCP 连接和一个独立 HOME 工具可各自刷新。 |
| Tool | 稳定 ID、描述、输入 schema、可选结果 schema、`exposure`、来源及版本化执行绑定 | 模型适配器生成当轮说明，分派器解析唯一执行者。 |
| 执行绑定 | HOME 或包内 Lua 固定其代码修订与处理函数。MCP 固定插件实例、连接身份与修订、远端原名、已发现签名及配置版本 | ToolCall 用它定位一次调用。凭据按引用授权使用，模型只提交业务参数。 |
| Toolset | 集合 ID、所有者、简介、成员 Tool ID 列表、集合修订，可有共用 section | 工具管理在生成目录时展开。首批支持显式成员，一个工具可加入多个集合。 |
| 本轮目录与 ToolCall | ModelCall 记录有效工具、签名、模型函数名的反向映射及绑定版本。ToolCall 记录原调用方、可选父调用、原参数、状态和结果 | 宿主复现某轮所见，对每项实际调用授权、派发、去重和恢复。 |

**标识归属。** HOME 的 `check_release_tag` 登记为 `home.check_release_tag`。包内工具按插件实例区分。MCP 绑定保留插件实例、连接身份与远端原名。即使两个实例各有名为 `ci` 的连接、两台服务各有名为 `search` 的工具，仍能区分实际执行者。示例把三项拼成 `mcp.ci.get_report`，分隔符编码与对外拼写待定。模型提供方对函数名另有限制时，沿用当轮可逆映射。远端只改展示名可沿用原 ID，原始工具名变化按撤去旧名、登记新名处理。

删除并重建同名连接时，为新来源分配新的内部身份。目录可以再次展示相同的可读 ID，原 ToolCall 仍引用原来源身份和修订。远端清单刷新成功但成员缺失时撤下该登记，已经受理的外部作业仍能按原调用接收回执。

**谁来写目录。** 包启用、独立工具应用或一次 MCP 发现时，工具管理先暂存贡献，宿主校验所有者、重复 ID、schema 和绑定，再按来源一次提交。并发刷新按来源修订和刷新先后判定是否仍有效。只有有效目录实际变化，才更新目录版本并按需通知。原讨论中的 `register_tool`、`register_toolset` 和 `tool.catalog_changed` 为候选名称。目录通知用来刷新视图，执行按本次调用绑定派发。

## 两种工具怎样进来

### MCP 连接负责发现和协议转换

建议首个可验收组合采用服务端场景的 **Streamable HTTP + MCP 2026-07-28**。实现上保留按协议版本选择客户端适配器的位置。需要接既有 `2025-11-25` 服务时，由对应适配器处理那一版的初始化及通知；运营方可以从连接状态读到实际使用的协议与不兼容原因。stdio 涉及本地子进程的部署授权，可独立排期。工具目录和 Agent 配置不依赖传输方式。[MCP 版本与兼容性](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning) · [MCP 传输](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports)

1. **建立连接。** Lua MCP 插件读取部署者批准的地址和凭据引用，宿主提供受限的 MCP 客户端能力，负责传输、凭据使用、请求期限和当前调用身份。插件选择导入策略、转换工具及结果。可用 Rust 协议库承载通信，工具注册与连接的业务所有者仍是 MCP 插件。
2. **发现。** 确认对端支持 tools 后，插件按同一远端授权上下文逐页读取 `tools/list`。建议 `imports` 省略时导入发现的全部工具，显式空列表表示导入零项，填写列表时只导入精确匹配项。登记和授权分别发生。每个导入项可由部署者显式指定 `exposure: direct`，缺省保持 `code`。远端注解可供说明和展示，不能据此授予权限、改入口或认定副作用可重试。[MCP 工具发现与命名](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
3. **翻译与发布。** 远端 `name` 留给实际调用，`description`、`inputSchema`、可选 `outputSchema` 转入 Tool 契约。按可验证的 schema 登记，遇不支持的 schema 方言或无法安全解析的引用，显示工具名与原因。刷新过程失败或候选页间定义矛盾时，丢弃该批次，从头重取，保留上次成功的目录以供排查。Loom 能保证自身发布的目录完整切换，远端分页的一致性取决于对端。[MCP 分页与缓存](https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching)
4. **变化与隔离。** 若对端支持，按该协议版本订阅工具清单变更，收到通知后重新列举。也提供显式刷新，按服务缓存规则重验。目录缓存按实例、连接修订和实际远端授权上下文隔离。首次连接未发现或重连期间的工具可显示待发现、失联或撤权状态；未获权的使用者不能看到别的身份的工具名或签名。连接停用、换地址或撤回凭据时立即阻断旧绑定新派发，旧的在途请求仍按原调用关联。[MCP 清单变更通知](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#list-changed-notification)
5. **实际执行。** 宿主将已绑定的调用交给 MCP 插件，插件对原连接发送 `tools/call`，传远端原名和已校验参数。返回时保留有序 `content` 和可选 `structuredContent`，有输出 schema 时校验成功的结构化值。分别记录工具报告的 `isError`、协议错误、结果格式错误和通信中断。已收到格式错误的结果要留存诊断，不能因此重执行业务操作。超限文本结果由宿主自动保存为 Note，Session 和结果 Event 可经同一引用回读。[MCP 结果与错误](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#error-handling) · [Note 读取契约](../../../../spec/reference/contracts.md#notes)

首批只声明已实现的客户端能力。远端要求额外输入时，不能把 `input_required` 当完成或伪造用户回答。当前组合若无法接续，应给调用方明确的能力诊断。业务工具返回 CI 作业号时，依该工具约定区分“已经提交”和“最终报告就绪”。MCP Tasks 有自己的能力协商，后续接入时可把任务句柄映射到原有等待。普通 JSON-RPC 请求 ID 不具备外部幂等保证。[工具多轮结果](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#input-required-tool-results) · [MCP Tasks 扩展](https://tasks.extensions.modelcontextprotocol.io/specification/2026-07-28/tasks)

### 自定义 Lua 工具负责团队自己的实现

- **装载。** 独立工具由工具管理插件从 `tools/<id>/tool.yaml` 读取契约，解析同目录的相对处理函数，校验 ID、schema，编译 Lua 并确认能取得处理函数。资源内的定义和代码一起生成不可变修订。只修改 `handler.lua` 也触发新修订，编译或校验失败时原版继续生效。包内工具由该包的 `plugin.lua` 显式登记，执行绑定同时记录包版本和实例配置。
- **调用。** 宿主先按声明校验 `args`，再调用固定修订的 `handler(ctx, args)`。Lua 在受限环境内运行，`ctx` 仅包含当前调用标识和已获准使用的宿主接口。独立工具不继承加载器的管理权限。示例返回符合 schema 的普通值，宿主将其包装成原 ToolCall 的成功结果。超时、执行异常、返回值不匹配分别记录。已产生外部动作时按该动作的恢复能力核实。
- **重载。** 新建调用可选用新修订。已登记的调用保留原代码与配置引用，后继 worker 能解析这些引用。副作用已可能发生时，恢复依据是原调用记录和外部证据，不能仅因原代码仍可读取就重跑整个处理函数。[文件应用与调用接续](../../../../spec/reference/home.md#apply)

## 怎样得到 Agent 本轮可用的工具

1. 从已应用的 Agent 定义读取 Tool 和 Toolset 引用。首次启动先装载各来源，再解析集合及 Agent，避免把文件遍历顺序当成依赖顺序。明确不存在的本地 ID 或已成功发现却拼错的远端成员，配置应用报错并保留旧修订。对已配置但首次发现尚未完成的连接，标记待解析，重试发现。依赖未齐的候选不替换已有有效修订。
2. 展开已开启集合，合并单独绑定，按 Tool ID 去重。集合成员显式引用稳定 ID，MCP 新发现一个工具不会自动进入这个集合。部署者修改成员表以后，绑定该集合的 Agent 在下一轮跟随已应用的成员修订。若一个已生效集合的远端成员后来消失或连接离线，保留该引用及诊断，其他获准成员可继续使用。
3. 把候选成员与实际身份、资源权限、来源健康和启用状态取交集，固定 ModelCall 目录。默认 `code` 成员由 Code Mode 的目录和调用入口发现及执行，`direct` 成员只走模型原生入口。共用 section 跟随集合开启和可见范围。多个集合选中同一工具仍只有一个执行绑定。如采用候选的 `on_demand`，开启仅作用于本 Run，下一轮才刷新。最小验收使用常开绑定。[工具目录与按需开启](../2026-11-23-initial/tools.md#on-demand-tools)
4. 两种入口最终都创建 ToolCall，复核当轮绑定、当前权限、批准条件与参数，持久登记派发意图，随后才进入唯一执行者。原生调用的结果交回模型等待，Code Mode 的子调用结果先交给父程序。若配置已撤销或已知远端签名已变，本轮旧调用返回绑定失效，由后续模型调用读取新签名。Loom 固定的是已发现契约和目标，对端未公布的代码变更无法靠本地修订锁住。
5. 查询复用 `GET /api/agents/{id}/tools`、`GET /api/runs/{id}/tools` 和 `GET /api/runs/{id}/tool-calls` 等已有候选接口。同一授权身份可从有效目录查到“通过哪个绑定进入、工具来自哪、走哪个入口、哪一版生效”，从 ToolCall 查“实际调用了谁”。模型仅可发现本轮获准的内容；管理员的配置诊断可以说明某引用为何不可用。[既有 REST 目录](../../../../design-review.md#rest)

## 实现切片与验收

可先验证独立 Lua 工具注册、目录快照和统一派发，再并入 MCP 发现及调用，最后用混合 Toolset 串起 Agent 绑定、刷新、撤权和恢复。下表记录设计验收场景，交付时可通过同一个 Agent 入口验证。[首版混合工具集验收](../../../../spec/goals/v1.md#mixed-toolset-acceptance)

| 场景 | 可观察结果 |
| --- | --- |
| 发布 Agent 只绑定上述混合 Toolset | 一轮目录中有两项工具。标签校验进入 Lua，读报告向 CI 发一次 `tools/call(get_report)`。若两项由同一段 Code Mode 程序执行，运行记录应有一个外层调用和两个有独立 ID 的子 ToolCall。每个子调用都能查到来源、版本与父调用，模型取得外层汇总。 |
| 单工具和两个集合重复绑定，同一集合含 `code` 和 `direct` 成员 | 每个 Tool ID 只解析一次，两个入口互斥。停用其中一条绑定后，其他有效绑定所选工具仍然可用。 |
| 两个 MCP 实例均有名为 `ci` 的连接，两端都提供 `search` | 内部 ID 和执行者能区分。只授权其中一端的调用者看不到另一端的目录。目录生成后撤权，实际外发次数为零。 |
| 清单有多页、末页失败、前一连接版本的刷新晚于新版本返回 | 保留原有效目录与诊断，不发布半份清单，旧刷新不能覆盖新修订。远端正式移除工具后刷新，原混合集中本地成员继续可用。 |
| 本地脚本更新或编译失败，远端工具改签名，随后更换 worker | 新的本地调用使用成功应用的版本。失败变更保留旧版。旧模型请求不能无声改投新远端签名，历史仍能解释原绑定。 |
| CI 已受理写操作，首个回执丢失 | 宿主凭原句柄、首次派发前持久化且远端可查的键、或有效期内同键同参的幂等约定恢复。缺少这些依据时保留待核实。迟到和重复回执只满足原等待一次。[恢复规则](../../../../spec/reference/contracts.md#外部工具的恢复能力) |
