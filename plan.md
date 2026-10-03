# Hyper 竞品对标与执行计划

更新：2026-10-03；代码基线：`92fe90d`。本轮核对七个产品的官方文档或维护者仓库。能力描述是文档证据；优先级是结合 Hyper 代码作出的产品判断。未安装竞品跑同一任务，未比较市场份额、成功率或实际成本。

## 定位与已完成基线

Hyper 继续定位为可审计、可恢复、默认限制执行范围的本地终端编码 Agent。下一阶段目标是可靠完成真实仓库任务，并在取消、编辑和长会话失败时提供清楚的恢复路径。

最近三轮已完成历史滑窗、总请求预算和 CLI 实时 JSONL；这些功能从待办基线中移除。现有 113 个自动化测试证明相应行为，尚不能代表真实编码任务的成功率。

- CLI/TUI 共用执行模式；Linux shell 有 Landlock 写入/TCP 边界和进程资源限制，其他平台的受限 shell 默认拒绝。
- Chat、Responses、Messages 均支持 SSE；TUI 显示增量文本，CLI `--jsonl` 输出完整持久化事件。
- 历史按完整轮次滑窗；每轮按实际协议 JSON 字节保守估算输入成本并预留输出空间，超预算在发送前失败。
- 运行固定实际历史、系统提示词和观察结果；新运行删除会话后仍能 replay。检查点覆盖直接 write/edit，shell 修改尚无同等快照覆盖。

代码依据：[运行与 replay](src/engine.rs)、[预算](src/context.rs)、[协议与配置](src/deepseek.rs)、[事件与 CLI](src/event_sink.rs)、[命令入口](src/cli.rs)、[TUI](src/tui/mod.rs)、[隔离](src/sandbox.rs)。

## 竞品证据与直接差距

| 产品 | 本轮确认的能力与来源 | Hyper 对应差距与计划 |
| --- | --- | --- |
| Codex CLI | [`exec --json` 与 schema 输出、非交互会话续接](https://learn.chatgpt.com/docs/non-interactive-mode)；[按目录加载 AGENTS.md 与 override](https://learn.chatgpt.com/docs/agent-configuration/agents-md)。 | 实时 JSONL 已有；项目说明没有专门加载语义，最终结果没有 schema 校验。先做 P1-4 项目说明，再做 P2-1 稳定自动化契约。 |
| OpenCode | [按工具、路径和命令设 allow/ask/deny，重复调用及外部目录权限，默认敏感文件读取规则](https://opencode.ai/docs/permissions/)；[自动压缩、旧工具输出裁剪与预留空间](https://opencode.ai/docs/config/#compaction)。 | Hyper 有模式与工具白名单，但审批策略依赖 CLI/TUI 入口；没有摘要压缩或重复调用检测。对应 P0-3、P1-1、P1-5。权限规则与 OS 隔离分别验收。 |
| Claude Code | [项目说明、按需 skills、MCP、hooks、独立 subagents 与代码智能](https://code.claude.com/docs/en/features-overview)；[会话/代码恢复及其限制](https://code.claude.com/docs/en/checkpointing)。 | 先补项目约定和验证反馈，随后小范围 skills/MCP。官方明确 shell 修改等不在所有 checkpoint 覆盖内，Hyper 也应说明自己的快照范围，不能把 replay 当作完整文件系统恢复。对应 P1-3、P1-4、P2-2。 |
| DeepSeek Harness | [插件化 model/tool/session/loop 与 web/headless/sdk/acp 组合](https://deepseek-harness.github.io/deepseek-harness/en/reference/)；[维护者标注 developer preview 与兼容性风险](https://github.com/deepseek-ai/deepseek-harness)。 | 借鉴明确的组件边界和事件契约。其 profile 是应用/插件组合，不等于 Hyper 待实现的 provider profile；暂不复制完整插件内核。对应 P1-6、P2-1、P2-2。 |
| Pi | [树形会话、分支与保留原始条目的摘要压缩](https://pi.dev/docs/latest/how-pi-works)；[压缩摘要及来源边界记录](https://pi.dev/docs/latest/compaction)；[JSONL 双向 RPC](https://pi.dev/docs/latest/rpc)。 | Hyper 的历史裁剪没有摘要，JSONL 是输出流而非双向控制接口。先做可审计压缩和主动取消；分支、RPC 待稳定生命周期后推进。对应 P0-2、P1-1、P2-1。 |
| oh-my-pi | [维护者 README 列出内容 hash 锚点编辑、LSP、调试器和隔离 worktree 子 Agent](https://github.com/can1357/oh-my-pi)。 | 当前 edit 替换首个匹配，read 固定截取文件前部。优先验证唯一定位、陈旧文件检查和按范围读取；Hashline、LSP、调试器与并行工作需有 Hyper 实测收益后再选。对应 P1-2、P1-3、P2-3。README 的性能宣传不视为独立评测。 |
| Aider（本轮新增） | [按依赖图与相关性选择仓库 map，受 token 预算影响](https://aider.chat/docs/repomap.html)；[可配置 lint/test 与修改后的反馈](https://aider.chat/docs/usage/lint-test.html)。 | Hyper 默认摘要优先 README/Cargo/Rust 文件，其他语言上下文不足；模型自主 bash 尚不是统一验证闭环。先做提示相关的文件选择及显式验证命令，再衡量 repo map 的增益。对应 P1-2、P1-3。 |

上表只比较已读取的能力文档，不推断某产品未提供其他功能；同名功能不代表范围、默认值或可靠性相同。

## 现状复核

| 维度 | 当前实现 | 剩余差距 |
| --- | --- | --- |
| 质量 | 113 个单元/集成测试，覆盖预算、三协议、JSONL、检查点与隔离 | 无固定真实任务集；无实际提供商首字时间和成本基线 |
| 运行控制 | 12 轮上限、shell 超时、进程组清理、崩溃恢复 | 没有共享取消入口；TUI Esc 退出不等于主动取消整个运行 |
| 上下文 | 历史滑窗、请求估算、输出预留、用量事件 | 字节估算并非 tokenizer；无摘要压缩、相关性选择或模型能力探测 |
| 工具 | read/search/bash/write/edit，artifact、diff、快照 | read 只有前 64000 字节，edit 只改第一个匹配；缺陈旧文件校验、范围读取与验证命令闭环 |
| 执行策略 | 模式、路径校验、命令规则、TUI 写入审批 | CLI/TUI 的审批行为不同；无统一 allow/ask/deny 配置、受保护审计区或完整跨平台隔离 |
| 自动化 | 实时 JSONL、失败退出码、会话与 replay | 无显式事件版本/兼容性契约、结果 schema、双向控制或会话分支 |
| 配置与扩展 | 单组 provider 参数、协议覆盖、任务工具白名单 | 无命名 provider profile、连接诊断、专门项目说明或 skills/MCP 生命周期 |

## 执行队列与验收

### P0：先让质量和运行控制可验证

**P0-1 固定任务集与结果报告。** 建立 10 个离线可复现的 Rust/Python/JS 小仓库任务，包含定位、跨文件修改、深文件读取、重复文本编辑、测试修复、只读规划、长会话和失败恢复。测试 fixture 与模型评测分开；真实模型用固定 provider/model、输入、轮数和预算，每项 3 次。

验收：产出 manifest、独立成功判定脚本和 JSONL/Markdown 结果报告。报告逐次列出通过条件、错误类型、首次 delta/工具结果时间、总耗时、usage、审批和恢复结果；缺失 usage 标为未知，成本只在有价格来源与日期时计算。对齐任务、工具权限和预算后才作竞品对跑，分别报告 harness 与模型因素。

**P0-2 共享取消生命周期。** 定义 running → cancelled 与现有 failed/interrupted 的区别；CLI 信号与 TUI 取消键使用同一取消源，覆盖等待模型、SSE 读取、重试等待、审批和 shell 子进程。

验收：门控测试在每个等待阶段取消，约定时限内停止新请求与新工具；子进程组终止，审批等待解除，取消终结事件/summary/session 状态一致且只出现一次；流式输出已持久化，能 replay 最后一次实际请求。取消时保留已完成文件修改并明确快照范围。

**P0-3 审计区保护与执行策略。** 先验证 shell/直接工具对 `.harness` 中 events、DB、task、summary、checkpoint 的写入风险，设计受保护存储或挂载边界并给 artifact/tmp 单独授权。明确 CLI/TUI 共用的 allow/ask/deny、批准范围和非交互 ask 的行为。

验收：伪造/删除审计、符号链接逃逸、子 shell、重定向和重复调用均有针对性测试；策略日志固定来源与原因。仓库上下文中的 prompt injection 按不可信输入验证，提示词声明不能代替执行隔离。批准与命令规则不能扩大 OS 边界；无法隔离的平台继续明确拒绝受限 shell。UDP/Unix socket、metadata、macOS/Windows 原生隔离作为分平台后续交付，不用一个 checkbox 宣称完成。

### P1：提高真实任务完成率

**P1-1 可审计的会话与循环压缩。** 先支持手动压缩，再基于预算阈值自动触发。保留目标、约束、已修改文件、未完成事项与近期完整工具轮次；摘要记录来源消息边界、生成模型、输入/输出用量及实际消息投影。

验收：原始记录不被删除；连续压缩和会话恢复后约束仍在，工具 call/result 成对，replay 与实际请求一致。摘要生成失败或被取消有终结事件，保留可用历史，避免无界重试。预算不足的单次必要输入仍给明确错误。

**P1-2 项目上下文与范围读取。** 先实现显式文件选择和 read 的行/字节范围，再按 prompt、路径和文件类型选择上下文；覆盖 Rust/Python/JS，减少固定前部截取与 Rust 偏好。repo map/语法索引列为同任务对照实验。

验收：同预算下能定位深文件与跨语言入口，所有摘录记录路径、范围、内容来源和截断；总请求预算继续生效，rg 与内置回退结果可解释。敏感路径排除规则覆盖自动上下文、read 和 search；文件过滤与 shell 读取隔离分别报告。

**P1-3 编辑与验证闭环。** 增加唯一匹配或明确 occurrence、预期内容 hash/版本检查、原子写入和失败定位。项目可显式配置 lint/test 命令，经正常执行策略、审批、超时与预算约束运行，将失败反馈送回有限重试。

验收：重复块和陈旧文件会安全拒绝或明确定位；缺内容、空替换、换行与 Unicode 行为兼容，快照在成功修改前创建。失败验证不会被宣布成功，无限重复工具调用会停止；记录首轮编辑成功率、重试次数与任务成功率，再决定是否引入 Hashline/patch。

**P1-4 项目说明加载。** 先支持项目根 AGENTS.md，再定义目录作用域与 override 的加载顺序、字节预算和来源展示；之后再加按需 skills。项目说明是模型指导，不能改变宿主授权或工具边界。

验收：相同代码与说明生成确定的加载记录；路径/优先级/缺失/超大文件均有测试，固定实际 prompt 以便 replay。读取项目说明本身不执行脚本；按任务触及目录加载的规则须有明确范围。

**P1-5 模型能力、精确计数和重复行为。** provider/model 可配置上下文、输出、协议路径与能力；官方目录/服务端计数存在时可采用，未知时保守回退并标注。处理输出截断、缺终结标记及重复失败调用。

验收：三协议分别覆盖正常完成、长度截断、工具参数残缺、预算及未知模型；不能把实际 max_tokens 截断当作成功。重复判断固定输入/结果和阈值，避免误伤正常分页或测试重跑。统计估算与服务端 usage 差异，不把字节预算称为精确 token 数。

**P1-6 命名 provider profile 与诊断。** 兼容旧配置和 `DEEPSEEK_*`；新增命名 profile、list/use/test、明确认证来源及三协议 endpoint 覆盖。每次运行固定有效 profile 与能力配置。

验收：离线 stub 区分认证、错误路径、协议、模型、限流、超时；只做受用户触发的连接测试。配置迁移和切换不泄漏密钥，旧用法继续可用。实际端点 smoke 是可选验证并单独记录。

### P2：由使用信号决定扩展

**P2-1 稳定自动化契约。** 先写事件 schema/版本与向后兼容规则，提供最终结果 JSON Schema 验证；共享取消稳定后再做 RPC/ACP 或会话 fork。事件输出流和双向控制明确分开。

**P2-2 按需 skills/MCP。** 从一个明确外部工具场景开始，定义配置来源、信任、权限继承、超时与断连恢复；先固化工具契约，后决定是否需要 hooks 或通用插件系统。

**P2-3 并行、LSP 和调试器实验。** 先评估只读工具并发，再评估隔离会话/worktree 子 Agent。write/edit/bash 保持审批与快照顺序；LSP 以一个语言的诊断/引用用例起步。比较同任务成功率、总耗时、额外 usage 和合并冲突，再决定推广范围。

P2 共用门槛：至少一个实际任务或集成消费者证明需求；已有预算、审计和权限不退化，有兼容测试与可复现对照结果。Web UI、全量插件内核和多人协作保留在候选池。

## 接下来执行什么

先交付 P0-1 的 fixture、独立成功判定和可运行评测脚手架，建立当前版本基线；随后推进 P0-2 共享取消与 P0-3 审计区保护。P1-2/P1-3 的改动使用同一任务集验证。已完成的 streaming、预算和 JSONL 保持回归，不重复列为新功能。

这是一份可执行顺序，不是所有阶段都必须等待前一阶段完整结束；P0 的评测和边界验证伴随每轮交付，收益未实测时保留为假设。
