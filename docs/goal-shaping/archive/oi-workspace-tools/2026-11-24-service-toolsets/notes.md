# OI Service 工具集接入讨论记录

主题为 `oi-workspace-tools`。封板前的代码基线是 `39de0f98180dc58aad8fbd04060bc95a8a356188`，工作树已有这次接入的待实现设计。封板前当前共识的 SHA-256 为 `800243773B9B80374886065E99D5369307874F352FE6B2EC92D8AEC6982D35A5`。

- 讨论从直接接入 OI MCP、HTTP 和本机 CLI 收敛到官方内部工具来源。本期用 MCP 发现和调用，身份由 OI 实例、Service 和成员确定。模型名跟随 OI 的发布名。
- 创造者管理 Service 和目标 Host 上已有 Skill 的列举、按需读取由 OI 提供。harness 复用 Toolset 选择与工具授权，两款 App 发布的 Action 走相同的同步和派发路径。
- 2026-11-24 用户确认两仓目标与 spec 封板。已采纳的范围、配置语义与验收条件归档于[设计快照](design.md)，同步至[当前共识](../../../current/oi-workspace-tools/design.md)及[目标规范](../../../../spec/goals/oi-workspace-tools.md)。候选包 ID、凭据覆盖的字段拼写与实现参数尚待落地。此记录关闭设计讨论，功能交付仍须由实现和跨仓实测确认。
