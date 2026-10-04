# Harness Rust 迁移进度

## 当前状态

项目已经全面迁移到 Rust 1.94，核心运行时不依赖 Node.js 或 TypeScript；npm 仅作为预编译二进制的分发渠道。

2026-10-04（第九轮）：交付 P0-3b2b1 工具范围授权。宿主显式 rules 按相对 literal 路径/目录、完整 shell 命令匹配，重叠 deny > ask > allow，无命中取基础权限；请求与 canonical 目标共同判定，审批显示/固定目标，实际打开 inode 后拒绝范围硬链接。Linux 路径范围启用，非 Linux 明确拒绝；search/context 只读取 read allow，ask 留给显式 read。CLI allow 保留范围 ask/deny，ask 收紧范围 allow，deny 禁止所有 mutation；规则与匹配索引落事件。171 个 Rust 测试通过，完整 OS metadata/读取隔离、行政恢复、非 Linux 与先前历史投影仍未交付，P0-3 保持未完成。见 [实际边界](docs/audit-boundary.md)。

2026-10-03（第八轮）：交付 P0-3b2a Linux 直接文件工具描述符边界。openat2 逐层约束工作区目标，拒绝校验后 symlink/magic link/mount crossing，打开 inode 后检查审计硬链接；read/快照/修改共用同一描述符，search/context 也安全读取。搜索改为遵守 ignore 的原生文本匹配。六个确定性测试覆盖最终/父目录替换、打开后替换、后插入审计硬链接、正常内部链接与 FIFO；157 个 Rust 测试通过。元数据、非 Linux、行政恢复、宿主目录整体移动及内容陈旧编辑仍未完成；完整 P0-3 不关闭。见 [实际边界](docs/audit-boundary.md)。

本轮验证：157 个 Rust 测试（64 单元、9 取消、9 权限、54 run、16 state、5 task）、10 个评测测试、fmt、Clippy 与 release 构建通过。正式代码 `0fc6dad` 的固定任务集真实回归 30/30，长会话与错误修改恢复各 3/3；45 轮 usage 完整且 stdout 与持久化 JSONL 一致，30 次 prune 全部成功。保留原任务/模型/预算/授权及外部 state 口径，未重跑替换；小样本不宣称质量或性能提升，成本未知。见 [描述符回归报告](evals/baselines/2026-10-03-descriptor/report.md)。

2026-10-03（第七轮）：交付 P0-3b1 外部审计内容存储与显式迁移。审计路径从 canonical workspace root 派生，状态保存在 checkout 外的宿主目录，仓库不提供定位文件。`ha state` 报告实际路径，`ha migrate-state --from` 明确导入旧 `.harness`/备份；拒绝自动导入、覆盖、活动源、symlink、坏记录/schema 或越界 checkpoint。复制新 inode、SQLite 在线备份捕获 WAL、session registry 按 transcript 修复、checkpoint snapshot 重绑/target 相对化，再原子提交；源历史保留，崩溃尾片段转 artifact 避免恢复事件粘连。tmp 移入 `.hyper-tmp`，artifact 由 harness 收集至外部。

本轮验证：151 个 Rust 测试（58 单元、9 取消、9 权限、54 run、16 state、5 task）、10 个评测测试、fmt、Clippy 与 release 构建通过。Linux 实测对 events/DB/task/summary/session/checkpoint 的写、删除、rename、hardlink 阻断，预制硬链接、继承/父进程 FD 绕过拒绝；8 并发运行共享状态，4 并发导入仅一次提交；WAL/原始源、绝对 checkpoint 迁移后恢复、真实请求 replay、同会话续聊、坏数据回滚和迁移后崩溃结算均覆盖。另实测 chmod 可把外部 marker 从 0600 改为 0400；内容保护不等于元数据隔离。当前用户 mount namespace uid_map 被拒绝，下一项 P0-3b2 元数据、并发路径替换和范围权限，完整 P0-3 保持未完成。见 [边界与实测](docs/audit-boundary.md)。

正式代码 `fa6101d` 的固定任务集真实回归 30/30，错误修改恢复与长会话各 3/3；45 轮 usage 完整且 stdout 与外部持久化 JSONL 一致，30 次 prune 全部成功。使用同一模型、预算、并发与明确授权，未重跑替换；存储体积现在统计实际外部 state、排除 workspace tmp，与旧 `.harness` 口径不直接比较。不据小样本推断存储改动提升成功率，成本未知。见 [外部存储回归报告](evals/baselines/2026-10-03-external-state/report.md)。

2026-10-03（第六轮）：交付 P0-3a 共用工具权限与直接审计路径保护，完整 P0-3 保持未完成。CLI/TUI 共用 allow/ask/deny，默认 read/search allow、bash/write/edit ask；CLI 无交互处理器时 ask 明确拒绝，自动化需显式 `--approval allow`。`--permissions FILE`、HYPER_APPROVAL 与参数优先级、有效权限及来源固定到事件，未知配置失败关闭。审批只授权当前调用，allow 不扩大 plan、白名单、工作区或 OS 边界。旧库 API 保持原审批行为，新 RunOptions 使用共用策略。

直接 read/write/edit 拒绝 `.harness` 及符号链接/Unix 硬链接别名，search 和自动上下文同步过滤，审批后重复校验文件目标。135 个 Rust 测试（58 单元、9 取消、9 权限、54 run、5 task）、10 个离线评测测试通过，fmt、Clippy 与 release 构建通过。在一次性仓库实测：明确 allow 后，workspace-write shell 仍能写 `.harness/probe.txt`；没有把命令过滤或直接工具保护当成完整审计防伪。下一项是 P0-3b shell 审计存储隔离、独立 artifact/tmp 授权与范围规则，另保留 Windows 硬链接和并发路径替换保护。见 [边界说明与复现](docs/audit-boundary.md)。

正式代码 `7824c5b` 的固定任务集回归 28/30，错误编辑恢复 3/3、长会话 2/3；使用明确 `--approval allow` 保持前轮 CLI 的写工具授权范围，完整 usage、持久化流一致性及 prune 均验证。保留两次失败：首行注释约束遗漏，以及代码正确但遇到已有 `2>/dev/null` 误拒绝、耗尽 12 轮。没有剔除或重跑替换，也不据小样本推断权限实现改变成功率。见 [真实回归报告](evals/baselines/2026-10-03-permissions/report.md)。

2026-10-03（第五轮）：完成 P0-2 CLI/TUI 共享取消。共享令牌覆盖模型响应头、三协议 SSE/JSON、重试、审批与 shell；CLI 信号取消返回 130，TUI Ctrl-C/运行中 Esc 或 `/cancel` 取消，审批 Esc 只拒绝当前操作。取消独立结算为 cancelled，保留已完成修改、输出、检查点与可 replay 的实际请求；退出等待后台 worker，外部信号直接传递到后台。Unix CLI JSONL 背压可取消。Windows stdout、终端渲染背压、OS 阻塞文件 I/O 及脱离进程组的 daemon 仍列后续，不宣称所有平台 I/O 有界。

本轮验证：125 个 Rust 测试（57 单元、9 取消集成、54 run 集成、5 task 集成）、10 个评测测试、fmt、Clippy 与 release 构建通过。门控测试覆盖网络/审批/shell/Unix 背压取消在 2 秒内结算，真实 Linux PTY 验证取消审批后同会话续聊与退出。正式代码 `77858a1` 上 OpenCode Go / deepseek-v4-flash / Chat 固定任务集 30/30 通过，错误修改恢复与长会话各 3/3，完整 usage、持久化流一致性和 prune 已检查。Responses / grok-4.6、Messages / minimax-m2.5 真实端点 readonly-plan 冒烟各 1/1；仅是单任务联通验证。见 [回归报告](evals/baselines/2026-10-03-cancellation/report.md)。前轮 28/30 与本轮小样本不足以把成功率变化归因于取消实现，成本未知。下一项为 P0-3 审计区保护与统一审批策略。

2026-10-03（第四轮）：完成 P0-1 固定任务评测。`evals/suite.json` 定义 10 个 Rust/Python/JS 离线任务，`fixtures.py` 生成全新仓库，`grade.py` 在模型工作区外判定行为与约束，`run.py` 固定模型/端点/协议/预算并记录延迟、usage、策略事件、恢复及 prune 前后磁盘体积。原始错误实现与已知正确实现都有测试；本地 HTTP stub 驱动真实 CLI，覆盖未知/部分用量、失败、超时清理、绝对路径 checkpoint 恢复和配置覆盖。离线评测已接入 CI。

本轮验证：113 个 Rust 测试、9 个离线评测测试和 debug/release 构建通过。正式基线固定评测代码 `369a415`、OpenCode Go / deepseek-v4-flash / Chat，每项 3 次，共 28/30 通过，错误编辑恢复 3/3 通过。两次失败分别为长会话首行注释要求遗漏，以及独立代码判定通过但策略拒绝后耗尽 12 轮；均保留在 [正式报告](evals/baselines/2026-10-03/report.md) 中，不剔除或重跑替换。30 次均验证 stdout 与持久化事件一致，完整 usage 见逐次 JSONL，成本未知。宿主 PATH/Cargo 配置/编译 wrapper 尚未隔离，已新增后续待办。待办下一项为 P0-2 CLI/TUI 共享取消。

2026-10-03（第三轮）：完成 CLI 实时 JSONL。`--jsonl` 支持 run/plan/build/直接 prompt 及会话，每条事件在 JSONL 文件与 SQLite 写入后立即输出并 flush；stdout 与审计日志完全一致。运行成功/失败终结事件保留 summary/failure，失败退出码为 1，诊断在 stderr；不打开配置向导。慢消费者施加背压；输出/flush 失败返回错误，已写审计事件保留，未完成运行在下次打开工作区时恢复为 interrupted。TUI 继续使用原有有界队列。

本轮验证：54 单元 + 54 run 集成 + 5 task 集成，共 113 测试通过；fmt、Clippy（warnings 视为错误）和 release 构建通过。门控 SSE 测试确认 CLI 在提供商完成前收到 delta，另覆盖完整日志一致性、四种入口、会话、任务失败退出码、缺少配置、非运行命令拒绝，以及 flush 故障阻止工具执行并保留审计。

2026-10-03（第二轮）：完成总请求预算。`HYPER_CONTEXT_TOKENS`（默认 128000）预留 `HYPER_OUTPUT_TOKENS`（默认 8192），三种协议均发送输出上限。每轮按实际协议 JSON 请求体的 UTF-8 字节数估算，覆盖工具定义、参数和观察结果；首次请求可继续裁掉完整旧会话，必要上下文超预算时在发送前以 `ContextBudgetError` 失败。每轮预算事件可审计，TUI 显示用量；replay 排除超预算而未发送的后续消息。模型能力探测与精确 tokenizer 仍在待办。

本轮验证：54 单元 + 48 run 集成 + 5 task 集成，共 107 测试通过；fmt、Clippy（warnings 视为错误）和 release 构建通过。新测试覆盖输出预留和配置校验、三协议流式/非流式输出上限、首次超预算、完整历史裁剪及工具输出增长后的准确 replay。stub 服务现在按 Content-Length 读完整请求，避免网络分片造成请求体记录不完整。

2026-10-03：新增会话历史滑窗预算 `HYPER_HISTORY_TOKENS`（默认 16000，0 禁用），按 UTF-8 字节与消息开销保守估算，保留最近完整用户轮次。原始会话保留；模型开始事件固定实际历史、系统提示词及裁剪计数，replay 在忘记会话后仍可还原新运行。总请求预算（当前输入、工具定义和本轮观察结果）仍是独立后续事项。

本轮验证：52 单元 + 45 run 集成 + 5 task 集成，共 102 测试通过；`cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings` 与 `cargo build --release --locked` 通过。新增回归覆盖整轮预算、中文估算、超大轮次、非法预算，以及带工具调用的裁剪历史在删除会话后与实际请求一致的 replay。

## 已完成

- [x] Clap CLI，生成 `hyper` 主命令和 `ha` 短命令：`config`、`init`、`validate`、`run`、`plan`、`build`、`runs`、`show`、`diff`、`artifacts`、`checkpoints`、`restore`、`undo`、`tui`。
- [x] 短命令 `hy` 改名为 `ha`：新增 `src/bin/ha.rs` 与 Cargo bin target，删除 `hy`；npm `bin`、Release 归档（`ha`/`ha.exe`）、smoke 校验同步改名；`ha` 与 `hyper` 是同一程序（`--version` 均报 `hyper`，usage 行按 clap 默认显示实际调用名）。
- [x] Serde task/event/failure/summary 数据模型及重复 step id 校验。
- [x] JSONL 事件事实日志和 Rusqlite 本地索引。
- [x] `.harness` workspace、run artifacts、sessions 和 checkpoint/undo。
- [x] `read`、`write`、`edit`、`bash`、`search` 工具。
- [x] plan/build 策略、路径越界防护（含符号链接逃逸防护）和危险 shell 命令拦截。
- [x] Ratatui + Crossterm 全屏 TUI，直接调用 Rust 核心，无子进程桥接。
- [x] 默认接入 DeepSeek OpenAI-compatible API；默认模型 `deepseek-v4-flash`，支持环境变量覆盖。
- [x] tool-calling agent loop：模型在 loop 中自主调用 `read`/`search`/`bash`/`write`/`edit`，观测结果回传直至产出最终答复（上限 12 轮）；plan 模式只暴露只读工具；`tools` 字段作为白名单。
- [x] TUI 交互审批：`bash`/`write`/`edit` 执行前弹窗确认（`y`/`n`），agent loop 内同样生效。
- [x] shell 进程组终止：超时杀死整个进程组，避免残留子进程（Unix/Windows 双平台实现）。
- [x] 快照/diff 命令：`ha diff`、`ha artifacts`、`ha checkpoints`、`ha restore <run> <checkpoint-id>`。
- [x] 跨平台发布流水线（Windows/macOS/Linux，tag `v*` 触发，自动上传 GitHub Release）。
- [x] 兼容原有 task JSON、workspace 目录和 SQLite schema。
- [x] 删除 TypeScript 源码、npm manifest、Vitest 和 Node 构建产物。

## 缺陷修复（本轮）

代码审查加实测发现的缺陷，已全部修复并补上回归测试：

- [x] **`bash` 管道死锁（严重）**：stdout/stderr 原先在子进程退出后才读取，命令输出超过管道容量（Linux 约 64 KiB）时双方互锁，只能等到超时被杀。现在边运行边抽干两条管道；当前每路最多保留 4 MiB，其中 256 KiB 进入事件，超出保留量的部分丢弃并标记 `truncated`。
- [x] **超时与普通失败不可区分**：超时现在记录 `"timedOut": true`，失败信息为 `command ... timed out after Nms and was killed`，`errorType` 为 `TimeoutError` 且 `retryable` 为 true。
- [x] **失败任务退出码为 0**：`ha run` / `ha plan` / `ha build` / 直接 prompt 在 run 未 finished 时以退出码 1 结束（stdout 仍打印 summary）。
- [x] **`write:` 缺内容行会静默清空文件**：缺少内容行直接报错；`write:a.txt\n` 仍表示写入空文件。同时 `write:` / `edit:` 内容改用未 trim 的原始 instruction，避免尾部换行被吃掉。
- [x] **崩溃后 run 永久停留在 `running`**：新增 `runs/<id>/lock` 咨询锁（进程退出即释放，含 SIGKILL），启动时把无人持锁的 `running` run 修复为 `interrupted`，并补写 `run.interrupted` 事件与 `summary.json`；仍在运行的 run 不受影响。
- [x] **孤儿进程**：bash 进程组增加 Drop 守卫（提前返回/panic 也会清理），Linux 上通过 `PR_SET_PDEATHSIG` 让 shell 随 harness 一同退出。
- [x] **脆弱测试**：`bash_timeout_kills_entire_process_group` 原先用 `pgrep -f "sleep 60"` 判定，会命中宿主上无关进程；现在由命令自报子进程 pid 并检查 `/proc/<pid>`。
- [x] **clippy**：`engine.rs` 的 test module 移到文件末尾，`cargo clippy --all-targets -- -D warnings` 通过。
- [x] **HTTP 连接复用**：`DeepSeekConfig` 持有 `reqwest::blocking::Client`，agent loop 各轮不再重复建连/TLS 握手。
- [x] **CLI 一致性**：`ha plan fix the bug`（多词不引号）现在可以解析。
- [x] 新增 14 个回归测试（大输出不死锁、输出截断、超时分类、write 缺内容行、崩溃修复、存活 run 不被误修复、harness 被杀后命令不再存活、cli 退出码、多词 prompt 等）。

## 发布渠道：npm

- [x] 新增 npm 分发渠道（`npm/`），采用 esbuild/biome 的「主包 + 平台子包」结构：`hyper-agent` 只含一个 Node shim（`npm/bin/hyper.js`，约 3KB 打包体积），各平台二进制放在 `hyper-agent-linux-x64`、`hyper-agent-linux-arm64`、`hyper-agent-darwin-x64`、`hyper-agent-darwin-arm64`、`hyper-agent-windows-x64`，由主包的 `optionalDependencies` 按 `os`/`cpu` 自动择一安装。shim 不硬编码平台矩阵：它遍历已声明的平台包，用包内 `os`/`cpu` 确认匹配后，从包内 `hyper.binary` 字段取二进制路径。
- [x] Windows 平台包命名为 `hyper-agent-windows-x64`（不是 `hyper-agent-win32-x64`）：后者被 npm 反垃圾启发式确定性拒绝（`403 Package name triggered spam detection`，疑似与 `@esbuild/win32-x64` 这类平台包命名撞形），实测重试无效、其余 4 个包同一秒内发布成功。
- [x] shim 只做定位与转发：`stdio: inherit`（TUI 保有真实 TTY）、退出码原样传递、SIGINT/SIGTERM/SIGHUP 转发；缺少平台包时给出可操作的报错而不是堆栈；无生命周期脚本、安装期不下载任何东西；支持 `HYPER_BINARY_PATH` 指向自编译二进制；`ha` 与 `hyper` 链接到同一 shim。
- [x] `npm/platforms.json` 作为平台矩阵唯一来源（npm 包名、`os`/`cpu`、release artifact 后缀、目标三元组），`npm/scripts/publish.mjs` 据此暂存并发布；平台包先发、主包后发。
- [x] release 流水线扩展：build 矩阵新增 `linux-arm64`（使用公开仓库免费的 `ubuntu-24.04-arm` 原生 runner，避免交叉编译 bundled SQLite / ring）；新增 `npm` job，在上传前先暂存并 smoke test，已存在的版本自动跳过（可重复执行），手动触发默认只做演练。
- [x] `npm/scripts/smoke.sh`：打包真实 tarball → 用用户路径安装（`npm pack` + `npm install`）→ 校验 `hyper`/`ha` 可执行、版本正确、失败 run 退出码透传、缺平台包时的报错。
- [x] 本地已完整验证该链路（aarch64 Linux + 真实 release 二进制）：平台包 3.9MB 压缩 / 9.1MB 解压，主包 3.3KB / 4 个文件，`hyper --version` 经 shim 输出 `hyper 0.1.0`。

npm 发布顺序陷阱：npm 对 publish 是异步处理的——命令返回成功、日志里打印 `+ pkg@ver`，此时包可能仍在队列中（`Your package is being processed and may take a few minutes to become available`）。而安装器对「暂时解析不到的 optionalDependency」是**静默跳过**的，于是会出现「主包已可见、平台包还没可见」的窗口，用户装完只有 shim 没有二进制。已在 `publish.mjs` 中修掉：平台包全部发布后轮询 `registry/<pkg>/<version>` 直到可见，才发布主包；超时（10 分钟）则直接失败并提示重跑（已发布的包会被跳过），确保不会出现无二进制的版本。

npm 包名：主包必须叫 `hyper-harness` —— `hyper-agent` 被 npm 相似度检查永久拒绝（`403 Package name too similar to existing package hyperagent`，后者是 2022 年的无关包），`hyper-coding-agent`/`hyper-agent-cli` 这类变体归一化后仍含 `hyperagent`，风险高；平台子包沿用首发时的 `hyper-agent-*` 前缀不动。

发布前置条件：仓库需要配置 `NPMJS_TOKEN` secret，workflow 会以 `NODE_AUTH_TOKEN` 传给 npm。必须是 classic **Automation** token（或勾选 Bypass 2FA 的 granular token）：classic *Publish* token 会在 CI 里以 `EOTP`（需要一次性验证码）失败——首次发版即因此失败过一次。


## 模型配置

配置文件 `~/.config/hyper/config.json`（`hyper config` 写入，0600）：

```json
{ "deepseek_api_key": "sk-…", "base_url": "https://api.deepseek.com", "model": "deepseek-v4-flash" }
```

优先级：`DEEPSEEK_API_KEY` / `DEEPSEEK_BASE_URL` / `DEEPSEEK_MODEL` 环境变量 > 配置文件 > 内置默认值（`https://api.deepseek.com` + `deepseek-v4-flash`）。旧版只含 `deepseek_api_key` 的配置文件仍可读。

OpenCode Go（订阅制，模型 id 与 DeepSeek 相同）：

```bash
export DEEPSEEK_API_KEY="<opencode-go key>"
export DEEPSEEK_BASE_URL="https://opencode.ai/zen/go/v1"
export DEEPSEEK_MODEL="deepseek-v4-flash"   # 或 deepseek-v4-pro
```

请求头：`x-opencode-session: hyper-<id>`（每个 step 一个，同一步的多轮共享，OpenCode Go 缺此头返回 400 `MissingSessionID`）、`User-Agent: hyper/<version>`。`model.started` 记录实际 provider（`deepseek` / `opencode-go` / 其他主机名）、base URL 与 protocol；`model.finished` 记录 provider 和最终响应。

### 协议选择

| protocol | 路径 | 认证 | OpenCode Go 模型 |
| --- | --- | --- | --- |
| `chat`（默认） | `{base}/chat/completions` | `Authorization: Bearer` | GLM、Kimi、LongCat、MiMo、DeepSeek、Hy |
| `responses` | `{base}/responses` | `Authorization: Bearer` | Grok、GPT、Muse Spark |
| `messages` | `{base}/messages` | `x-api-key` + `anthropic-version: 2023-06-01` | MiniMax、Qwen3.6 Plus / 3.7 / 3.8 |

`DEEPSEEK_PROTOCOL`（环境变量）> 配置文件 `protocol` > 自动探测。探测仅对 opencode.ai 主机按模型家族生效，其他主机固定 `chat`。三个协议共用重试与状态码分类；消息转换在 `deepseek.rs`，SSE 解析在 `deepseek/stream.rs`。中性消息格式仍是 OpenAI chat 形状。

## 会话（多轮上下文）

`.harness/sessions/<session-id>.jsonl` 存对话（prompt + 回答），`.harness/harness.db` 的 `sessions` 表存标题/轮数/run 数/更新时间。`--session <id>` 让 `plan`/`build`/直接 prompt 续接对话；`hyper sessions` / `session <id>` / `forget <id>` / `resume <id>` 分别是列出、打印、删除、在 TUI 中继续。TUI 一次启动即一段对话，标题栏显示会话 id，`/new` 开新对话、`/session` 打印 id。会话 id 作为文件名使用，限制为 `[A-Za-z0-9_-]{1,64}`。

工具调用细节保留在 run 的 `events.jsonl`，会话只存 prompt 与回答——回放给模型的是后者，完整轨迹仍可查。

## 危险命令策略

`src/policy.rs` 按 shell 词法（引号、`;`/`&&`/`|`/`&`/换行、重定向、命令替换）分词后判定，取代原先 6 条子串匹配。拦截文件系统破坏（`rm`/`shred`/`truncate`/`chmod`/`chown` 等作用于 `/`、`$HOME`、工作区根、顶层或系统目录）、重定向写入系统路径、机器级程序（`sudo`/`doas`/`dd`/`mkfs*`/`fdisk`/`shutdown`/`systemctl`…）、`curl|sh` 管道，以及 `sh -c '…'` / `sudo` / `env` / `timeout` / `xargs` 内的嵌套命令。解析器本身是轻量禁止而非沙箱；Linux 默认执行模式另有 Landlock 边界和进程级资源限制。

## Replay、artifacts 与保留

- `hyper replay <run-id>` 按 `task.json` + `events.jsonl` 重建该 run 发给模型的完整 messages 并输出 JSON——system、会话前缀、本次 `input`、每轮 assistant 消息与紧随其后的 observation，即**最后一次请求实际携带的 messages**。会话前缀取 transcript 中本 run 之前的部分（会话被 `forget` 则为空，run 自身的记录仍完整）；缺字段的旧格式 run 报错拒绝而非猜造。
- 为可重建补上的事件字段：`model.started.input`（实际发送的用户消息，含 workspace context；文件摘录有约 64 KB 的目标预算，但整个输入没有严格上限）、`model.tool_calls.message`（整轮 assistant 消息，含 `content` 与 `tool_calls`；原有 `calls` 摘要保留）、`model.observation`（`turn`/`callId`/`tool`/`observation`）。观测的派生逻辑抽成纯函数 `observation()`。
- `bash` 输出落盘：每路最多 4 MB 写入 `runs/<id>/artifacts/<step>-<index>-bash-stdout.log`（空流不建文件，超量尾部加截断 marker），事件仍只存 256 KB，并以 `stdoutArtifact`/`stderrArtifact` 指向文件——模型的 observation 里也能看到路径。`ha artifacts` 从此有内容。
- 保留策略：`hyper prune --keep <N> [--runs] [--dry-run]`——会话按 `updated_at`、run 按 `started_at` 只留最近 N 个；`--runs` 连事件、artifacts、checkpoint 一起删；持锁的运行中 run 永不作为候选。
- 索引兜底：`Workspace::open` 先 `reconcile_events()` 再 `reconcile_stale_runs()`，用 JSONL 补齐缺失的 run 行（`task.json` + 首个事件）与事件，常态只比对行数不解析日志；`insert_event` 由 `INSERT OR REPLACE` 改 `INSERT OR IGNORE`，避免重复插入换 rowid、让时间戳相同的事件在 `ORDER BY timestamp,rowid` 下重排。

## 验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Rust 集成测试覆盖 task 校验、shell event、plan 只读、shell 失败、路径隔离（含符号链接越界防护）、edit 指令校验、tools 白名单、checkpoint 恢复以及 undo 恢复最新 checkpoint。

本轮新增覆盖：bash 大输出不死锁与输出上限、超时分类与 `retryable`、`write:` 缺内容行被拒绝、崩溃 run 的修复、存活 run 不被误修复、harness 被杀后命令不再存活、`ha run` 退出码、多词 subcommand prompt；provider 配置解析优先级（env > 文件 > 默认）、配置文件往返与旧文件兼容、0600 权限、请求携带 `x-opencode-session` 与 `hyper/<version>` UA、每个会话独立 session id、provider 名称按 base URL 判定、仅用配置文件驱动一次完整 CLI run（XDG_CONFIG_HOME 隔离 + stub 服务端断言真实请求）。

本轮（协议/会话/沙箱）新增覆盖：三个协议各自的请求路径与认证头（messages 必须 `x-api-key` 且**无** `authorization`）、body 翻译字段与响应解析（文本 + tool_calls + usage）、messages 工具参数不可解析时不 panic、`detect_protocol` 家族判定与非 opencode 主机回退、显式协议覆盖探测、非法协议名报错；会话跨轮重放（stub 断言第二问的请求体含第一问的 prompt 与回答，且首问不重复）、transcript 顺序与标题、每次 run 只计一次 run、session id 穿越防护（`../outside`、`a/b`、`.`、`..`、空串全部拒绝且不落盘）、`sessions|session|forget` 的 CLI 行为与幂等报错；危险命令的结构化判定（文件系统破坏、机器级程序、`curl|sh`、wrapper 与 `sh -c` 嵌套、工作区根保护、命令分词）。

本轮（replay/artifacts/prune/兜底重建）新增覆盖：`hyper replay` 的 messages 与 stub 实收请求体**逐字节相等**（system、user、assistant、tool 四条消息的 role 与 `tool_call_id` 全对）、事件新增字段确实落盘、旧格式 run 被拒绝且缺 run 行也能从日志重建；`bash` 大输出的 artifact 保留尾部而事件仍是 256 KB、`stdoutArtifact` 路径正确、空流不建文件、`ha artifacts` 列表；索引整表丢失后从 JSONL 重建（run 行、task 名、`finished` 状态、事件条数与首尾顺序）；`prune --dry-run` 只报告不删、会话与 run 各按 keep 保留最近的、持锁的运行中 run 不被 prune。

截至 2026-09-29：48 单元 + 43 集成（run.rs）+ 5 集成（task.rs）通过；`cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 均干净。此数字是当时快照，后续以测试命令输出为准。

2026-09-29：Linux `bash` 新增 `RLIMIT_AS`、`RLIMIT_FSIZE`、`RLIMIT_CPU`，默认分别是 8 GiB 虚拟地址空间、单文件 1 GiB、wall timeout 向上取整加 2 秒 CPU 时间。任务步骤可在 `limits` 中覆盖三个值；`tool.started`/`tool.finished` 记录预算，CPU 或文件超限写 `ResourceLimitError`。测试覆盖内存上限继承、1 MiB 文件上限、1 秒 CPU 上限以及非法零值。这些上限按进程生效，不是 cgroup 总配额。

2026-09-29：增加 `src/sandbox.rs` 的 Linux Landlock 边界。默认 `workspace-write` 允许 shell 在工作区内写入，限制工作区外写入与 TCP bind/connect；`read-only` 禁止工具写入和 shell 写入；`unrestricted` 需显式选择，跳过 shell 隔离和危险命令解析。CLI 的 `--sandbox` 与 `HYPER_SANDBOX` 也作用于 TUI；模式记录在 `run.started`，TUI 标题展示当前模式。策略拒绝写 `tool.denied`（工具、目标、原因），交互确认写 `tool.approval`。集成测试覆盖符号链接、子 shell、工作区内写入、工作区外写入、TCP、显式模式和审计事件。剩余限制见 `todo.md`：Landlock 不能隔离全部网络与 metadata 操作，非 Linux 尚无原生 shell 隔离。

此前非流式版本的真实端点与真实 TUI 手工验证（OpenCode Go）：

- 三协议各一轮对话与带工具续跑：`deepseek-v4-flash`（chat）、`grok-4.6`（responses）、`minimax-m2.5`（messages）均成功；强制错协议（grok+chat、minimax+responses）以 503 `Endpoint is unavailable` 失败，证明探测必要。
- `model.started` 事件分别记录 `protocol=chat|messages|responses`。
- 会话：CLI 跨轮（第二问依赖第一问的数字，答 42）、TUI 跨轮（答 8）、`/new` 后模型答 `NO CONTEXT`（证明上下文确实断开且旧会话保留为独立文件）、`resume <id>` 答出旧数字并追加同一 transcript。
- 沙箱：`rm -rf /`、`sudo id`、`curl|sh`、`dd`、`sh -c "rm -rf /usr"` 全部 `PolicyError`；`cargo --version`、`rm -rf target`、`echo hi > out.txt` 正常执行。

GitHub Actions 在 `main` 分支和 Pull Request 上自动运行 fmt/clippy/test/release 构建（`.github/workflows/ci.yml`）。

2026-09-29：agent loop 的三种协议改用 SSE。Chat 累积 `tool_calls` 参数分片，Messages 累积 `tool_use` 的 JSON 分片，Responses 在 `response.completed` 取完整结果；收到文本即写 `model.delta` 并送到 TUI 实时显示。断流缺完成标记报错，未完成的工具参数不会执行；返回普通 JSON 的服务端仍可用。新增本地分段 SSE 测试，确认首块在连接结束前抵达回调，以及三协议工具调用和用量重建。尚未对改动后的流式路径进行真实服务端点或真实 TUI 手工验证。
