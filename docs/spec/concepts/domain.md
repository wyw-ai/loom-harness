# 领域概念

一次构建失败可以产生一个 Event。把它投给资料 Session 留档，再投给值班 Agent 排查，会产生两次独立的投递。只有排查形成一项交办，属于承接它的 Session。一个事实由此可以进入多份上下文，而每个 Run 都有唯一的归属。

| 概念 | 含义与关系 |
| --- | --- |
| Agent | 承担工作的主体。每个 Agent 有独立 spec，定义稳定 ID、名称、简介、基础 system prompt、默认模型及备选，并引用 Tool 或 Toolset。同一 Agent 可参与不同会话。 |
| AgentTemplate | 插件随包提供的 Agent 定义样例。调用方选定来源和版本并指定新的 Agent ID，由宿主校验并创建可独立维护的 Agent。首版官方模板为普通 Agent 和创造者。[模板和插件扩展字段](../reference/home.md#agent-templates) |
| Session | 连续交办的会话身份，也是可以单独收件的地址。只接收资料的 Session 可以没有 Run。 |
| Run | 一项交办从受理到交付、失败或取消的生命周期。一个 Run 归属一个 Session，由选定的 Agent 执行。一次 Run 可以跨多轮推理和多个 worker。 |
| Goal | 为一项交办显式启用的目标，由可选的官方 Goal 插件在所属 Run 内跟进。阶段性答复之后，目标仍有可推进的工作时继续同一 Run。模型根据证据显式标记完成，Run 负责最终交付。 |
| Event | 一个发生过的事实。保存来源与必要的关联，使同一内容能被多次投递、选入多个会话或供插件观察。 |
| 来源与投递安排 | 来源提供事实。投递安排根据直接请求、时间或来源条件决定何时向哪些目标投递。Lua 规则可选择投递内容和目标。评审名称为 `EventSource`、`Dispatch`。 |
| 投递与收件 | 一次实际投递连接一个 Event 与一个目标 Agent 或 Session。目标的 Inbox 保存收件记录；是否留档、发起新工作或接续已有 Run，由消费意图和目标状态决定。评审名称为 `EventDelivery`。 |
| ModelCall 与 ToolCall | ModelCall 关联一次推理决定、相应输入和结果。故障切换时应能追溯实际模型。每项工具执行可独立关联其请求与回执。Code Mode 的外层调用与内部子调用分别保留身份。 |
| Note | 归属一个 Session、由宿主在 HOME 保存的 Markdown 文本，有描述、正文和稳定引用 `note_ref`。Agent 可主动写 Note，超出存储阈值的工具文本也可自动保存为 Note。原回执保留预览和引用，Session 历史和结果 Event 可引用同一份原始版本。可按描述发现、按引用读全文或指定正文行。[Note 契约](../reference/contracts.md#notes) |
| Message list | 某次模型调用使用的有序输入。从当前 Session 选定的 Event、工具结果、图片等材料组装而成。需要重用的材料和版本可被引用，每次组装允许编辑和排序。 |
| Section | 由插件贡献并维护的一段系统指令，可注册、刷新和撤出。Agent 的基础指令与适用的 sections 共同形成生效的 system prompt。 |
| 分支与压缩 | 分支以子 Session 从既有位置独立继续；压缩产物表示一段已选历史。两者都需保留来源关系，原调用的执行与等待仍由其原 Run 承接。 |
| Tool、Toolset、Skill | Tool 是有参数与结果的可调用操作。工具集组织工具与共用说明。Skill 通过简介和按需读取正文实现渐进式披露，可由工具或插件提供。MCP 是工具接入方式。 |
| 插件包、实例、Settings、ctx | 包版本提供可安装的实现和配置声明。实例选定某一包版本，拥有自己的设置和具名资源。宿主为一次调用构造 ctx，传入身份、作用范围和已生效的配置。 |
| HOME 与资源修订 | HOME 是一个服务实例的持久目录。Agent、部署者提供的独立 Tool 和 Dispatch 等均可独立维护。一次生效修订标识通过校验的定义及其引用脚本，调用记录可据此还原执行时所用的内容。[HOME 资源规范](../reference/home.md) |

**能力装配。** 部署者的独立 Tool、插件随包工具和 MCP 工具共用注册表，Agent 按稳定 ID 选择。按需开启是候选机制，长期绑定归 Agent，临时开启记录归 Run。包安装、实例启用和当前 Run 的工具可见性各有独立含义。随包文档与工具目录的接口见[技术参考](../reference/contracts.md)。
