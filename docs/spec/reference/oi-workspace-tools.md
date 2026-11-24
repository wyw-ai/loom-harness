# 从 OI Service 登记 Toolset

官方 `internal-tools` 插件的每个实例连接一套 OIS。同一套 OIS 可以在不同 Service 中发布一台或多台 Host 的能力。Agent 选择 Service 对应的 Toolset，实际工具身份按实例、Service 和成员定位。本期联验使用 loom-oi 的 `9efec9d`，该版本的 OIS 会在实际派发时校验 MCP 调用携带的固定来源。[目标与验收](../goals/oi-workspace-tools.md) · [现有工具来源契约](tool-sources.md)

## 最小 HOME 配置

实例、实例设置和 Agent 选择分别写在 `LOOM_HOME` 下。部署方预置一个可列 Service 的目录入口，例如 `oi-catalog` 只发布 `loom-oi.list_services`。首次同步从该入口读取清单。

```yaml
# plugins/oi-office/instance.yaml
id: oi-office
package: internal-tools
version: "1.0.0"
enabled: true
---
# plugins/oi-office/settings.yaml
base_url: https://ois.example.org
catalog_service: oi-catalog
catalog_credential_ref: oi-directory
consumer_credential_ref: oi-workspaces
service_credentials:
  oi-creator: oi-creator-token
refresh_interval_secs: 30
---
# agents/assistant/agent.yaml（工具集绑定节选）
tools:
  - toolset: oi-office.workbench
grants:
  toolsets: [oi-office.workbench]
```

包的声明及入口位于 `packages/internal-tools/1.0.0/plugin.yaml` 和 `plugin.lua`。启用实例后，宿主默认每 30 秒同步。目录身份用于枚举 Service。读取工具签名和执行调用优先取 `service_credentials[service_id]`，其余 Service 使用 `consumer_credential_ref`。上例中的引用 `oi-directory` 从宿主环境变量 `LOOM_MCP_CREDENTIAL_OI_DIRECTORY` 解析。请给执行同步与调用的进程配置相应环境变量。

管理员打开「工作台 → OI 连接」，点击「连接 Loom OI」。也可按 Ctrl K 搜索「接入 OI」。表单接收实例名、OIS 地址、目录 Service 和凭据引用。保存后立即刷新，页面按 Service 列出状态、实际工具及使用它的 Agent。点击「连接到 Agent」时，网页按 Agent 当前修订一次保存授权与工具集绑定。已绑定的 Agent 可以直接打开对话，编辑示例草稿后发送。

同一流程的 API 是 `POST /api/plugins` 创建实例，`PATCH /api/settings/oi-office` 保存设置，`POST /api/plugins/oi-office/refresh` 发起同步，再通过 `GET /api/plugins/oi-office/discovery` 读取各 Service 的 `ready`、成员、错误及 Agent 命名冲突。绑定使用既有的 Agent PATCH，同时更新上例中的授权和工具集选择。Agent 详情也能核对结果。

## 通过 API 验证一次工具调用

| 时机 | 插件与宿主执行的操作 | 产物 |
| --- | --- | --- |
| 管理员启用 `oi-office` | 用目录身份连接预置的 `/mcp/oi-catalog`，调用其 `list_services({})`。发现 `workbench` 已发布 MCP 后，用消费身份连接 `/mcp/workbench`，执行 `tools/list`。 | 取得 Service 及其成员的发布名、描述和输入 schema。 |
| 后台同步成功 | 将 `workbench` 及成员作为同一修订提交到 harness 的工具目录。以 `view_on_a` 为例，记录逻辑身份、模型名与远端调用绑定。 | harness 工具目录中可查到 `oi-office.workbench` 及其成员。 |
| 通过 Agent API 或 `agent.yaml` 绑定 `workbench`，随后发起一次交办 | 保存现有 `tools: [{toolset: oi-office.workbench}]` 绑定。宿主从本地生效目录装配该 Agent 获授的直接工具。 | 首个 ModelCall 的 `tools` 含 `view_on_a(path, …)`；启动 Agent 无需再向 OIS 扫描目录。 |
| 模型调用 `view_on_a({"path":"captures/83.png"})` | 按本轮命名映射还原逻辑 Tool，派发前核对授权和绑定修订，再向 `/mcp/workbench` 发送 `tools/call`，其 `name` 为 `view_on_a`、`arguments` 为该路径对象。 | OIS 将请求送到已固定的 Host A。harness 将回执与 OI 调用号关联至该 ToolCall，供本轮和历史查询使用。 |

## 代码如何分工

- **官方包与 OI 适配。** 包负责启停、配置 schema 及 OI 来源的注册。建议在宿主增加一个小型 Rust 适配模块，提供这个官方来源所需的同步与调用入口。同步读取完整 Service 目录并协调发布工具及集合；调用接收已经鉴权的逻辑 Tool 与参数。Lua 入口只使用受限宿主能力。
- **共用 MCP 客户端。** 从现有实现分离握手、分页列工具、调用、响应和会话生命周期。通用 MCP 来源和 OI 来源分别决定如何登记身份，底层共用这套协议客户端。已有的通用 `discover` 同时会按照 MCP 连接名登记工具，不能直接作为 OI 的目录写入逻辑。[现有协议与发现代码](../../../src/catalog/mcp.rs)
- **工具目录与模型名称。** 同步在实例启用时运行，后续由后台刷新，对同一实例的过时刷新结果按修订丢弃。复用现有 Toolset 展开及派发复核。模型名需随每次请求保存双向映射，支持 OI 发布名原样进入请求并在回执中反查；现有完整 ID 转码会改写下划线。[目录与鉴权](../../../src/catalog.rs) · [现有模型名称编码](../../../src/engine/tools.rs)

## 从 OIS 同步到工具目录

1. 插件启用时读取完整可见 Service 清单，之后定期同步，配置更新也触发同步。已发布 MCP 的 Service 使用各自的 `/mcp/<service_id>` 和消费身份握手、完整读取 `tools/list`。清单给出 Service ID 和发布状态，调用入口须属于已配置的 OIS。底层协议会话由宿主管理。
2. 每个可见 Service 对应一个由该实例拥有的 Toolset。MCP 尚未发布或消费身份不能调用时，保留集合身份，标记未就绪并告知原因。对就绪 Service 的一批完整结果，原子更新其工具与成员表。完整 Service 清单中已删除的 Service 退出可选目录；超时、残缺响应和失败分页不构成删除依据。
3. OI 发布源是成员关系的依据。选择整份 Service Toolset 表示使用其持续同步的成员，展开时仍取当前 Agent 有效授权范围的交集。宿主利用已有的集合选择、目录展开及派发复核；对按整个 Service 已获授权的 Agent，插件须让新成员继承该范围。只被授权单项的 Agent 仍按单项过滤。用户的选择不能扩大部署方授予的访问范围。[现有展开与授权](../../../src/catalog.rs)
4. 登记后的工具按已选集合自动出现在模型的直接工具列表。执行时插件根据绑定将参数送到对应 Service 的 `tools/call`；ToolCall 记录逻辑 Tool ID、实际执行绑定与 OI 的调用标识。将来可增加 `POST /api/v1/exec` 适配器，Agent 配置及历史逻辑身份不依赖这个选择。

## 名字和身份

| 层次 | `workbench` Service 的例子 | 用途 |
| --- | --- | --- |
| OI 成员发布名 | `view_on_a` | 配在 OI Service 的 `actions[].name`；`source: {channel: host-a, app: filesystem, action: view_image}` 固定执行来源。 |
| harness 的 Toolset ID | `oi-office.workbench` | 实例与 Service 定位集合。 |
| harness 的逻辑 Tool ID | `oi-office.workbench.view_on_a` | 来源服务为 OI；用于授权、快照和审计。持久化时各分量独立保存或可逆编码。 |
| 模型工具名 | `view_on_a` | 直接沿用发布名，保留下划线，无 MCP 标记。 |
| 本期执行绑定 | MCP，Service `workbench`，远端成员 `view_on_a` | 协议、端点和消费身份属于可替换的执行配置。 |

同一 Agent 的多个集合若产生重名、与保留工具名冲突，或发布名不符合当前模型供应商的函数命名要求，在绑定或刷新时指出具体冲突及来源。修改 OI 的发布名即可消除歧义。模型请求记录实际名称到逻辑 Tool 的映射；链接到该次请求的回执按当时的绑定回看。

## 管理与 Skill 两类入口

一个管理 Service 可发布 `list_hosts`、`list_apps`、`list_actions` 和 Service 管理操作，自动形成管理 Toolset。一个业务 Service 可发布目标机的 `list_skills` 和 `read_skill`，与项目文件工具一起进入业务 Toolset。这里的 `list_skills(product?)` 列出 OI 目标机可读目录中的名称、简介与引用；`read_skill(ref, resource?)` 在需要时读取正文或包内材料。工具发现只同步这两个 Action 的签名，Skill 内容在调用发生时由目标 Host 提供。
