# Todo

## 已完成

- [x] **事件级 replay（按 task.json + 事件重建 messages）**：`hyper replay <run-id>` 把一次 run 发给模型的完整 messages 重建出来并输出 JSON——system prompt、会话前缀、本次 `input`、每轮 assistant 消息与紧随其后的 observation，即**最后一次请求实际携带的 messages**；与 stub 服务端收到的请求体逐字节相等（回归测试断言）。补齐两块原本缺失的数据：`model.tool_calls` 现在记录整轮 assistant 消息（新增 `message` 字段，含 `content` 与 `tool_calls`，原有 `calls` 摘要保留）；observation 不再只在运行时派生，改为落库为新事件 `model.observation`（`turn`/`callId`/`tool`/`observation`），派生逻辑抽成纯函数 `observation()`。另在 `model.started` 增加 `input` 字段记录该 step 真正发送的用户消息（含 workspace context）；文件摘录使用约 64 KB 的目标预算，但文件清单和用户提示会另占空间，因此整个 `input` 没有严格的 64 KB 上限。新运行的会话前缀与 system prompt 固定在 `model.started`，裁剪后及 `forget` 后仍可回放；旧运行前缀取 transcript 中本 run 之前的部分，会话被 `forget` 时旧前缀为空；更早的 run（缺 input、assistant message 或 observation）直接报错拒绝，不猜造。步骤只在真的走到模型时才出现（`bash:` 前缀步骤没有 messages）。
- [x] **artifacts 落盘**：`bash` 每路输出最多保留 4 MB（`MAX_ARTIFACT_OUTPUT`），写入 `runs/<id>/artifacts/<step>-<index>-bash-stdout.log`——空流不建文件，超过保留量时尾部追加 `... [truncated after N bytes] ...`。事件仍只存 256 KB（`truncated`/`stdoutBytes` 语义不变），`tool.finished` 新增 `stdoutArtifact`/`stderrArtifact` 字段指向文件，observation 里模型也能看到路径，被截断时可以自己 `read` 回来。实测 `seq 1 300000`：事件 256 KB、artifact 1.99 MB 且末行 `300000` 完整保留；`ha artifacts` 从此有内容。`read` 不复制（原文件就在工作区），observation 的来源 payload 本就在日志里。
- [x] **保留策略 `ha prune`**：`hyper prune --keep <N>` 按 `updated_at` 只留最近 N 个会话（transcript 文件与注册行一起删），`--runs --keep <N>` 按 `started_at` 留最近 N 个 run（events / artifacts / checkpoints 随目录一起删），`--dry-run` 只报告不删；**持锁的运行中 run 永不作为候选**。会话不必再靠 `ha forget` 逐个删。
- [x] **事件/DB 兜底重建**：`Workspace::open` 先 `reconcile_events()` 再 `reconcile_stale_runs()`。按 `events.jsonl` 行数与索引事件数比对，相等且 run 行存在就跳过——常态启动只扫文件不解析日志；不等则读日志，必要时用 `task.json` + 首个事件重建缺失的 run 行（task id/name/started_at），再逐条补索引；解析不了的半行跳过（写入方还没写完）。`INSERT OR REPLACE` 改为 `INSERT OR IGNORE`：重复插入不再换 rowid，避免时间戳相同的事件在 `ORDER BY timestamp,rowid` 下被重排。
- [x] **协议适配**：`Protocol`（`chat` / `responses` / `messages`）三个线协议，消息翻译在 `deepseek.rs`，流式解析在 `deepseek/stream.rs`。`/v1/messages` 用 `x-api-key` + `anthropic-version`（Bearer 会报 `Missing API key`），`/v1/responses` 用 Bearer；三者共用重试与状态码分类。opencode.ai 主机按模型家族自动探测（minimax/qwen3.6-plus/qwen3.7/qwen3.8 → messages，grok/gpt/muse-spark → responses，其余 chat），其他主机一律 chat；`DEEPSEEK_PROTOCOL`（env）或配置文件 `protocol` 显式值优先，非法值报错而非静默回退。`model.started` 事件新增 `protocol` 字段。此前非流式实测：三个协议各自单轮与带工具续跑均通过（grok-4.6 / minimax-m2.5 / deepseek-v4-flash）；故意强制错协议会以 503 `Endpoint is unavailable` 失败，证明探测是必要的而非装饰。
- [x] **会话（多轮上下文）**：`sessions/<id>.jsonl` 存真实对话（prompt + 回答），SQLite `sessions` 表登记标题/轮数/run 数/更新时间；支持 `hyper sessions|session|forget`、`hyper resume <id>`，以及 `plan`、`build` 和直接 prompt 的 `--session <id>`。TUI 一次启动即一段对话：首条消息开新会话，后续消息续接，标题栏显示当前会话 id，`/new` 真正开新对话（旧对话保留可查），`/session` 打印 id。session id 会作为文件名，因此限制为字母/数字/`-`/`_` 并拒绝 `.`/`..`/路径分隔符。实测：CLI 与 TUI 的跨轮上下文（第二问依赖第一问的数字）、`/new` 后模型确实答 `NO CONTEXT`、`resume` 后答出旧数字并追加到同一 transcript。
- [x] **轻量沙箱（危险命令禁止）**：`src/policy.rs` 以 shell 词法（引号/分隔符/命令替换）解析后判定，取代原先的 6 条子串黑名单。拦截：`rm`/`shred`/`truncate`/`chmod`/`chown`/`mv` 等作用于 `/`、`$HOME`、工作区根、顶层目录或系统目录（`/etc`、`/usr`…）；重定向写入 `/dev/*`、`/etc/*`；`sudo`/`doas`/`dd`/`mkfs*`/`fdisk`/`shutdown`/`systemctl` 等机器级程序；`curl|sh` 管道；以及 `sh -c '…'`、`sudo`、`env`、`timeout`、`xargs` 里嵌套的命令（含 `-ec` 这类聚合 flag，并跳过 wrapper 自己的操作数如 `timeout 5`）。`rm -rf target`、`chmod +x script.sh`、`curl -o f.tar.gz`、`echo hi > out.txt` 仍放行。失败归类为 `PolicyError`。
- [x] agent loop 的观测结果按「头 + 尾」截断（4 KB），确保构建/测试日志末尾的错误不会丢失。
- [x] 跨平台运行时：Windows 使用 `cmd`；`rg` 缺失时回退到内置 workspace 枚举与固定字符串搜索。
- [x] SQLite 并发：启用 WAL 并显式设置 busy_timeout。
- [x] provider 配置可用 `hyper config` 持久化：配置项从「只有 API Key」扩展为 `deepseek_api_key` / `base_url` / `model`（`~/.config/hyper/config.json`，0600，旧文件仍可读）；`DEEPSEEK_*` 环境变量 > 配置文件 > 内置默认值的优先级；交互提示中 base URL / model 回车即取默认值。
- [x] 接入 OpenCode Go（`https://opencode.ai/zen/go/v1`）：请求带 `x-opencode-session`（每个 step 一个会话 id，同一步的多轮共享），UA 为 `hyper/<version>`——缺这两个头时 OpenCode Go 直接返回 400 `MissingSessionID`，此前完全无法使用。`model.started` 记录实际 provider（`deepseek` / `opencode-go` / 其他主机名）与 base URL，`model.finished` 记录 provider，不再一律写 `deepseek`。
- [x] 短命令由 `hy` 改名为 `ha`（`hyper` 为规范名，`ha` 只是同一程序的另一个入口名）：`ha` 二进制取代 `hy`，npm `bin`、Release 归档、smoke 校验与文档同步；程序身份仍是 `hyper`（`--version` 报 `hyper`，提示文案写 `hyper config`），usage 行按 clap 默认显示实际调用名。
- [x] 默认接入 DeepSeek provider 与环境变量模型配置（`DEEPSEEK_API_KEY` / `DEEPSEEK_MODEL` / `DEEPSEEK_BASE_URL`）。
- [x] 实现 tool-calling agent loop：模型自主调用 `read`/`search`/`bash`/`write`/`edit`，观测回传，上限 12 轮；plan 模式只暴露只读工具。
- [x] 增加 Windows/macOS/Linux 发布流水线（tag `v*` 触发，4 平台构建并发布 GitHub Release）。
- [x] 路径隔离（含符号链接逃逸防护）、危险 shell 命令拦截、`tools` 白名单。
- [x] 修复 `undo` 按随机文件名取快照的问题（改为按创建时间取最新）。
- [x] TUI approval prompt：`bash`/`write`/`edit` 执行前弹窗确认（`y` 允许 / `n`/`Esc` 拒绝），agent loop 内同样生效。
- [x] shell 进程组终止：超时/取消时杀死整个进程组（Unix `process_group`+`SIGKILL`，Windows `CREATE_NEW_PROCESS_GROUP`+`taskkill /T /F`）。
- [x] `ha diff <run>` 打印文件 diff；`ha artifacts <run>` 列出产物；`ha checkpoints <run>` 列快照；`ha restore <run> <checkpoint-id>` 恢复到指定快照。
- [x] 修复 `bash` 管道死锁：输出超过管道容量（约 64 KiB）的命令会卡到超时；现在边运行边抽干管道，每路最多保留 4 MiB，其中 256 KiB 进入事件。
- [x] 超时与普通失败区分：记录 `timedOut` / `TimeoutError`，`retryable` 不再恒为 false。
- [x] `ha run` / `ha plan` / `ha build` 在 run 未 finished 时返回退出码 1。
- [x] `write:` 缺内容行不再静默清空文件。
- [x] 崩溃恢复：run 锁 + 启动时把无人持锁的 `running` run 修复为 `interrupted`；bash 进程组随 harness 退出而终止（Linux `PR_SET_PDEATHSIG` + Drop 守卫）。
- [x] `bash_timeout_kills_entire_process_group` 的不稳定断言改为按子进程 pid 检查。
- [x] `cargo clippy --all-targets -- -D warnings` 恢复干净（engine test module 移到文件末尾）。
- [x] agent loop 复用 reqwest client，避免每轮重新握手。
- [x] `ha plan fix the bug` 等多词 subcommand prompt 可解析。
- [x] npm 分发渠道：`hyper-harness` 主包 + 5 个平台子包 `hyper-agent-*`（`optionalDependencies`，按 `os`/`cpu` 自动择一），发布流水线新增 `npm` job，需要仓库 secret `NPMJS_TOKEN`（必须是 classic Automation token）；build 矩阵新增 `linux-arm64`（原生 arm64 runner）。
- [x] **实时展示运行中的 event stream**：`EventWriter::write` 在事件持久化后推送简短状态到有界 `EventSink`，TUI 每帧 drain 并保留最近 12 条；重复状态合并。聊天 markdown 只在新消息加入时解析并缓存；模型文本分片另进入有上限的显示缓冲。
- [x] streaming 响应（SSE）：agent loop 的 Chat、Responses、Messages 请求现在使用流式响应；文本分片写入 `model.delta` 事件并实时显示在 TUI。Chat 的 `tool_calls` 和 Messages 的 `tool_use` 参数按分片累积，Responses 从完成事件取完整 `function_call`；缺少协议完成标记时失败，不执行残缺的工具调用。服务端返回普通 JSON 时兼容读取。
- [x] CLI 机器可读实时 JSONL：`--jsonl` 支持 run/plan/build/直接 prompt（含会话），每条完整事件在文件与索引持久化后立即写入并 flush stdout，与 events.jsonl 完全一致；不混入普通回答、summary 或配置向导。失败保留退出码 1，stderr 放诊断；慢消费者背压，输出失败停止执行且保留审计。门控 SSE 测试证明 delta 在提供商完成前到达，另覆盖失败、会话、四种入口及 flush 错误。
- [x] 资源限制：Linux `bash` 子进程增加 `RLIMIT_AS`（默认 8 GiB 虚拟地址空间）、`RLIMIT_FSIZE`（默认单文件 1 GiB）、`RLIMIT_CPU`（默认 wall timeout 向上取整后加 2 秒），支持步骤级 `limits.memoryMb` / `fileMb` / `cpuSeconds`；继承更严格的父进程软限制，超 CPU/文件上限记录 `ResourceLimitError` 与配置值。限制按进程生效，不是整棵进程树或整个工作区的总配额；后续若需聚合上限，需另做 cgroup/job object。
- [x] 会话历史的上下文预算：`HYPER_HISTORY_TOKENS`（默认 16000，0 禁用历史）按 UTF-8 字节数 + 每条 8 的保守 token 估算，滑窗保留最近完整用户轮次；不拆分超大轮次，也不回填更旧轮次。原 transcript 保留，`model.started` 固定实际 history、systemPrompt 与预算/保留/丢弃计数；忘记会话后仍能准确 replay，新旧事件兼容。当前 prompt、workspace context、工具定义与本轮工具输出不在该预算内。
- [x] 总请求上下文预算：`HYPER_CONTEXT_TOKENS`（默认 128000）减去 `HYPER_OUTPUT_TOKENS`（默认 8192）作为输入预算；每次按三种协议实际 JSON 请求体的 UTF-8 字节数保守估算，覆盖 system/input/tools、参数和观察结果。首次请求进一步裁掉整轮旧会话；不可省略的输入或工具循环超预算时，在发送前以 `ContextBudgetError` 失败，完整审计保留。三种协议发送输出上限，`model.context_budget` 记录每轮用量和判断，replay 停在最后一次通过预算的请求。预算需按所用模型手动配置，尚不探测真实模型能力，也未接入 provider tokenizer。
- [x] 删除死代码：`deepseek::chat`（一次性、无工具）全仓库无调用点，已删除。
- [x] **P0-1 固定任务集和评测脚手架**：10 个 Rust/Python/JS 离线 fixture、工作区外独立行为判定、全新仓库/会话与固定模型配置，每项真实运行 3 次；逐轮延迟、usage、错误、审批、恢复和 prune 前后磁盘体积落 JSONL/Markdown。正式基线 `369a415` 上 28/30 通过，3/3 错误编辑恢复通过，缺失 usage/成本标未知；113 个 Rust 测试与 9 个评测测试通过，离线评测已接入 CI。见 [脚手架](evals/README.md) 与 [正式报告](evals/baselines/2026-10-03/report.md)。

- [x] **P0-2 CLI/TUI 共享取消**：共享运行令牌覆盖响应头、SSE/JSON 正文、重试、审批和 shell；CLI 信号取消退出 130，TUI Ctrl-C/运行中 Esc 与 `/cancel` 取消，审批 Esc 仅拒绝当前动作。取消记录独立的 run.cancelled，summary/DB/session 结算一次，保留已完成修改、输出与 checkpoint，并停止后续工具；外部信号直接传递到后台，退出等待 worker。Unix JSONL 背压可取消；125 个 Rust 测试与 10 个评测测试通过，含真实 Linux PTY 取消后续聊；固定任务集 30/30 通过，Responses/Messages 真实端点冒烟各 1/1。见 [回归报告](evals/baselines/2026-10-03-cancellation/report.md)。

- [x] **P0-3a 共用工具权限与直接审计路径保护**：CLI/TUI 共用 allow/ask/deny，默认 read/search allow、bash/write/edit ask；CLI 无处理器时 ask 明确拒绝，显式 `--approval` / `--permissions` / HYPER_APPROVAL 固定来源与优先级，审批仅授权一次。直接文件工具拒绝审计路径、符号链接和 Unix 硬链接，search/context 同步过滤；旧库 API 保持兼容。135 个 Rust 测试和 10 个离线评测测试通过，真实任务回归 28/30、恢复 3/3，保留两次既有约束/策略失败。见 [回归报告](evals/baselines/2026-10-03-permissions/report.md)。完整边界及 shell 风险复现见 [审计边界说明](docs/audit-boundary.md)。

- [x] **P0-3b1 外部审计内容存储与显式迁移**：权威 state 移出 checkout，由 canonical root 的 SHA-256 在宿主状态目录定位，`ha state` 返回位置；不信任仓库定位文件，旧 `.harness`/备份用 `ha migrate-state --from` 明确导入，原始源保留且不覆盖已初始化目标。新 inode、SQLite WAL 在线备份、记录/schema 校验、session registry 修复、checkpoint 路径重绑与原子提交；活动源/坏数据/symlink 拒绝，崩溃尾片段保留 artifact 后恢复。Linux 内容写/删除/rename/hardlink 与父进程 FD 绕过实测阻断，151 个 Rust 测试通过。tmp 独立置于 workspace，artifact 仅 harness 写入。元数据不宣称已隔离。见 [实际边界](docs/audit-boundary.md)。

- [x] **P0-3b2a Linux 直接工具描述符边界**：read/write/edit、search 与上下文经工作区目录描述符和 openat2 读取；校验后 symlink/审计硬链接拒绝，快照和修改绑定实际打开的同一 inode，FIFO 非阻塞拒绝。157 个 Rust 测试通过，含 6 个确定性路径替换/兼容性测试。原生文本搜索不再由 rg 重开路径；非 Linux、admin restore、宿主目录移动和并发内容写者仍另列边界。见 [验证范围](docs/audit-boundary.md)。

## 下一步（2026-10-03 竞品复核）

对标范围、官方来源、现状与详细验收见 [plan.md](plan.md)。最新代码/评测基线为 `0fc6dad`：streaming、CLI JSONL、历史滑窗、总请求预算、输出预留、固定任务脚手架、共享取消、共用工具权限、直接审计路径保护及外部审计内容存储/显式迁移、Linux 直接工具描述符边界已完成。[本轮真实回归](evals/baselines/2026-10-03-descriptor/report.md) 30/30 通过，错误修改恢复及长会话各 3/3；外部 state 的磁盘统计排除 workspace tmp，不与旧口径直接比较。本清单只列未完成交付；优先级是产品判断，已有 Hyper/指定模型基线，尚未与竞品对跑。

### P0：质量与运行控制

- [ ] **P0-3b2b 元数据、剩余路径竞争与范围授权（下一项）**：外部内容/目录边界已完成，但 Linux 实测 shell chmod 可改变外部审计文件权限，存在可用性风险；需要独立 OS 元数据边界，不能靠命令过滤。补路径/命令批准范围、Windows 硬链接、非 Linux 描述符保护、admin restore 和宿主目录移动竞争，验证审计区 metadata/读取、已有挂载别名、仓库 prompt injection 与旧 source 写者协调；不能扩大 OS 边界。当前环境用户 mount namespace 不可用。
- [ ] **平台隔离后续**：Linux UDP/Unix socket、metadata、外部读取边界及 macOS/Windows 原生隔离，按平台报告支持范围；目前 Landlock 只覆盖部分写入和 TCP，非 Linux 或 ABI 不足仍拒绝受限 shell。与 P0-3 分阶段交付。
- [ ] **评测环境与失败样本扩展**：固定或记录 PATH/Cargo 配置/编译 wrapper，加入良性 `2>/dev/null` 与重复策略拒绝样本；跟踪长会话约束位置遗漏。正式基线已有一次代码正确但耗尽 12 轮的失败，归入 P0-3/P1-5 策略与重复检测验收，不放宽外部路径边界。

- [ ] **取消的原生 I/O 后续**：Windows stdout 背压、终端渲染背压和 OS 阻塞文件 I/O 的有界取消；自定义同步 event writer 需自行可中断。Unix shell 仍按进程组清理，脱离该组的 daemon/继承管道场景需另测并纳入平台隔离交付。

### P1：提高任务完成率

- [ ] **P1-1 可审计压缩**：手动后自动；摘要保留目标、约束、改动和剩余工作，记录来源边界、模型、usage 和实际消息投影；保留原始会话/事件，保证 call/result 成对和 replay 一致，失败或取消时保留可用历史并限制重试。
- [ ] **P1-2 相关上下文与范围读取**：先显式文件选择、read 行/字节范围，再按 prompt/路径/语言选择摘录；覆盖 Python/JS，记录范围和截断，遵守预算与敏感路径排除；空仓库的 rg --files 退出码 1 应视为空集合，而不是启动失败。repo map/语法索引以固定任务集比较收益。
- [ ] **P1-3 编辑和验证闭环**：唯一匹配/显式 occurrence、陈旧文件 hash 检查、原子写入与可定位错误；显式 lint/test 命令走执行策略与审批、有限重试，记录首轮编辑成功率。重复块、并发变更、快照与 Unicode 必须验证，再选 Hashline/patch。
- [ ] **P1-4 项目说明**：根 AGENTS.md 起步，随后目录作用域和 override；固定加载顺序、来源、预算及实际 prompt，说明不能改变宿主授权/工具权限。不在读取时执行脚本；按需 skills 后续接入。
- [ ] **P1-5 模型能力与完成语义**：可配置 provider/model 上限、协议路径与能力，支持可用的 tokenizer/服务端计数及明确回退；检测输出长度截断与残缺工具调用，不把截断当成功。为重复失败调用设置可审计阈值，避免误伤分页/测试重跑。
- [ ] **P1-6 命名 provider profile**：兼容旧配置和 DEEPSEEK 环境变量，list/use/test、认证来源、三协议 endpoint 覆盖；运行固定有效配置，stub 区分认证/路径/协议/模型/限流/超时，不记录密钥。

### P2：按实测需求扩展

- [ ] **P2-1 自动化契约**：事件 schema/版本和兼容规则、最终结果 JSON Schema；共享取消稳定后再做双向 RPC/ACP、会话 fork。JSONL 输出流不等于 RPC。
- [ ] **P2-2 按需 skills/MCP**：从一个外部工具场景起步，定义来源、信任、权限继承、超时和断连恢复，再评估 hooks/插件系统。
- [ ] **P2-3 并行与代码智能实验**：先只读 read/search 并发，再隔离会话/worktree 子 Agent；write/edit/bash 保留审批、快照与顺序语义。LSP/调试器从单语言真实任务起步，用成功率、耗时、usage 与冲突证明收益。全量插件内核、Web UI 和多人协作暂留候选池。

### 评测与存储口径

之前的约 2 KB trivial run / 10 KB 工具 run 是加入 SSE、历史快照和预算事件前的样本，不能作为当前容量估算。新版本每个 delta 都持久化，历史/system prompt 也固定到事件；输入、SQLite 页、artifact 和 checkpoint 另占空间。旧报告记录 `.harness` 逻辑/分配体积，新存储报告按权威外部审计目录统计，工作区 `.hyper-tmp` 不计入；不直接混用两种口径，均记录 `prune --runs --keep 1` 效果；外部 trace 副本另外保留，不计入该体积。
