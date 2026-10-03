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

## 下一步（按优先级）

> 本清单于 2026-09-28 复核；本轮完成事件级 replay、artifacts 落盘与保留策略、事件/DB 兜底重建后，剩余项已重写。下列体积是加入 SSE 前的样本：一次 trivial run ≈ 2 KB JSONL；一次带工具调用的模型 run ≈ 10 KB JSONL。现在每个文本分片还会产生 `model.delta` 事件，实际体积依赖提供商分片方式；`model.started.input` 也没有严格的 64 KB 上限，另有 SQLite 索引与页开销。

### 可观测性
- [x] **实时展示运行中的 event stream**：`EventWriter::write` 在事件持久化后推送简短状态到有界 `EventSink`，TUI 每帧 drain 并保留最近 12 条；重复状态合并。聊天 markdown 只在新消息加入时解析并缓存；模型文本分片另进入有上限的显示缓冲。
- [x] streaming 响应（SSE）：agent loop 的 Chat、Responses、Messages 请求现在使用流式响应；文本分片写入 `model.delta` 事件并实时显示在 TUI。Chat 的 `tool_calls` 和 Messages 的 `tool_use` 参数按分片累积，Responses 从完成事件取完整 `function_call`；缺少协议完成标记时失败，不执行残缺的工具调用。服务端返回普通 JSON 时兼容读取。
- [x] CLI 机器可读实时 JSONL：`--jsonl` 支持 run/plan/build/直接 prompt（含会话），每条完整事件在文件与索引持久化后立即写入并 flush stdout，与 events.jsonl 完全一致；不混入普通回答、summary 或配置向导。失败保留退出码 1，stderr 放诊断；慢消费者背压，输出失败停止执行且保留审计。门控 SSE 测试证明 delta 在提供商完成前到达，另覆盖失败、会话、四种入口及 flush 错误。

### 工程健壮性
- [ ] 并行 tool calls：现为 `for call in &reply.tool_calls` 串行（engine.rs:221）。收益中等，但需要先定哪些工具可并发（write/edit/bash 涉及审批、checkpoint 与顺序语义），不建议先做。
- [x] 资源限制：Linux `bash` 子进程增加 `RLIMIT_AS`（默认 8 GiB 虚拟地址空间）、`RLIMIT_FSIZE`（默认单文件 1 GiB）、`RLIMIT_CPU`（默认 wall timeout 向上取整后加 2 秒），支持步骤级 `limits.memoryMb` / `fileMb` / `cpuSeconds`；继承更严格的父进程软限制，超 CPU/文件上限记录 `ResourceLimitError` 与配置值。限制按进程生效，不是整棵进程树或整个工作区的总配额；后续若需聚合上限，需另做 cgroup/job object。
- [ ] OS 级沙箱（分阶段）：Linux 默认模式已用 Landlock 限制 shell 及其子进程的工作区外写入与 TCP bind/connect；`read-only` / `workspace-write` / `unrestricted` 由 CLI、TUI 共用，非 Linux 或缺 Landlock ABI 4 时默认拒绝 shell。剩余：网络 UDP/Unix socket、Landlock 未覆盖的 metadata 操作、macOS/Windows 原生隔离，以及对 `.harness` 审计文件的保护。workspace context 仍会把仓库内容发给模型，prompt injection 是活路径。

### 协议与模型
- [ ] 协议能力的**能力差异**处理：`detect_protocol` 按模型家族在白名单内探测（opencode.ai 主机）；网关新增模型或改名时需要同步，输出上限现已支持 `HYPER_OUTPUT_TOKENS` 配置，但仍需按模型能力手工设置。若某网关把三种协议挂在不同 base path 下，探测表需改为可配置。
- [x] 会话历史的上下文预算：`HYPER_HISTORY_TOKENS`（默认 16000，0 禁用历史）按 UTF-8 字节数 + 每条 8 的保守 token 估算，滑窗保留最近完整用户轮次；不拆分超大轮次，也不回填更旧轮次。原 transcript 保留，`model.started` 固定实际 history、systemPrompt 与预算/保留/丢弃计数；忘记会话后仍能准确 replay，新旧事件兼容。当前 prompt、workspace context、工具定义与本轮工具输出不在该预算内。
- [x] 总请求上下文预算：`HYPER_CONTEXT_TOKENS`（默认 128000）减去 `HYPER_OUTPUT_TOKENS`（默认 8192）作为输入预算；每次按三种协议实际 JSON 请求体的 UTF-8 字节数保守估算，覆盖 system/input/tools、参数和观察结果。首次请求进一步裁掉整轮旧会话；不可省略的输入或工具循环超预算时，在发送前以 `ContextBudgetError` 失败，完整审计保留。三种协议发送输出上限，`model.context_budget` 记录每轮用量和判断，replay 停在最后一次通过预算的请求。预算需按所用模型手动配置，尚不探测真实模型能力，也未接入 provider tokenizer。
- [ ] 模型能力与精确 token 计数：按 provider/model 获取上下文和输出上限，支持 tokenizer 或服务端计数；当前默认预算与字节估算不等于实际模型限制。

### 清理
- [x] 删除死代码：`deepseek::chat`（一次性、无工具）全仓库无调用点，已删除。
