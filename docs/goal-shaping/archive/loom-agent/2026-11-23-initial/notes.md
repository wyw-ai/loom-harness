# loom-harness 沟通笔记

本轮讨论标识为 `initial`，文档位于 `docs/goal-shaping/`。本轮已于 2026-11-23 归档，范围覆盖云端 Agent 的目的、项目边界、首版运行能力和验收标准。启动时仓库尚无提交，也没有可接续的设计共识。

## 2026-11-19 · 启动

用户提出用目标塑形讨论 loom-agent，并在启动询问后回复“确认”。本轮从用户想解决的具体问题谈起。

## 2026-11-19 · 更名后接续

用户将仓库更名为 `loom-harness`，并要求继续目标塑形。更名的考虑和项目要承担的职责还有待讨论。

## 2026-11-19 · 为什么现在做

用户最初想做 Loom 原生的 Agent，后来先做了 loom-oi。现在用户判断 loom-oi 已基本成型，希望复用它和 Loom 的已有能力来构建 AI 产品。用户明确了群聊承担基本工作方式、OI 提供工具这两项分工。

首份共识基线见 [当前设计](../../../current/loom-agent/design.md)。本轮同步了项目目的和上述分工。

## 2026-11-19 · 目标与产品形态

用户进一步明确，目标是“让大模型更好的控制受控端”，具体形式有很多，是否仅提供 Agent 能力仍未决定。已将目标与原生 Agent 这一候选形态分开写入当前共识。用户请我们先学习三个产品，再继续讨论。

- ChatGPT 的云端浏览器提供了人和模型在同一工作现场交接控制的例子。用户可以接管浏览器，完成需要本人处理的步骤后交还控制。与之相关，dot 的消息入口和实际操作的计算机也可以分别选择。[云端浏览器](https://learn.chatgpt.com/docs/browser) · [计算机与应用](https://learn.chatgpt.com/docs/dots/computers-and-apps)
- Claude Managed Agents 将调用模型、路由工具请求的 harness 与执行命令的环境分开。对于自定义工具，应用接到调用事件后执行操作，按事件 ID 返回结果，会话据此继续。[架构说明](https://www.anthropic.com/research/managed-agents) · [工具事件与结果](https://platform.claude.com/docs/en/managed-agents/events-and-streaming)
- GitHub Copilot cloud agent 把工作放在仓库情境中，可以运行测试、形成改动，并让用户审阅 diff、继续调整或创建 PR。这个例子展示了特定任务的可检查结果。[云端工作方式](https://docs.github.com/en/copilot/concepts/copilot-surfaces/copilot-on-github) · [研究、计划与迭代](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/research-plan-iterate)

OI beta 的终端已提供结果、长执行句柄和续读。讨论中提出一个例子：同一模型用相同 OI 工具执行远程回归时，能否从进程状态继续追到实际测试结果。它是讨论建议，尚未采纳为产品范围。[OI beta 目标与交付范围](https://github.com/wyw-ai/loom-oi/blob/9f935d8cc460fb8268c423ef677557d7d491b992/docs/spec/goals/beta.md)

## 2026-11-19 · 模型调用与执行循环

用户明确 loom-harness 应具备调用大模型的能力，并询问 agent loop 如何支持多步工作。已将模型调用能力同步到当前共识；由 loom-harness 驱动模型与 OI 工具之间的循环，作为候选职责写入草稿。

## 2026-11-19 · 怎样做这个 loop

用户请我们提出做法。本轮建议在一次 Loom 工作回合内多次调用模型，以 OI 的真实返回驱动下一步，并用长命令、交互提示和结果未知三个情况检验行为。建议尚待讨论。协议参照 [OpenAI 函数调用](https://developers.openai.com/api/docs/guides/function-calling) 和 [Claude 工具结果](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls)。

## 2026-11-19 · 从循环回到 harness 的组成

用户指出，应先厘清 harness 的组成。过去 Loom 为外部 Agent 提供协作位置，受控机器与 Agent 的耦合让控制不够自然。OI 解开了这一层；harness 的动机是进一步减轻模型和 Agent 的运行负担，服务于 Loom 的“黑灯工厂”形态。已将动机同步到当前共识。

本轮提出运行配置、上下文管理、模型与工具执行、工作状态与接续四项候选职责。以夜间构建和验收交接说明角色、工作、远端进程可有不同的生命周期。组成与场景均待讨论。

## 2026-11-19 · 先盘点 Agent 的基础能力

用户调整了讨论顺序：先想清楚一个 Agent 正常工作需要哪些能力，再讨论各部分怎么做。用户先举出 LLM loop、Tool、Skill。草稿补充了职责与指令、输入与交付、上下文、会话与运行状态，并单列依场景而定的工作空间、产物和跨任务记忆。清单尚未定稿。

用户指出清单过于抽象，并追问 Instructions 是否对应 system prompt。草稿将此候选项具体化为常驻提示词，另以线程消息表示每次变化的任务要求。

用户选择从 system prompt 开始逐项讨论。草稿以验收角色举例，提出由 Agent 定义者填写角色提示词、harness 提供简短公共约定的方案，等待确认。

## 2026-11-19 · Agent、Actor 和内置指令

讨论从“谁保存提示词”转向“谁承担工作”。用户纠正了以现有 Loom 设计约束新项目的前提，明确 Loom 可以为新 harness 调整。Loom 的 Actor 用于关联协作对象，新 harness 需要对接 Actor。用户认可 harness 使用 Agent 这个名称，也明确 system prompt 是 Agent 必要的内容。用户认为 Agent 确实需要描述，头像可以保留但不推荐，把多项业务元数据放进 harness 的必要性尚不明确。

用户提出用带 metadata 的 Markdown 描述 Agent，借鉴 Skill 的渐进式披露。助手建议简介作为摘要、正文按需展开。格式、必填项、管理入口和是否加入通用运行指令都没有定案。Grok Bot 通过聊天交代长期职责的体验曾作为参照，其内部如何组装 system prompt 未据此推断。[Grok Bot 官方说明](https://docs.x.ai/grok-bot/bots)

## 2026-11-19 · 云端执行方式与场景

用户明确未来的 loom-harness 是云端 Agent 管理器，要求把 Agent 配置、message list 和 LLM loop 一起讨论。用户提出服务端 while、客户端 SDK 驱动后回传 Session、订阅事件后推进执行三种方式，并追问第三种同样能否暂停、恢复和处理并发结果。

助手先后推荐服务端循环、事件驱动、可暂停的过程式执行，期间混淆了执行位置、控制流表达和持久恢复。用户两次要求重新推敲，并要求寻找软件工程中的同构问题。可借鉴的设计包括按工作实例归集事件和决定下一步、将决策与外部执行分开。事件订阅和持久化的顺序代码均可满足部分相同的运行语义。用户未选定其中一种架构。[Process Manager](https://www.enterpriseintegrationpatterns.com/patterns/messaging/ProcessManager.html) · [Amazon SWF 决策与执行](https://docs.aws.amazon.com/amazonswf/latest/developerguide/swf-dev-tasks.html) · [Erlang 事件状态机](https://www.erlang.org/doc/system/statem.html)

用户点名了三种需要覆盖的场景：一次输入引发多轮推理和工具调用，并行工具乱序返回，异步工具稍后交回结果。用户希望用统一、稳定、健壮且易扩展的架构承载它们。助手通过流程图和回归验收时序推演了事件驱动 Run，还提出改口、部分失败和重启接续的例子。三种场景属于明确需求；Run 的设计、工具完成条件与首版 Done 仍待讨论。

## 2026-11-19 · 接续与共识同步

用户提醒检查 goal-shaping。核对时，笔记停在 system prompt 的候选用法。已在原讨论补录其后有实质变化的内容，更新候选设计，并将明确的产品定位、Agent 与 Actor 的关系、角色指令和简介、三个运行场景同步到 current。本轮讨论继续进行。同步前 current 内容 SHA-256 为 `690c1b2952b1bfd973dbffe3599ce5a0a703f8588be36b958f389410204e16bd`，同步后基线为 `7ddc7f4bb6773bcc071ccde4f61f03136dcea2ba3e268ec67af3fcd3ed49b508`。

## 2026-11-19 · 暂定事件驱动 Run

用户表示“我觉得你最近说的这个是可以的先记下来吧，我们逐步对细节”。结合前面的流程图，将云端事件驱动 Run 记为继续细化的方向，已同步到当前共识与草稿。Run 归集事件并按序决定下一步，工具可以并行，模型在需要判断时获得本轮消息。异步交办的完成与交付、Session 边界、恢复与技术选型仍待逐项讨论。本次同步后的 current 内容 SHA-256 为 `2b4e626cbe8206af022e63d2e969e4859ddb8d9b900c2f511b840ecc6046842e`。

用户指出设计文档缺少聊天中的两张原图。草稿原先只有简化流程图，现已补入完整运行流程与调用时序，并从当前共识添加入口。此次文档修订后的 current 内容 SHA-256 为 `9d8e71f8113b75820fc946da66c6d230466b84adbed66bf59bfc2438a544e4ed`。

## 2026-11-19 · 可编辑的 message list

用户明确希望 message list 可编辑，并请助手复述理解。草稿用长日志改为摘要来区分单轮编辑与后续轮次继承。编辑者、操作范围和生效时机有待对齐。此次同步后的 current 内容 SHA-256 为 `9eff19ab41a79f812ffcb5b3785bcb6d76d4d4cdc2be24bc3ccfde634a49f4dd`。

## 2026-11-19 · 外部派生上下文与事件选择

用户纠正了将“可编辑”理解为修改单条消息后是否继承的解释。明确需求是外部能针对 message list 做 branch、fork、copy 等操作，并自定义哪些事件进入列表。用户尚在考虑职责归属。助手提出由 harness 提供通用操作、外部定义选择策略的方案，作为讨论建议。当前共识内容 SHA-256 为 `1c75d502300206cfdb0617ed44625e3169c0265ab9ee92de078bbe22b9d9c7b2`。

## 2026-11-19 · 事件能否被多个 Session 消费

用户提出将 Event 与 Session 解耦为多对多。助手建议把多对多用于事件投递与消费，按接收方决定进入上下文的内容和唤起时机，工具回执保留与原调用的关联。候选方案和跨会话共享测试报告的例子已写入草稿，待继续讨论。

## 2026-11-19 · 用时序检验调度

用户要求用案例检验多对多关系的实际调度。草稿用验收和排查两个 Session 走完了事件到达顺序，并推演调换顺序、fork 边界、用户中途改口、后台回报、重复投递和挂起时结果抵达。讨论得到的候选分工是将原调用的回执关联、外部可配置的消息写入和启动或接续 Run 的条件分别表达，再把依赖同一份输入的动作按顺序提交。案例用于比较设计，尚待确认。

四个案例的时序与分支已画入[设计草稿](design.md#scheduling-cases)，供逐项讨论。

## 2026-11-19 · 当前范围回到 loom-harness 本身

用户明确要求当前只考虑 loom-harness。助手此前把具体接入产品带入了订阅例子。订阅、事件、Run 和 message list 均从 harness 自身的行为及其对调用方开放的能力来讨论。早先的 Actor 与群聊关系留作后续对接背景。当前共识内容 SHA-256 为 `4ee45663101c7b12dafc5bee2118e3ffaceca84ea4c8bf35bfd94c0a0fa111a6`。

## 2026-11-19 · 订阅条件的反例审查

用户认为条件机制可以继续推演，并要求找出漏洞。助手用五条时间线检查了漏收已完成作业、将单次尝试失败当作最终失败、修改等待后没有新事件、列表派生或再次编辑，以及写入后重启。草稿建议把等待已知结果与监听未来事件分开，按等待者记录待完成的动作，并核对模型实际读取的列表版本。这些修正仍待讨论。

## 2026-11-20 · 事件能力与规则引擎的产品边界

用户询问能否内置通用规则引擎，以及事件接入、管理和对外开放能否纳入 harness。候选方案按三项可验收的能力演进：先让 Run 可靠接续，再允许外部按权限上报与续读，最后托管可离线执行的声明式规则。草稿加入整体关系图，以同一份测试报告说明原 Run 接回、外部只读和按规则启动排查的不同结果。公开契约、规则表达能力和首版范围尚待确认。

## 2026-11-20 · 参照 DeepSeek Harness

用户希望从 DeepSeek Harness 获取设计启发。对照官方仓库 `2e39e70` 的实现和文档，记录了默认循环与插件钩子的职责、消息到达与唤醒的独立语义、fork 的截点、后台作业通知的休眠竞态，以及 Webhook 的进程内投递边界。草稿增加一个跨 worker 恢复的案例，用来讨论哪些交办需要云端保证。

## 2026-11-20 · 云端二次开发的范围

用户询问能否基于 DeepSeek Harness 二次开发，并提出 GUI 也适合作为客户端。候选方案优先用 SDK 和 Cordis 组合开发，把云端工作归属与执行进程分开。用关闭客户端、更换 worker 和外部派生上下文的流程检验公开接口能否满足目标。本轮是架构建议，尚未做原型联调或确定技术选型。

## 2026-11-20 · Cordis 的采用判据

用户要求独立判断引入 Cordis 的成本。助手收窄了先前优先基于 DeepSeek 二次开发的建议：外部可编辑上下文和云端可靠接续是当前需求，运行中动态装卸相互依赖的插件仍是假设。建议核心先采用显式组装，DeepSeek 保留为可对照的执行器。此为选型建议，尚未通过原型或性能测试。

## 2026-11-20 · Lua 扩展与可替换的模型调用

用户指定 Lua 为第三方扩展方式，并要求扩展范围可以覆盖模型调用本身。已把 Lua、多处扩展和模型调用的默认实现同步到当前设计，并按前述澄清对齐每轮 message list 的定位。候选接口区分可叠加的钩子与当次选用的模型调用实现。名称、选择方式和组合顺序仍待确定。同步后当前设计的 SHA-256 为 `39518a81b0b0166d4f074389cbf3db1b43c55b7289083ff3101347ef1864213f`。

## 2026-11-20 · 确定 Rust 核心与 Lua 插件

用户确认采用 Rust + Lua。核心预留 `PluginProvider`，本期唯一插件载体为进程内 Lua。已将这项决定同步到当前设计和草稿，预构建插件和远程回调仅作方案对照。当前设计同步后的 SHA-256 为 `9ea2410007277fcaf85662841a1df3b0868ab2c5d8f6537f7951bfad603457a4`。

## 2026-11-20 · 确定 Tool 与 MCP 的关系

此前讨论 Agent 的 MCP 和 Skill 配置时，曾建议 Agent 绑定 MCP 接入配置。用户进一步确定 Tool 支持内置实现和 Lua 插件实现。MCP 是 Lua 插件的一种工具接入方式，后续可由插件增加新来源。通用的 Agent 能力配置按 Tool 表达，MCP 连接配置归对应插件。已同步当前设计和草稿，新的共识基线为 `e5f7c7add09541755e149933a7ea8683f01987416db469a15c01cb6e4fd1ce8d`。Tool、Skill 的具体绑定字段仍待确认。

## 2026-11-20 · 调研 Codex Code Mode

用户希望了解 Codex 如何调用工具，以及云端 harness 可以借鉴什么。源码中的嵌套工具调用将原始结果返回给模型编写的 JavaScript，工具调用记录和模型可见输出可分别追踪。草稿据此补充可选的组合调用入口、子调用与外层调用的关联，以及临时脚本与持久异步作业的边界。是否提供 Code Mode 和脚本语言均属候选。

## 当前问题

<a id="agent-runtime-comparison"></a>

### Pi、Codex、Claude Code、Hermes Agent 与 OpenClaw 的运行能力对照（2026-11-23）

对照对象是 Pi coding agent、Codex CLI/App Server、Claude Code、Nous Research 的 Hermes Agent 和 OpenClaw。检索以各自官方文档、官方仓库内的说明及协议为依据，未运行五个产品做同条件测试。loom-harness 工作树目前只有设计。下表记录可观察的行为差异与作者建议，新增项均待评审。

| 能力 | 参照产品的可观察行为 | loom-harness 目前的设计及可补之处 |
| --- | --- | --- |
| 边生成边显示，断线后接回 | Pi 分开文本增量、完整消息和运行结束。Codex 可显示文字增量并读取持久历史。[Pi JSON stream](https://docs.pi.dev/latest/json) · [Codex App Server](https://developers.openai.com/codex/app-server/) | 有 Event 游标、工具进度和最终交付，实时模型片段在草稿中有过提议。尚缺对外读取同一回复、取消后保留的内容、重连后以最终消息收尾的约定。增量可由读取接口提供，无须把每个 token 都写入上下文。 |
| 运行中补充与下一个任务排队 | Pi 区分 steer 和 follow-up，OpenClaw 可将单条消息作为新的回合排在已有工作之后。[Pi RPC](https://docs.pi.dev/latest/rpc-commands) · [OpenClaw 队列](https://docs.openclaw.ai/concepts/queue) | 已有 add_input、request_cancel、start_run 和同 Run 合并。尚未明确“做完这一件再做下一件”与同时独立交办的区别、何时确认排队，以及排队期间前项取消后怎样处置。 |
| 有界执行和用量 | Claude Code 非交互调用支持轮次和估算费用上限。Hermes 跟踪迭代预算，包括父子 Agent。[Claude Code CLI](https://code.claude.com/docs/en/cli-reference) · [Hermes loop](https://hermes-agent.nousresearch.com/docs/developer-guide/agent-loop) | 已提预算耗尽应有明确状态。仍需定义 Run 用量归集、停止时机，以及用尽后怎样交付部分结果。输入窗口预算已有单独用途。还可区分反复执行相同调用且毫无进展，以及工具按约定轮询一个尚在运行的作业。[OpenClaw 工具循环检测](https://docs.openclaw.ai/tools/loop-detection) |
| 上下文诊断 | OpenClaw 可查看各项指令、Skill 简介及工具 schema 的大小，Pi 可查询会话用量。[OpenClaw context](https://docs.openclaw.ai/concepts/context) · [Pi RPC](https://docs.pi.dev/latest/rpc-commands) | 已能在 ModelCall 留存输入引用、记录压缩前缀与工具绑定。适合提供授权只读视图，解释某轮实际采用哪些来源、版本以及大致大小，估算量与提供方回报分开。 |
| 输入附件与程序可读的最终结果 | Pi RPC、Codex 支持消息携带图片；Codex 的 outputSchema 按轮指定，Claude Code 非交互模式可验证最终 JSON。[Pi RPC](https://docs.pi.dev/latest/rpc-commands) · [Codex App Server](https://developers.openai.com/codex/app-server/) · [Claude Code CLI](https://code.claude.com/docs/en/cli-reference) | 工具图片、大结果与 Tool 的返回 schema 已有设计。尚需走通用户附件到实际模型输入的路径，并允许交办方约定最终输出结构、取得校验结果。业务 outcome 验收另属产品取舍。 |
| 子任务委派与回交 | Codex 可派生及等待子 Agent，Claude Code 可给子 Agent 独立上下文和工具权限。Hermes 支持并行委派，OpenClaw 的子任务默认回报请求者。[Codex](https://developers.openai.com/codex/concepts/subagents) · [Claude Code](https://code.claude.com/docs/en/sub-agents) · [Hermes](https://hermes-agent.nousresearch.com/docs/guides/delegation-patterns) · [OpenClaw](https://docs.openclaw.ai/tools/subagents) | Dispatch、branch 与 Run 等待是可复用基础。父子运行关联、并行结果回收、权限收窄及取消归属尚缺共同契约。委托是可选扩展方向。 |
| 选模、用量与故障切换 | Codex 可按轮指定模型和推理档位，OpenClaw 可按故障类型轮换凭据或转备用模型。[Codex App Server](https://developers.openai.com/codex/app-server/) · [OpenClaw failover](https://docs.openclaw.ai/concepts/model-failover) | 本地文档已有可替换的 model.invoke 和失败重试。实际生效的模型及参数、可否安全重试、何时允许切换到另一配置仍要明确定义。 |
| 工具的实际工作环境 | Hermes 有文件和终端工具，Claude Code 的沙箱可约束文件和网络。Claude Code 的文件检查点覆盖受跟踪的编辑，Hermes 也提供可选的文件检查点。[Hermes 工具](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/tools.md) · [Claude Code 沙箱](https://code.claude.com/docs/en/sandboxing) · [Claude Code 检查点](https://code.claude.com/docs/en/checkpointing) · [Hermes 回滚](https://hermes-agent.nousresearch.com/docs/user-guide/checkpoints-and-rollback) | 已有工具授权、审批、作业句柄、后台回执和对话分支。首个执行后端可声明工作目录、可读写范围、进程控制能力及能否快照恢复，可直接验证会话分支是否也拥有独立文件状态。 |
| 跨会话学习及可选业务工具 | Hermes 能搜索历史会话和持久记忆，也能把经验编写或修订成 Skill；另有任务清单、文件和浏览器等工具。[Hermes memory](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory) · [Hermes skills](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills) · [工具目录](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/tools.md) | 渐进披露与 Section 已有。经验写回、跨会话检索与这些入口用途不同。跨会话记忆已明确暂缓，工作区指令发现由接入方承担，工具包按用途选择。 |

**与插件和 branch 有关的额外参照。** Pi 同团队的实验包 Pi Durable 允许插件声明自己的会话数据在分支时取切点版本、取最新值或重新初始化。例如从旧切点派生时，任务清单应该继承到哪一步。loom-harness 已约定材料继承与 Run 执行权归属，可以在首个保存会话状态的插件出现时补上此约定。此项仅用于后续设计。[Pi Durable](https://earendil.com/posts/pi-durable/)

建议优先用“用户能看见进行中的工作并可重连”“追加新任务的执行顺序符合意图”“达到上限后能拿到进度”验收普通单 Agent。上下文读取视图直接服务云端上下文管理定位，其余能力按使用场景选择。

用户明确本期暂缓子 Agent 委派，采纳上表“用户附件与程序可读的最终结果”“模型选择与故障切换”，暂缓工作环境和文件恢复。这是对上一轮回复中第二组四项的反馈。已同步上述范围，当前共识摘要为 `56bec2efed8b1d6dadf29563611e7783a5e4cf8d2d8d332c847299b07b50524a`。用户提出 Agent 可直接创建 Session，进一步询问如何通用地让 Agent 操作 harness，包括编写和动态安装插件。管理工具清单、包上传形式与热启用语义仍属[自管理候选](overview.md#self-management)。

DeepSeek Harness 对照取自上游提交 `4b399c39a6d1479cd84ca591b4d7c88e24d38a96`。其开发指引让 Host 只实现一次资源操作，UI 和工具用各自的入口调用，操作批准和放宽策略保留给用户。运行检查工具只提供已声明的只读查询。其 MCP 示例在开启热加载时，对比安装返回与同一 Session 实际调用 ping 的结果。[共享管理操作](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/preset/agent-preset/skills/dsh-doc/references/user-actions.md) · [运行时发现](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/preset/agent-preset/skills/dsh-doc/SKILL.md) · [安装后实测的上游用例](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/docs/user/develop/practice/dynamic-cordis.md)

用户纠正了参照重点，想了解 DSH 对 Agent 开放了哪些能力，以及 Agent 怎样完整地改变运行中的产品。查阅同一上游版本后，补充两条路径。具名工具让 Agent 使用已有功能，如查会话事件、改定时任务。开发链让它超出现有工具清单，借助运行时检查、随版本发布的开发指南和模板、普通文件工具、正式插件管理器以及实际使用结果，编写和迭代自己的扩展。DSH 的设置页直接引导用户进入 Creator 会话来定制 Agent 预设。新预设需要开新会话验证，可热加载的 MCP 则可在当前会话直接调用。对 loom-harness 的启发已补入候选草稿，仍待讨论。[预设入口](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/client/ui-agent-preset/README.md) · [编写、安装和验证](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/preset/agent-preset/skills/dsh-doc/references/plugin-development.md) · [生成的工具目录](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/docs/tool-catalog.md)


用户追问第一版自管理工具和按需开启的配置。候选方案把新工具按查契约、管理资源、读写插件版本、验证上下文列入[官方工具表](overview.md#official-tools)。建议普通 Code Mode 工具延续按需查签名，另由 Agent 在绑定上声明少数可按需开启的工具集或工具。开启结果保存在当前 Run，下次模型调用同时刷新目录、原生声明和相关 section。这样能在需要时带入插件开发说明，也能在 worker 更换后接续。接口名、首版清单及 Run 作用域均为本轮建议，尚待评审。[绑定和开启的配置示例](tools.md#on-demand-tools)

用户询问是否需要类似 DSH 的文档入口。建议首版让可自管理的 Agent 通过只读的 `harness.docs` 按需查阅核心指南和随插件版本发布的文档。普通工具发现和按需开启由工具目录与开启工具自身的简介交代，运行事实与签名各自沿用已有入口。文档只承载操作指南、示例和验证方法。该工具为候选。

按用户要求重新盘点八类设计。总览同步了 Agent 工具绑定、Run 的开启记录、按包版本提供的 Doc、manifest 文档注册和宿主建索引时机，补列与新工具配套的候选 REST 视图。32 个固定事件名已与插件、Rust 两份消费映射核对；按需开启和查文档可以沿用现有调用及通知机制。这是设计盘点，未变更已确认的交付范围。 用户追问四项已确认的模型与输入输出能力是否已经实现。当前仓库仅有设计文档，已在总览补列逐项执行契约、REST 映射、失败处理及一条可联动验收的图片用例。[四项能力的设计](overview.md#model-io)

2026-11-23，用户提议在 Delivery 之前加入消息接入插件，默认提供 HTTP 投递入口，并支持按用户配置的时间投递。用户希望比较脚本和纯文本。[候选方案](overview.md#message-ingress)将定时配置表述为投递已有事件引用或在本次时刻生成一条新消息。建议先支持纯文本和结构化数据，业务脚本随实际交办执行，按条件生产消息可由来源插件扩展。这些建议待评审。

用户追问 Agent 如何知道有哪些投递方式、怎样使用。草稿建议复用 Code Mode 的目录和工具说明，为实际发送与创建定时计划提供可选的[官方工具](overview.md#official-tools)。工具名及打包方式待评审。

用户希望由独立的一层负责实际投递之前的事情，并由该层向 Agent 提供工具。草稿据此以[投递安排层](overview.md#message-ingress)替代之前分开发现即时发送与定时计划两个工具的提案。HTTP 和 Agent 工具都是提交入口，即时和定时是同一请求可选的触发条件。层名、工具名和回执字段仍为候选。

用户认可独立投递安排层的方向，提出直接、定时和条件投递应可扩展，未来增加 Webhook 时允许选择来源并直接或按条件投递。用户和 Agent 应能分别通过 API 和工具管理同一份配置。这些方向已同步到当前共识。草稿建议来源实例可复用，每份安排选择一种触发时机、可选的单事件筛选条件、内容和目标。API 创建的来源可由 Agent 选取，API 也可修改 Agent 创建的安排。统一的安排 ID、接口名和暂停恢复语义仍为候选。同步后共识文件 SHA-256 为 `92fb8f0377dbf933c3bf9874f575ed755f1f6bda2a25cb7f48d5d9f8f65713fe`。

用户希望用脚本高度自定义条件与投递规则。已将脚本可参与两项决定同步到当前共识。上一轮字段比较和单目标不再作为规则能力的上限。草稿建议每份安排可写 Lua，单次求值返回零到多个投递意图；预览、接口名、按项重试及上下文细节仍为候选。同步后共识文件 SHA-256 为 `ad892ca9987fa7d38fe9febda2817248033f276fd76241d4d1ea6e2ffb17756a`。

<a id="context-plugin-research"></a>

**2026-11-23 · 官方上下文插件候选调研**

本次以 Codex、GitHub Copilot、Claude Code、Claude Managed Agents 和 Grok Bot 的公开产品行为为依据。下列内容供讨论，交付优先级属于建议。

| 候选 | 可借鉴的行为 | loom-harness 里的职责 |
| --- | --- | --- |
| 跨 Session 记忆 | Managed Agents 在 Session 挂载记忆库，提示词只介绍库，具体内容由工具读取。Copilot 对仓库记忆保留代码依据，使用时结合当前分支判断。Grok Team Bot 区分成员个人知识和团队采纳的知识。[Managed Agents](https://platform.claude.com/docs/en/managed-agents/memory) · [Copilot Memory](https://docs.github.com/en/copilot/concepts/prompting-and-context/copilot-memory) · [Grok Team Bot](https://docs.x.ai/grok-bot/team-bots) | 建议作为官方可选插件。Agent 绑定可用记忆库，Session 按调用者取得可读写范围。section 提供库的用途，Tool 提供按需查读、更新和忘记。一次性交办留在 Run。 |
| 大工具结果按需读取 | Claude Code 会将超限的成功且不含图片的 MCP 结果保存为文件，模型先得到路径。[Claude Code MCP](https://code.claude.com/docs/en/mcp#mcp-output-limits-and-warnings) | 在直接工具结果或 Code Mode 外层结果进入模型输入时提供短预览、调用状态和稳定引用。原文可定位、搜索或分段读取。优先复用工具方案已有的产物引用与读取权限。 |
| 工作区指令 | Codex 沿项目根到工作目录发现 AGENTS.md，Copilot 云端 Agent 支持仓库和路径规则，Claude Code 在访问匹配文件时加载相应规则。[Codex](https://developers.openai.com/codex/guides/agents-md/) · [Copilot](https://docs.github.com/en/copilot/reference/custom-instructions-support) · [Claude Code](https://code.claude.com/docs/en/memory) | 有工作区的 Agent 可启用。选择与本轮工作范围有关的规则，记下来源版本，通过上下文构建提供。普通仓库文件保留其来源和指令权限级别。 |
| 验收反馈 | Managed Agents 的 Outcomes 在独立上下文中按标准检查产物，逐项反馈交回主 Agent。[Managed Agents Outcomes](https://platform.claude.com/docs/en/managed-agents/define-outcomes) | 适合明确约定验收标准的交办。在准备对外交付之前检查候选产物，有缺项时把证据作为原 Run 的输入，沿正常触发策略决定是否再推理。按同一产物版本去重，并给返工设上限。 |

显式点选文件、代码片段或既有记录可复用投递材料和 `context.build`。可与大工具结果共用受权限和版本约束的读取能力，保留不同的选入时机。基础工具发现已在 Code Mode 的 `catalog.search` 和 `catalog.describe` 中。[Codex IDE](https://developers.openai.com/codex/ide/features/) · [工具方案](tools.md)

2026-11-23，用户进一步收敛上述四项。跨 Session 记忆暂缓。工作区规则发现属于外部应用，loom-harness 作为原子的上下文与 Agent 管理中心，只提供投递、选材、组装和 section 等可组合能力。独立 outcome 验收暂缓。轻量用例可把本次验收要求直接写入原交办 Event，由既有 Run 引用；已有检查工具的返回可作为完成依据。这个建议无须创建独立资源或评分流程。以上范围已同步当前共识，基线为 `a8112541b08c024dcd9a749ddc2a83373e550327651575d3a4320b24a9bd4bb5`。用户要求细化大工具结果如何存和看，[工具方案](tools.md#large-tool-results)现给出宿主持久存原文、模型有界预览、按引用读取和外部下载的候选设计，接口名与后端仍为建议。

<a id="always-on-agents"></a>

2026-11-23，查阅 William Hu 于 11 月 13 日发表的《Building always-on agents》。文中以持久收件箱、定时器、记忆、权限和发件箱解释 Agent 怎样跨多次唤醒持续工作。其延迟工具调用示例会结束这次 Run，收到审批后在原 Session 开启新 Run。loom-harness 可以保留一次交办对应一个 Run 的定义，等待时释放 worker，由真实回执接回原 Run。新事件何时触发模型推理，仍由运行状态和触发策略决定。[Anthropic 原文](https://claude.com/resources/articles/building-always-on-agents)

工程设计的两个检验场景是：T42 的超时提醒和完成回执同时到达，已完成的调用应合并处理，避免多跑一次推理；同一报告提供给父子 Session 作为资料时，自动通知只发往有交付约定的目标。已有 `run.reply_ready` 与交付器可以承担重试，分支各自的新交办仍能独立交付。上述为调研启发。11 月 17 日的 Claude Docs 案例另外展示了怎样用外部可检查的结果验证长时间运行的 Agent。[Claude Docs 案例](https://claude.dev/blog/chaos-loop-claude-docs-launch/)

2026-11-23，用户接受把上述启发融入 loom-harness 的原生设计。已将“收件后按当前事实决定是否推理、同一交办跨等待继续、按交付约定结束”合入[当前共识](../../../current/loom-agent/design.md)，本次同步后的基线为 `459e230da5c069efedb39b3d4ff17a706dc12f058aff664b6e487eae56751b7c`。草稿已对齐工具回执归位与触发插件的分工，并补入到期检查、审批期间重启、父子会话独立交付和发送结果未知的场景。事件名、确认方式及 Code Mode 如何交回长等待仍属实现草案。

2026-11-23，官方会话分支插件统一命名为 `branch`。接口草案采用 `/api/sessions/{id}/branches`，事件沿用 `branch.created`。之前讨论记录中的 fork 指这项能力。

2026-11-20，用户要求设计官方 fork 插件，并可参照 Codex。已核对 Codex 的 thread 历史切点和进行中回合的分叉行为。[候选方案](design.md#session-fork)用 Session 表达可独立收件的分支，冻结来源视图，另存子会话追加材料；用并行工具、迟到结果和截在摘要中间的例子检验继承范围。此次为设计讨论。

2026-11-20，用户澄清要核对的是本次压缩功能还有哪些决定需要本人作出。按既定范围，目前没有需要用户拍板的事项。上一条所列的策略、参数和实测均由实现方负责，已在[工程决策与验收](design.md#compaction-open-questions)中更正。

2026-11-20，用户希望评估对压缩的理解是否完整。已把源码核实范围、尚缺的真实工作负载验证和[影响首版契约的待决问题](design.md#compaction-open-questions)列入草稿。特别区分了“压缩期间新到的材料被保存”和“本次已固定输入的模型调用是否可以使用它”。

2026-11-20，用户提出复用 Event 传递压缩结果。草稿提出由压缩插件发布结果就绪事件，按所覆盖的输入前缀组装 message list，并区分新增工具结果、编辑已覆盖历史、恢复原构建三种情况。事件名和契约属于候选。

2026-11-20，用户进一步追问 Codex 压缩时使用哪一份消息列表以及结果如何进入下一次推理。已在[压缩设计草稿](design.md#context-compaction)补入远程结构化压缩项、文本摘要路径、活动历史替换与后台压缩期间新到消息的处理，作为实现参照。

2026-11-20，用户要求核对 Codex 实际在哪个时机压缩上下文。查阅官方 Rust 运行循环和配置，将轮首、同轮续调前、可选的轮末压缩及轮首对新输入的计量边界记入[候选流程的实现参照](design.md#context-compaction)。loom-harness 的触发策略仍待讨论。

2026-11-20，用户核对上下文压缩的设计进度。此前已有官方插件方向、组装扩展点和分支事件预计算建议；已将触发预算、压缩范围、未决调用保留、分支摘要复用和失败行为整理为[候选流程](design.md#context-compaction)。算法、阈值与排期尚待讨论。

2026-11-20，用户决定暂缓官方 `read_skill` 工具。已从本期官方工具清单移除，并将正文读取示例限定为接入插件自行提供的能力。共识文件 SHA-256 为 `3131ed4891d9f7ac6f6f96b9bb6683ff8db29cf796a3ec87970d7d8128aa2aa7`。

2026-11-20，用户明确删除补充输入、请求取消的两个专用路由，确定外部动作影响 Agent 运行时优先走事件。该决定取代上一条讨论中保留便捷路由的候选。已同步当前共识、API 总览和事件接入草稿；共识文件 SHA-256 为 `981f150146768faddf1944cf84e1910a2623efb43b91318f9c79af2f6feb6ed1`。

2026-11-20，用户追问补充输入和取消能否复用事件机制。草稿改为优先复用目标 Session 的定向投递，把原 Run 标作关联对象，保留授权、幂等和取消终态语义。是否提供便捷路由或单请求上报并投递仍是候选项。

2026-11-20，用户要求总览同时覆盖领域、REST、官方插件、扩展点、工具、事件和插件／Rust 订阅映射。已在[总览](overview.md)补齐候选目录，将“子调用回给程序、顶层等待满足才评估推理”映射到具体消费者，并区分独立订阅和唯一执行者。此轮归纳没有确定新的官方插件包或事件协议版本。

2026-11-20，用户确认推理触发、消息选材可分别由默认插件提供，并要求全部插件可通过统一入口订阅、消费事件。已同步当前设计，共识文件 SHA-256 为 `dfaab2f69a896bed15de1f1204be6e913208b6a817a99806a0370ff780c32cc1`。[统一订阅方案](design.md#plugin-events)列出了两阶段交接、消费者进度以及六组可复用场景。入口名称、阶段事件名和投递细节为实现建议。

2026-11-20，本轮把来自 API 的业务输入和内部运行事实放入逻辑事件总线，各消费者选择关注的事件。用户提出由插件机制分别承担推理触发和 message list 选材。已将两项职责同步到当前设计，共识文件 SHA-256 为 `6550a85c72af4583569e9fea16ad1bac93aba4d7d6f15790cc84fc7f1dfbe843`。默认触发条件、选材表、扩展点名称与输入截止位置是本轮建议，见[设计草稿](design.md#model-event-policies)。

2026-11-20，用户提出 Session 与 Agent 都应可以收件，并追问消费者在什么时机触发模型。修订总览草案，将送达统一定义为目标持久收件，把是否触发作为消费决定，将新建 Run 与模型请求分别追踪。Agent 新交办的默认会话选择、接收方消费规则及 Lua 扩展方式待评审。

2026-11-20，用户指出总览没有清楚展示事件管理和投递入口。总览补列事件的上报、筛选和详情，以及显式投递、逐目标查询、失败重试。EventDelivery、资源路径、动作名称与公共订阅接口属于候选，已知工具回执仍由原调用接回。

2026-11-20，用户确定业务工具默认走 Code Mode，只有显式标记的工具才直接进入模型原生工具列表。该决议取代此前将 Code Mode 作为可选调用方式的建议，已同步当前设计，共识文件 SHA-256 为 `d2622140906dccc9cfcf3efd816ba6b966c99c8a171fc738f95b0f06b18c50b3`。新增[工具层方案](tools.md)供评审，覆盖工具集成员、插件注册、模型发现、嵌套分派、并行与异步回执。字段名、模型程序语言、宿主与随包插件的分工仍属建议。

用户将 Session 手动切换工具集留到以后，沿用 Session 发起时首次组装、模型调用前检测 section 变更的流程。按本次讨论整理了领域模型、REST API、官方插件和扩展点的总览，标明已明确的能力、候选接口与举例。

用户提出用事件统一 section 变更的生效时机，在模型调用前定向同步相关插件。草稿补充分工：来源已经推送新内容时直接应用，只通知变更时再定向读取，撤出由 system prompt 插件直接处理。模型请求等待本轮截点内的变更处理完成。

用户明确了 section 的三个生命周期操作，提出工具和工具集在注册时贡献片段，由 system prompt 插件在 Session 发起时完成首次组装。Skill 工具实现端可主动上报本地技能简介。草稿建议已有 Session 在下一次模型调用时采用后续刷新或撤出，区分来源撤出和 Session 局部停用。

用户继续推演工具集在 Session 中贡献 system prompt section，提出由提示词插件提供这一接口。讨论区分 Agent 的角色配置、Session 的启用选择和每次模型请求的组装结果。工具集是否还需要注入持续生效的使用约定，是确定 section 接口范围的待讨论问题。

用户提出工具可注册进入 system prompt 的内容，Skill 因而可复用工具或工具集，harness 提供通用的渐进式披露能力。候选方案用一份跨工具的验收指南和一份纯说明的写作指南检验注册粒度。保留 Skill 内容格式和产品体验、核心是否设置独立 Skill 类型可分别决定。

用户追问已绑定技能应有哪些内容进入 system prompt。草稿明确建议在构建每轮请求时将可用技能名称、适用场景和一条共用读取说明放入系统指令。Agent 保存角色指令和绑定关系，技能正文按需进入当次任务上下文。

用户追问 Skill 是否经由 Tool 影响 message list。草稿补入连续两轮的例子，区分目录的上下文注入、模型用工具按需读取，以及请求和结果如何进入下一轮输入。外部已选中技能时可直接组装正文。

用户确认 Agent 应支持 Skill，并追问 Agent 加入的究竟是什么以及渐进式披露的作用。建议把 Agent 的可用技能引用、独立技能包和模型按需读取的过程分开描述。支持 Skill 已记入共识，绑定和加载方式仍为候选。

用户将统一配置入口确定为 `/settings/{plugin}`。已将读写和 schema 查询示例对齐到 `/api/settings/` 前缀。

用户明确需要统一 settings，允许插件注册配置项。注册后由 harness 对外暴露配置服务，生效值进入调用上下文。草稿以 MCP 请求超时说明声明、REST 读写和 `ctx.settings` 读取，建议按插件实例隔离并在一次逻辑调用内固定版本。具体路径、作用域和版本策略仍为候选。

用户要求盘点当前能确定的官方插件和扩展点。草稿补入插件交付状态与扩展位置的对应关系，区分已经提出的官方能力、明确的插件接入方式、默认实现和举例。具体插件包、扩展点名称及首发范围继续保留待定状态。

用户明确需要 REST 风格的外部管理 API，允许外部定义 Agent、MCP 和插件，各插件可以为自己的资源提供服务。用户以启用任务插件后出现 `/api/task` 为例。该能力目标已记入当前设计。草稿进一步建议由宿主统一挂载路由、构造请求上下文并发布接口描述，路径和装配细节待讨论。

用户希望将工具等能力拆成插件，并指出多个插件协作时，调用上下文、参数归属和插件管理都需要清晰的设计。草稿用工具注册和两个并行的 MCP 调用拆出装配、调用传参和异步接续的边界。具体接口和装配规则是候选设计。

用户追问内置工具和整个工具体系都能否使用 Lua 插件实现。草稿补充候选方案：内置描述随包交付，工具注册、列举和分派可以统一放进 Lua。Rust 提供宿主能力，Run 持久记录调用进度并接回异步结果。整体工具体系的实现归属待确认。

用户提供与 Hansion 的讨论：拟采用 Rust 核心与 Lua 插件，用官方插件提供压缩、fork 等能力。Hansion 提出用 SDK 编写插件，预构建后由 harness 在启动时加载。两条本地执行路径都无需每次访问远程插件服务。500 次串行扩展各增加 50 毫秒往返的 25 秒是条件估算，尚无性能测试。草稿记录支持进程内 Lua 和保留语言无关契约的建议；第二种插件载体尚未决定。

用户继续对齐公开术语。当前建议用“插件”命名 Lua 交付单元，用“扩展点”命名接入位置，仍沿用统一的注册方式。此命名待确认。

用户询问能否统一 Hook 和 Provider 的名称。草稿建议对外统一称扩展，用同一 Lua 登记方式参与各扩展点。串行修改、选择单个实现和通知观察者仍由相应扩展点分别定义。

用户询问 Hook 和 Provider 是否共用上下文。候选设计按调用共享身份，由宿主按接口传递处理后的请求、结果或错误。并行调用分别识别，可选临时数据限于本次调用和同一扩展。是否需要额外的跨插件数据交换能力，待具体用例出现时再判断。

用户进一步说明，希望第三方可以编写一份脚本，在 harness 预留的多个位置增强逻辑，Lua 是举例。草稿将上下文构建纳入通用脚本扩展机制，用“补入评审清单”和“派发前拒绝部署”展示同一脚本在两个钩子的不同作用。具体钩子名、Lua 接口和脚本版本配置仍为候选。

用户提出，Event 到 message list 的转换可以类似插件、扩展点或 mod。候选方案将每次推理前的上下文构建定义为可替换的函数。一次调用选用一个构建实现，Run 调度和模型请求继续由 harness 负责。插件是提供实现的方式，加载形式和配置归属待定。

用户进一步明确，排序、编辑、分叉和复制都可以发生在从 Event 组装模型输入的过程中。此前把 Message List 列为第五个独立实体，混合了可持续使用的分支编辑与单次模型请求。草稿改为按调用生成消息。是否由 harness 托管可复用的分支编辑仍待决定。

用户希望从整体理解未来的领域模型。草稿用 Agent、Session、Run、Message List 和 Event 作为讨论入口，以每轮模型调用实际读取的列表版本说明可编辑上下文，以工具调用的等待说明后台结果如何接回工作。此次是模型提案，Session 是否固定关联一个 Agent、订阅和技能是否由 harness 独立管理仍待确认。

用户指出“Event 又绑 Run 又绑 Session”让归属变得含糊。新建议只固定 Run 归属一个 Session。Event 描述作业、Run 或 Session 等对象发生的事实。T42 的结果可以满足原 Run 登记的等待，也可以被另一个列表引用。决定这些用途的是等待或订阅动作，事后通过处理记录或列表引用可查询。此建议待确认。

用户追问 Session、Run 与 Event 的关系。草稿建议每个 Run 有一个归属 Session，并通过 T42 报告说明原调用关联、跨会话投递、模型是否用到这份报告可以分别表达。

用户要求回到事件机制，用时序图理解整体流程。草稿现用一次回归测试展示从交办、两次推理和工具并发，到异步报告唤起原 Run、交付结论的全过程，并说明其他 Session 可以单独订阅同一事实。

用户澄清此次参照对象是 Cordis，上一轮对 DeepSeek 上层功能的展开偏离了原意。本轮以工具服务和独立的结果监听插件为例，讨论依赖如何声明、注册归谁、卸载后怎样释放。草稿将上层产品对照收起作为参考。

用户要求对齐订阅条件的机制。助手建议以结构化事件字段表示匹配条件，将一次性等待的结束条件与命中后的动作分开，并区分单事件筛选和需要记住先前事件的组合等待。该细节仍是候选方案。

用户希望先听懂四个场景，并要求独立评估其想法。补充讨论区分了“已有报告可以共享”和“未来报告到达时主动触发排查”。前者可由外部引用已有材料完成，后者才引出订阅与调度的归属。Event 与 Session 多对多仍是待检验的设想。

用户继续追问未来事件的订阅机制。草稿用已知测试作业说明登记、完成竞态、按结果投递和结束订阅，并与长期关注未来一类事件作区分。推荐先讨论外部指定接收方和动作、由云端兑现一次性等待的做法。通用订阅、规则归属和接口仍待确认。

先对齐事件选择、写入目标列表与触发推理三者的职责，以及派生后如何接收新事件。

异步工作中，用户何时能离开，结果通过什么方式送达，怎样才算这次交办完成？以这个可观察的体验细化 Run 的等待、结束和后续交付。简介的具体格式、通用指令和首版运行配置仍然开放。

## 2026-11-23 · 本轮归档

用户确认可以结束本轮目标讨论，要求保存全过程并生成首版 spec。主题沿用 `loom-agent` 标识，项目名为 loom-harness。已将当前共识、设计快照、八项总览、工具层草案、流程图与笔记关联到[规范入口](../../../../spec/README.md)。

本轮部分采纳。云端 Agent 运行、Rust 与 Lua 插件、事件投递、上下文与工具边界、模型选择与故障切换、用户图片输入和结构化结果交付进入设计基线。32 个事件的名称、具体 REST 与工具名、Run 临时开启、动态安装、文档注册格式和插件分包等保留为设计候选。子 Agent、跨会话记忆、工作环境恢复、通用官方 `read_skill` 与独立 outcome 验收暂缓。归档时仓库只有文档，运行验收仍待实现后执行。
