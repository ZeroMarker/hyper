# Hyper 竞品对标与执行计划

更新：2026-10-04；最近 Linux 模型评测基线：`b8464f9`（其后 macOS 文件入口改动另由原生 CI 验证）；评测用量修正：`1d2c273`。竞品证据沿用 2026-10-03 对七个产品官方文档或维护者仓库的复核。能力描述是文档证据；优先级是结合 Hyper 代码作出的产品判断。已建立 Hyper/指定模型的真实任务基线，未安装竞品跑同一任务，未比较市场份额或竞品成功率/实际成本。

本轮补 macOS 直接工具逐层 openat 描述符入口，共享读/快照源/修改 inode；跨设备路径与校验后链接替换拒绝。本地检查及实现 `cc5e81e` 的 [CI](https://github.com/ZeroMarker/hyper/actions/runs/37225246393) 全部通过，macOS arm64 原生 15 项通过；Linux 模型历史基线不冒充本轮 macOS 验证。输出/行政 API、同设备挂载别名、宿主目录移动及完整 OS 范围隔离继续待办。见 [具体边界](docs/audit-boundary.md)。

## 定位与已完成基线

Hyper 继续定位为可审计、可恢复、默认限制执行范围的本地终端编码 Agent。下一阶段目标是可靠完成真实仓库任务，并在取消、编辑和长会话失败时提供清楚的恢复路径。

最近已完成历史滑窗、总请求预算、CLI 实时 JSONL、固定任务评测脚手架、共享取消、共用权限与外部审计内容存储及 Linux 直接工具描述符边界、工具路径/命令授权与 Linux restore/undo、read-only 显式元数据修改限制与 Linux 独立库快照/共用提交；这些功能从待办基线中移除。220 个本地 Linux Rust 测试及 14 个离线评测测试通过；自动化测试数量不代表真实编码任务的成功率。

[前一轮基线](evals/baselines/2026-10-03/report.md)（`369a415`）：10 个 Rust/Python/JS 任务各跑 3 次，固定 OpenCode Go / deepseek-v4-flash / Chat、预算与并发参数，28/30 通过。3/3 错误编辑恢复通过；两次失败分别为长会话注释位置约束遗漏，以及代码正确但反复遇到策略拒绝、耗尽 12 轮。所有原始事件仅留本地，提交的逐次汇总与元数据可审阅。宿主编译环境仍被继承，后续比较须保持或隔离该环境。

[共享取消回归基线](evals/baselines/2026-10-03-cancellation/report.md)（`77858a1`）：相同 10 项任务各 3 次，30/30 通过，错误编辑恢复与长会话均 3/3；完整 usage、stdout/持久化一致性与 prune 均已检查。Responses / grok-4.6、Messages / minimax-m2.5 真实端点各完成一次 readonly-plan 冒烟，不能据此比较模型或宣称完整协议基准。两轮小样本不证明取消功能带来编码成功率提升；主动取消由本地门控与 PTY 测试单独验证。

[权限与直接审计保护回归](evals/baselines/2026-10-03-permissions/report.md)（`7824c5b`）：相同任务、模型和预算，显式 `--approval allow` 保持前轮 CLI 的写工具授权范围，28/30 通过；错误修改恢复 3/3、长会话 2/3。失败分别为首行约束遗漏，以及代码正确但排查宿主 Cargo 配置时遇到已有 `2>/dev/null` 误拒绝、耗尽 12 轮。原结果不剔除或重跑替换；小样本不作权限改动的因果判断。默认 ask/非交互拒绝由本地测试另验。

- CLI/TUI 共用执行模式；Linux shell 有 Landlock 写入/TCP、seccomp 显式 socket 边界和进程资源限制，其他平台的受限 shell 默认拒绝。
- Chat、Responses、Messages 均支持 SSE；TUI 显示增量文本，CLI `--jsonl` 输出完整持久化事件。
- 历史按完整轮次滑窗；每轮按实际协议 JSON 字节保守估算输入成本并预留输出空间，超预算在发送前失败。
- 运行固定实际历史、系统提示词和观察结果；新运行删除会话后仍能 replay。检查点覆盖直接 write/edit，shell 修改尚无同等快照覆盖。

代码依据：[运行与 replay](src/engine.rs)、[预算](src/context.rs)、[协议与配置](src/deepseek.rs)、[事件与 CLI](src/event_sink.rs)、[命令入口](src/cli.rs)、[TUI](src/tui/mod.rs)、[隔离](src/sandbox.rs)。

[外部审计存储回归](evals/baselines/2026-10-03-external-state/report.md)（`fa6101d`）：相同任务、模型、预算及明确授权，30/30 通过，错误修改恢复与长会话各 3/3；45 轮 stdout 与外部持久化 JSONL 一致，完整 usage、30 次 prune 均验证。磁盘体积改为实际外部 state，排除 workspace tmp，与此前 `.harness` 口径不直接比较。未重跑替换，小样本不推断因果；对抗边界及迁移另由本地测试验证，chmod 元数据风险仍待完成。

[Linux 描述符边界回归](evals/baselines/2026-10-03-descriptor/report.md)（`0fc6dad`）：同一任务集、模型、预算、授权与外部存储口径，30/30 通过，长会话和错误修改恢复各 3/3；45 轮完整 usage、stdout/持久化一致性及 30 次 prune 均验证。六个确定性路径替换测试另验安全边界，不将正常评测当作对抗验证；元数据、非 Linux、admin restore 与宿主目录移动竞争仍列后续，小样本不宣称质量或性能提升。

[工具范围授权回归](evals/baselines/2026-10-04-scopes/report.md)（`6382c84`）：原任务、模型、预算、授权与外部存储口径，30/30 通过；45 轮完整 usage、stdout/持久化一致性及 30 次 prune 均验证，长会话和错误修改恢复各 3/3。常规套件不启用范围规则，只验证兼容；单独真实模型任务在 read/edit/write/bash/search 基础 deny、仅允许 read/edit bounds.py 时 1/1 通过，独立行为判定、受保护 fixture、context 过滤与持久化均检查。范围拒绝由本地测试另验，不从单任务或小样本推断通用质量/安全收益。

[Linux 恢复回归](evals/baselines/2026-10-04-restore/report.md)（`ccc53be`）：相同任务、模型、预算与明确授权，28/30 通过，错误修改恢复与长会话各 3/3；45 轮 stdout/持久化一致，30 次 prune 成功。保留 Rust 跨文件任务代码判定通过但耗尽 12 轮（良性重定向误拒绝与 git 内部 `/dev/null` 访问失败），以及 Python 深文件任务流式工具参数残缺、未执行修改的失败。评测用量修正 `1d2c273` 按原事件重算，29/30 次、44/45 轮用量完整，缺失最终回复标未知，保留已观察部分；没有重跑或改变成功判定。原运行与修正的 revision/digest 分别记录，不从小样本推断恢复实现改变成功率。

[read-only 元数据回归](evals/baselines/2026-10-04-readonly-metadata/report.md)（`88672f0`）：原任务、模型、预算、授权与外部存储口径，30/30 通过，长会话、错误修改恢复、只读规划各 3/3；45 轮完整 usage 与 stdout/持久化一致，30 次 prune 成功。runner 已包含缺失回复用量修正，无需二次重算。新 read-only shell 元数据边界另由两个 BPF 判定和四个原生集成验证，普通只读规划主要使用 read/search，不能据其通过推断对抗 shell 保证。原生只验证 aarch64；workspace-write 外部 chmod 风险继续待办。保留前轮失败、不重跑替换，不从小样本归因成功率变化。

[独立库/共用快照回归](evals/baselines/2026-10-04-checkpoint/report.md)（`267d206`）：同一任务、模型、预算、授权与外部存储口径，29/30 通过；长会话、错误修改恢复与只读规划各 3/3，45 轮 stdout/持久化一致，30 次 prune 成功。保留 Python 深文件任务残缺流式工具参数失败，未执行修改/生成 checkpoint 的工具；独立判定未通过，不确定是 provider 输出还是适配问题。runner 直接将缺失最终回复标未知，29/30 次、44/45 轮用量完整，保留已知部分，无需修正或重跑。新增六个确定性/四个 API 测试另验路径替换与提交边界；本地原生只验证 Linux/aarch64 的 ext4/tmpfs，不以小样本成功率变化归因快照改动。

Chat/Responses 明确未完成拒绝与有界诊断已交付：即使参数完整，length/content_filter 与 Responses 未完成状态也不执行工具；失败事件保留计数与 JSON 位置，缺 usage 不补零，不自动重复请求。独立非流式、Messages 停止语义和 provider/model 能力仍属 P1-5。见 [规则与验证](docs/model-completion.md)。

[完成语义回归](evals/baselines/2026-10-04-completion/report.md)（`f8a1cc7`）：相同任务/模型/预算/授权，30/30 通过；长会话、错误修改恢复、只读规划与 Python 深文件各 3/3，45 轮完整 usage 和 stdout/持久化一致，30 次 prune 成功。没有出现 model.failed，拒绝/诊断语义由受控本地故障验收，不宣称前轮残缺输出根因已解决。Responses/grok-4.6 与 Messages/minimax-m2.5 各一次真实只读冒烟通过、用量/持久化一致，只证明正向兼容。保留历史失败，不作因果或竞品成功率判断。

[socket 回归](evals/baselines/2026-10-04-sockets/report.md)（`b8464f9`）：相同任务/模型/预算/授权/并发，30/30 通过，45 轮完整 usage 与持久化一致、30 次 prune 成功；错误编辑恢复与长会话各 3/3。实现 CI 全部通过，含 Linux x86 原生 socket/编译回归及 Windows 硬链接回归。初版 `edc3118` 的 [27/30](evals/baselines/2026-10-04-sockets-initial/report.md) 原样保留；Rust 子进程匿名 socketpair/send/recv 需求由确定性测试定位，例外只允许 AF_UNIX pair 与无地址收发。P0-3 不关闭，不从小样本推断总体质量因果。

## 竞品证据与直接差距

| 产品 | 本轮确认的能力与来源 | Hyper 对应差距与计划 |
| --- | --- | --- |
| Codex CLI | [`exec --json` 与 schema 输出、非交互会话续接](https://learn.chatgpt.com/docs/non-interactive-mode)；[按目录加载 AGENTS.md 与 override](https://learn.chatgpt.com/docs/agent-configuration/agents-md)。 | 实时 JSONL 已有；项目说明没有专门加载语义，最终结果没有 schema 校验。先做 P1-4 项目说明，再做 P2-1 稳定自动化契约。 |
| OpenCode | [按工具、路径和命令设 allow/ask/deny，重复调用及外部目录权限，默认敏感文件读取规则](https://opencode.ai/docs/permissions/)；[自动压缩、旧工具输出裁剪与预留空间](https://opencode.ai/docs/config/#compaction)。 | Hyper 有模式与工具白名单，且已有共用工具级 allow/ask/deny；没有摘要压缩或重复调用检测。对应 P0-3、P1-1、P1-5。权限规则与 OS 隔离分别验收。 |
| Claude Code | [项目说明、按需 skills、MCP、hooks、独立 subagents 与代码智能](https://code.claude.com/docs/en/features-overview)；[会话/代码恢复及其限制](https://code.claude.com/docs/en/checkpointing)。 | 先补项目约定和验证反馈，随后小范围 skills/MCP。官方明确 shell 修改等不在所有 checkpoint 覆盖内，Hyper 也应说明自己的快照范围，不能把 replay 当作完整文件系统恢复。对应 P1-3、P1-4、P2-2。 |
| DeepSeek Harness | [插件化 model/tool/session/loop 与 web/headless/sdk/acp 组合](https://deepseek-harness.github.io/deepseek-harness/en/reference/)；[维护者标注 developer preview 与兼容性风险](https://github.com/deepseek-ai/deepseek-harness)。 | 借鉴明确的组件边界和事件契约。其 profile 是应用/插件组合，不等于 Hyper 待实现的 provider profile；暂不复制完整插件内核。对应 P1-6、P2-1、P2-2。 |
| Pi | [树形会话、分支与保留原始条目的摘要压缩](https://pi.dev/docs/latest/how-pi-works)；[压缩摘要及来源边界记录](https://pi.dev/docs/latest/compaction)；[JSONL 双向 RPC](https://pi.dev/docs/latest/rpc)。 | Hyper 的历史裁剪没有摘要，JSONL 是输出流而非双向控制接口。共享取消已完成；下一步做可审计压缩，分支、RPC 待稳定生命周期后推进。对应 P1-1、P2-1。 |
| oh-my-pi | [维护者 README 列出内容 hash 锚点编辑、LSP、调试器和隔离 worktree 子 Agent](https://github.com/can1357/oh-my-pi)。 | 当前 edit 替换首个匹配，read 固定截取文件前部。优先验证唯一定位、陈旧文件检查和按范围读取；Hashline、LSP、调试器与并行工作需有 Hyper 实测收益后再选。对应 P1-2、P1-3、P2-3。README 的性能宣传不视为独立评测。 |
| Aider（本轮新增） | [按依赖图与相关性选择仓库 map，受 token 预算影响](https://aider.chat/docs/repomap.html)；[可配置 lint/test 与修改后的反馈](https://aider.chat/docs/usage/lint-test.html)。 | Hyper 默认摘要优先 README/Cargo/Rust 文件，其他语言上下文不足；模型自主 bash 尚不是统一验证闭环。先做提示相关的文件选择及显式验证命令，再衡量 repo map 的增益。对应 P1-2、P1-3。 |

上表只比较已读取的能力文档，不推断某产品未提供其他功能；同名功能不代表范围、默认值或可靠性相同。

## 现状复核

| 维度 | 当前实现 | 剩余差距 |
| --- | --- | --- |
| 质量 | 220 个本地 Linux Rust 测试、14 个评测测试、10 个固定任务及多轮各 30 次真实基线、范围授权与其他协议单次模型冒烟；记录首字/工具延迟、usage 与磁盘体积 | 小任务集尚不代表通用编码收益；无竞品对跑；成本未知，宿主编译环境待固定 |
| 运行控制 | 12 轮上限、共享取消、shell 超时/组清理、审批释放、独立取消结算和崩溃恢复；Linux 门控与 PTY 验证 | Windows stdout/终端渲染背压、OS 阻塞 I/O 及脱离组的 daemon 尚无完整有界取消验证 |
| 上下文 | 历史滑窗、请求估算、输出预留、用量事件 | 字节估算并非 tokenizer；无摘要压缩、相关性选择或模型能力探测 |
| 工具 | read/search/bash/write/edit，artifact、diff、快照 | read 只有前 64000 字节，edit 只改第一个匹配；缺陈旧文件校验、范围读取与验证命令闭环 |
| 执行策略 | 模式、路径校验、命令规则、共用工具级 allow/ask/deny、直接工具保护、外部审计内容边界与 Linux 描述符读取/修改与工具范围授权 | Linux chmod 仍能影响审计可用性；OS 范围隔离、非 Linux 路径授权/描述符边界、非 Linux restore/create_checkpoint、宿主目录移动竞争及完整跨平台隔离待完成 |
| 自动化 | 实时 JSONL、失败退出码、会话与 replay | 无显式事件版本/兼容性契约、结果 schema、双向控制或会话分支 |
| 配置与扩展 | 单组 provider 参数、协议覆盖、任务工具白名单 | 无命名 provider profile、连接诊断、专门项目说明或 skills/MCP 生命周期 |

## 执行队列与验收

### P0：先让质量和运行控制可验证

**P0-1 固定任务集与结果报告（已交付）。** [脚手架](evals/README.md) 包含 10 个离线可复现的 Rust/Python/JS 小仓库任务，覆盖定位、跨文件修改、深文件读取、重复文本编辑、已有测试修复、只读规划、长会话和错误修改恢复。测试 fixture 与模型评测分开；真实模型用固定 provider/model、输入、轮数和预算，每项 3 次。正式基线与逐次结果已提交，后续实现沿用这套任务验证。

验收：产出 manifest、独立成功判定脚本和 JSONL/Markdown 结果报告。报告逐次列出通过条件、错误类型、首次 delta/工具结果时间、总耗时、usage、审批和恢复结果；缺失 usage 标为未知，成本只在有价格来源与日期时计算。对齐任务、工具权限和预算后才作竞品对跑，分别报告 harness 与模型因素。

后续扩展：宿主 PATH、Cargo 配置及编译 wrapper 现已在 metadata 中记录（继承而非固定，见 [eval README](evals/README.md)）；良性重定向、`/dev/null` 读写和重复策略拒绝已固化为离线用例，剩余把宿主编译环境固定为可复现镜像并跟踪长会话约束位置遗漏；保留流式工具参数残缺样本，跟踪完成语义。基线中的 `2>/dev/null` 拒绝与 12 轮耗尽分别进入 P0-3/P1-5 验收；长会话首行约束遗漏进入 P1-1 回归。不为改善分数放宽外部路径边界。

**P0-2 共享取消生命周期（已交付）。** running → cancelled 独立于 failed/interrupted；CLI 信号和 TUI 取消键共用令牌，覆盖等待模型、三协议 SSE/JSON 读取、重试等待、审批和 shell 进程组。Unix CLI JSONL 背压可取消，外部信号无需 UI 轮询即可停止后台；关闭 TUI 等待 worker 结算。

验收：门控测试在每个等待阶段取消，约定时限内停止新请求与新工具；子进程组终止，审批等待解除，取消终结事件/summary/session 状态一致且只出现一次；流式输出已持久化，能 replay 最后一次实际请求。取消时保留已完成文件修改并明确快照范围。已有门控测试检查网络/审批/shell/Unix JSONL 等待取消在 2 秒内结算，真实 Linux PTY 验证审批 Ctrl-C 取消、同会话续聊与退出。Windows stdout、终端渲染背压和 OS 阻塞文件 I/O 列为后续，不将协作取消视为 daemon 隔离。

**P0-3 审计区保护与执行策略（部分交付，未完成）。** 工具级 allow/ask/deny、来源记录、逐次审批、CLI 非交互 ask 拒绝与直接工具/搜索/上下文的审计路径保护已交付。默认写工具均 ask，自动化需明确 `--approval allow`。前轮已复现的 shell 内容篡改由外部存储阻断，旧目录不再权威；201 个 Rust 测试包含迁移与原生 Linux 攻击回归。workspace-write 的 chmod 元数据修改仍已复现，不能将内容边界宣称为完整防伪/防拒绝服务；细节见 [审计边界说明](docs/audit-boundary.md)。P0-3b1 外部内容存储、显式迁移、WAL/session/checkpoint/replay、tmp/artifact 分离已交付；P0-3b2a Linux read/write/edit、search/context 的 openat2 入口和同 inode 快照/修改已交付，非 Linux 及宿主目录移动竞争未覆盖，Linux restore/undo 已补目录描述符与外部 staging 原子提交。P0-3b2b1 工具 literal 路径/完整命令授权、审批固定目标和 read 范围投影已交付，未缩小 shell OS 读取范围。read-only 显式元数据接口 seccomp 拒绝已补，含 ABI 校验、继承与原生测试；Linux 独立库快照源和共用输出提交已补 FD 边界与 NOREPLACE 清单发布，六个确定性/四个 API 测试覆盖；workspace-write 路径级 metadata 仍未保护，读取 atime/锁/外部服务另列边界。下一阶段继续 P0-3b2b2 元数据与 OS 范围隔离。

进一步解决 metadata 与剩余路径竞争，补 OS 范围隔离。外部状态通过 `ha state` 定位，旧工作区和移动/备份在目标第一次使用前用 `ha migrate-state --from` 导入；源写者须停止。导入不读取仓库定位文件、不覆盖已有目标，验证原始源保持、活动源拒绝、原子回滚和内容边界。

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

本轮交付 Linux 两种受限模式的显式 socket syscall 限制：UDP/Unix socket 创建/收发、io_uring 和外部 FD 导入禁止，匿名 AF_UNIX socketpair/无地址收发允许，命名本地服务受限；pipes/文件兼容。215 个 Rust 测试、14 个离线评测与 fmt/Clippy/release 通过。主动继承 socket 的通用 I/O、预建立共享映射、元数据与完整 OS 读取边界仍未交付；x86 原生已由 CI 验证，固定模型任务 30/30 通过（初版 27/30 独立保留），见 [实际边界](docs/audit-boundary.md)。

本次交付 P0-3b2b2 中 Windows 直接工具硬链接检查：真实句柄查询失败或链接计数不为 1 拒绝，解析/打开双检查，read/write/edit/search/context 共用；普通硬链接也拒绝。新增 Windows 原生 CI（3 单元 + 3 集成），实现 `9c801d9` 的 [CI](https://github.com/ZeroMarker/hyper/actions/runs/37215937962) 已全部通过；Windows x64 六项测试与 Linux 209 个 Rust/14 个离线评测分别验证。未重跑模型基线，Linux 历史报告保持原样。Windows shell/目录描述符/恢复及 Linux 元数据仍待交付，完整 P0-3 不关闭，见 [边界说明](docs/audit-boundary.md)。

下一项推进 P0-3b2b2 元数据、剩余路径竞争与 OS 范围隔离；P0-3a 共用权限、P0-3b1 外部内容存储与显式迁移已交付，完整 P0-3 保持未完成。P0-2 共享取消及 Linux 门控/PTY 验证已交付。P0-1 的 fixture、独立成功判定、脚手架及当前版本真实基线已交付；后续运行控制与 P1-2/P1-3 的改动使用同一任务集验证。已完成的 streaming、预算和 JSONL 保持回归，不重复列为新功能。

这是一份可执行顺序，不是所有阶段都必须等待前一阶段完整结束；P0 的评测和边界验证伴随每轮交付，收益未实测时保留为假设。
