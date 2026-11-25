# OI Service 自动成为 Agent 可选的 Toolset

**2026-11-24 已确认本期目标和验收范围，插件实施及跨仓联验待开展。**

本次 harness 的目标是提供对接 OI 的官方内部工具插件。部署者接入一个 OIS，插件自动发现获准访问的 Service，把每个 Service 登记为一个 Toolset，把各 Service 发布的成员登记为 Tool。调用方通过 Agent API 或 `agent.yaml` 绑定对应 Toolset；Agent 启动沿用已有集合展开、鉴权和模型工具装配流程。

例如，OI 已有的 `workbench` Service 将 Host A 的 `computer-use.screenshot` 发布为 `capture_a`，将 `filesystem.view_image` 发布为 `view_on_a`。Agent 绑定 `workbench` 后，模型直接使用这些发布名。传输使用 MCP，工具身份属于 OI；以后执行改为 HTTP API，原 Agent 绑定和模型名字继续有效。

部署方预置目录 Service，分别配置发现和消费身份。成员按完整的 Service 同步结果原子生效。目录读取失败时保留上次完整记录并提示状态。整集合授权覆盖后续新成员，单项授权保留原范围，派发时再核权。回看时由 ModelCall 的实际模型名映射、ToolCall 和 OI 调用号定位来源及结果。

OI 的 `loom-oi` App 负责盘点 Host、已加载的 App、Action 和管理 Service，`skills` App 从目标 Host 的工作区及用户主目录按需读取已有 Skill。验收时由创造者用管理 Service 选取并发布真实 Action，由另一个获授权 Agent 绑定产生的业务 Toolset，操作 Host A 并读回指定 Skill，最后对上两侧调用记录。[目标与验收](../../../spec/goals/oi-workspace-tools.md) · [封板快照](../../archive/oi-workspace-tools/2026-11-24-service-toolsets/design.md)
