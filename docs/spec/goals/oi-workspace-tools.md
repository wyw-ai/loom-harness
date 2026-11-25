# 内部工具插件的目标与验收

**本期已在 Windows 通过双仓联合验收。** 基线为 harness `main@39de0f9` 与 OI 的 Service beta `212b970`。本期建设对接 OI 的官方内部工具插件，通过 MCP 接入 OI 发布的工具，提供与传输方式无关的工具身份。已有通用 MCP 客户端、Toolset 装配和 OI 的 Service 发布接口构成实现基础。[当前目标](../../goal-shaping/current/oi-workspace-tools/design.md) · [封板快照](../../goal-shaping/archive/oi-workspace-tools/2026-11-24-service-toolsets/design.md) · [配置与同步契约](../reference/oi-workspace-tools.md)

## 服务端操作与验证入口

1. 部署者一次配置 OIS 地址及发现、消费身份，启用插件实例。插件启动和后续自动同步时读取可见 Service 目录及其工具签名。
2. 创造者可以通过获授的 OI 管理 Service 查询 Host、已加载 App 和 Action，创建或更新业务 Service。获授权的调用方通过 Agent API 或 `agent.yaml` 的 `tools[].toolset` 绑定业务 Service 对应的集合。
3. Agent 启动时按现有 Toolset 路径加载其已选、获授的工具。以 OI 已有的 `workbench` 为例，`computer-use.screenshot` 和 `filesystem.view_image` 对应的模型工具名分别为 `capture_a` 和 `view_on_a`。查看 Skill 目录和按引用读正文是同一套普通 ToolCall。目录条目是返回数据，每个 Skill 无须登记成一个 Tool。

## 验收

| 场景 | 可观察结果 |
| --- | --- |
| 自动发现、选择与实调 | 配一个 OIS 并在其上发布两个可见 Service。自动得到两个 Toolset，重复同步仍各一份；通过 Agent API 给获授权的 Agent 绑定 `workbench`，发起交办，核对首个 ModelCall 的工具、实际 ToolCall 和结果。 |
| 工具名与绑定稳定 | OI 成员发布名为 `capture_a`，模型函数也叫 `capture_a`。从 ToolCall 可追到 OI 实例、Service、成员和执行修订，传输字段与逻辑 ID 分开。未来可扩展 HTTP 执行；本期验收使用 MCP。 |
| 自动跟随 Service 更新 | 增删成员后，成功同步更新原 Toolset。移除 Service 后新 Agent 目录不再展示它，已撤销成员不能从旧快照发起新调用。暂时的目录查询失败保留上一份完整记录并报告不可用状态。 |
| 名称与授权 | 两个已选集合发布同名成员时，指出冲突的 Service；模型请求不会静默绑定到任意一方。未获准使用管理 Service 的 Agent 无法从集合或猜测名称调用它。 |
| 发现与消费身份分开 | 让目录身份能列出一个私有 Service，但不给实例匹配的消费身份。检查其 Toolset 保留身份和不可用原因；给实例配置匹配身份并刷新后，就绪工具只进入已获授权 Agent 的目录。 |
| 集合授权与单项授权 | 两个 Agent 都绑定同一个 Service，分别只获授一个成员和获授整个 Service。在 OI 中加入新成员并完成同步，核对两个 Agent 下次 ModelCall 的工具清单及实际可调用范围。新成员只随整集合授权生效。 |
| 两个 OIApp 走同一条路径 | 创造者通过管理 Toolset 调 `list_hosts`、`list_apps`、`list_actions` 选取 Host A 的真实来源，通过 `apply_service` 发布截图、查看图片及 Skill 列举和读取。用 `get_service` 核实生效定义。业务 Agent 绑定生成的 Toolset，调用截图并把实际返回的路径交给查看工具。在 Host A 的四个约定根放置可辨别的 Skill，默认列出所有获准根；过滤 `product=codex` 后凭 `ref` 读取指定正文和包内材料。用 harness ToolCall 与 OI `call_id` 核对实际来源和结果。删除这份测试 Service 并同步，验证新调用停止，已有调用仍可追溯。 |

跨仓联验中，OI 负责固定来源、执行和返回调用标识，harness 负责目录同步、Agent 装配、派发和回执关联。服务端验收可通过 Agent API 发起；完整 WebUI 的产品验收再从页面操作同一条链路。
