# 内部工具插件与 OI Service 接入封板

2026-11-24 确认需求、接入语义和验收目标。harness 基线为 `main@39de0f9`，对端为 OI Service beta `212b970`。内部工具插件及两款新 OIApp 待实现，后续按两仓联验结果判定交付。

## 部署者、创造者和业务 Agent

一个插件实例连接一套 OIS。部署方预置目录 Service，配置 OIS 地址、目录 Service ID、发现凭据引用和消费凭据引用。发现身份读取可见 Service 目录，消费身份读取获准 Service 的成员签名并执行调用。一个可见 Service 在这个实例中对应一个 Toolset，一个发布成员对应一个逻辑 Tool。Service 尚未发布 MCP 或缺少匹配的消费身份时，保留集合身份并显示未就绪原因。

创造者通过获授的管理 Toolset 盘点 Host、已加载 App、Action 和 Service，按 OI 目录返回的固定来源发布成员。业务 Agent 通过现有 API 或 HOME Agent spec 选择集合。启动时直接从生效目录装配获授工具，调用时核对当前授权和绑定。

## 以 workbench 为例

在 OI 中，Host A 的 `computer-use.screenshot` 和 `filesystem.view_image` 可按 Service 成员的 `name` 分别发布为 `capture_a`、`view_on_a`。业务 Agent 选择 `oi-office.workbench` 后，模型可以直接调 `capture_a({})`，将截图回执中的路径交给 `view_on_a`。harness 在本期经 `/mcp/workbench` 派发，按实例、Service 和成员记录逻辑身份及本次执行绑定，并关联 OI 调用号。模型使用成员发布名，包含下划线的名字保持原样。重名须指出冲突来源。未来接入 OI HTTP 执行可以复用相同身份。

完整同步成功才更新相应的目录和成员。刷新失败保留上一完整记录及诊断，确认删除或撤权后阻断新派发。整份 Service 授权随成员变动生效，单项授权按原有范围过滤。

## 跨仓完成条件

在 OI 预置管理入口。创造者查询 Host A 的来源后，创建一份同时发布截图、查看图片和 `skills.list_skills`、`skills.read_skill` 的测试 Service。harness 自动同步后，给业务 Agent 绑定相应集合，核对首个 ModelCall 的实际工具名、截图及查看结果、用 `product=codex` 列出并按 `ref` 读回的目标机 Skill，以及同一次调用在两仓的记录。更新和删除测试 Service 后核对新调用权限与历史可追溯性。

本期固定配置职责和运行行为。示例包 ID `internal-tools`、私有 Service 凭据覆盖的字段格式、刷新周期及内部模块划分留给实现设计。[目标与验收](../../../../spec/goals/oi-workspace-tools.md) · [HOME 配置与同步契约](../../../../spec/reference/oi-workspace-tools.md)
