# Todo

## 已完成

- [x] **协议适配**：`Protocol`（`chat` / `responses` / `messages`）三个线协议，`chat_messages` 对外签名不变，协议差异只在其内部翻译。`/v1/messages` 用 `x-api-key` + `anthropic-version`（Bearer 会报 `Missing API key`），`/v1/responses` 用 Bearer；三者共用重试与状态码分类。opencode.ai 主机按模型家族自动探测（minimax/qwen3.6-plus/qwen3.7/qwen3.8 → messages，grok/gpt/muse-spark → responses，其余 chat），其他主机一律 chat；`DEEPSEEK_PROTOCOL`（env）或配置文件 `protocol` 显式值优先，非法值报错而非静默回退。`model.started` 事件新增 `protocol` 字段。实测：三个协议各自单轮与带工具续跑均通过（grok-4.6 / minimax-m2.5 / deepseek-v4-flash）；故意强制错协议会以 503 `Endpoint is unavailable` 失败，证明探测是必要的而非装饰。
- [x] **会话（多轮上下文）**：`sessions/<id>.jsonl` 存真实对话（prompt + 回答），SQLite `sessions` 表登记标题/轮数/run 数/更新时间；`hyper sessions|session|forget`、`hyper tui --resume` 之外的 `hyper resume <id>`、以及 `plan/build/直接 prompt` 的 `--session <id>`。TUI 一次启动即一段对话：首条消息开新会话，后续消息续接，标题栏显示当前会话 id，`/new` 真正开新对话（旧对话保留可查），`/session` 打印 id。session id 会作为文件名，因此限制为字母/数字/`-`/`_` 并拒绝 `.`/`..`/路径分隔符。实测：CLI 与 TUI 的跨轮上下文（第二问依赖第一问的数字）、`/new` 后模型确实答 `NO CONTEXT`、`resume` 后答出旧数字并追加到同一 transcript。
- [x] **轻量沙箱（危险命令禁止）**：`src/policy.rs` 以 shell 词法（引号/分隔符/命令替换）解析后判定，取代原先的 6 条子串黑名单。拦截：`rm`/`shred`/`truncate`/`chmod`/`chown`/`mv` 等作用于 `/`、`$HOME`、工作区根、顶层目录或系统目录（`/etc`、`/usr`…）；重定向写入 `/dev/*`、`/etc/*`；`sudo`/`doas`/`dd`/`mkfs*`/`fdisk`/`shutdown`/`systemctl` 等机器级程序；`curl|sh` 管道；以及 `sh -c '…'`、`sudo`、`env`、`timeout`、`xargs` 里嵌套的命令（含 `-ec` 这类聚合 flag，并跳过 wrapper 自己的操作数如 `timeout 5`）。`rm -rf target`、`chmod +x script.sh`、`curl -o f.tar.gz`、`echo hi > out.txt` 仍放行。失败归类为 `PolicyError`。
- [x] agent loop 的观测结果按「头 + 尾」截断（4 KB），确保构建/测试日志末尾的错误不会丢失。
- [x] 跨平台运行时：Windows 使用 `cmd`；`rg` 缺失时回退到内置 workspace 枚举与固定字符串搜索。
- [x] SQLite 并发：启用 WAL 并显式设置 busy_timeout。
- [x] provider 配置可用 `hyper config` 持久化：配置项从「只有 API Key」扩展为 `deepseek_api_key` / `base_url` / `model`（`~/.config/hyper/config.json`，0600，旧文件仍可读）；`DEEPSEEK_*` 环境变量 > 配置文件 > 内置默认值的优先级；交互提示中 base URL / model 回车即取默认值。
- [x] 接入 OpenCode Go（`https://opencode.ai/zen/go/v1`）：请求带 `x-opencode-session`（每个 step 一个会话 id，同一步的多轮共享），UA 为 `hyper/<version>`——缺这两个头时 OpenCode Go 直接返回 400 `MissingSessionID`，此前完全无法使用。`model.*` 事件改为记录实际 provider（`deepseek` / `opencode-go` / 其他主机名）与 base URL，不再一律写 `deepseek`。
- [x] 短命令由 `hy` 改名为 `ha`（`hyper` 为规范名，`ha` 只是同一程序的另一个入口名）：`ha` 二进制取代 `hy`，npm `bin`、Release 归档、smoke 校验与文档同步；程序身份仍是 `hyper`（`--version` 报 `hyper`，提示文案写 `hyper config`），usage 行按 clap 默认显示实际调用名。
- [x] 默认接入 DeepSeek provider 与环境变量模型配置（`DEEPSEEK_API_KEY` / `DEEPSEEK_MODEL` / `DEEPSEEK_BASE_URL`）。
- [x] 实现 tool-calling agent loop：模型自主调用 `read`/`search`/`bash`/`write`/`edit`，观测回传，上限 12 轮；plan 模式只暴露只读工具。
- [x] 增加 Windows/macOS/Linux 发布流水线（tag `v*` 触发，4 平台构建并发布 GitHub Release）。
- [x] 路径隔离（含符号链接逃逸防护）、危险 shell 命令拦截、`tools` 白名单。
- [x] 修复 `undo` 按随机文件名取快照的问题（改为按创建时间取最新）。
- [x] TUI approval prompt：`bash`/`write`/`edit` 执行前弹窗确认（`y` 允许 / `n`/`Esc` 拒绝），agent loop 内同样生效。
- [x] shell 进程组终止：超时/取消时杀死整个进程组（Unix `process_group`+`SIGKILL`，Windows `CREATE_NEW_PROCESS_GROUP`+`taskkill /T /F`）。
- [x] `ha diff <run>` 打印文件 diff；`ha artifacts <run>` 列出产物；`ha checkpoints <run>` 列快照；`ha restore <run> <checkpoint-id>` 恢复到指定快照。
- [x] 修复 `bash` 管道死锁：输出超过管道容量（约 64KB）的命令会卡到超时；现在边运行边抽干管道，并对每路输出设 256KB 上限。
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

> 本清单于 2026-09-21 复核；本轮完成协议适配、会话与轻量沙箱后，剩余项已重写。体积参考：一次带工具调用的模型 run ≈ 34 KB（JSONL 4.3 KB + DB payload 1.8 KB + SQLite 页开销），每次 trivial run ≈ 4.7 KB。

### 可观测性
- [ ] **实时展示运行中的 event stream（建议先做）**：`EventWriter::write`（engine.rs:32）是所有事件的唯一出口，`ApprovalGate`（approval.rs）已经是「工作线程推送 + TUI 主循环 drain」的现成范式，照它加一个 `EventSink` 即可。前置工作：TUI 目前每帧重新解析全部消息的 markdown（ui.rs:50、219），必须改成「渲染结果缓存 + 一条临时 tail 行」，否则事件一变多就会随 output 增长而变慢；事件队列还需要合并与限长。
- [ ] replay/resume（事件级重放）：会话已解决「对话上下文」，但**从事件重建一次 run 的完整 messages** 仍缺两块——`model.tool_calls`（engine.rs:191）没有记录该轮 assistant 的 `content`；observation 是运行时由 payload 派生（engine.rs:331 头尾截断）而非存储。推进顺序：先补事件字段并抽出 observation 派生函数，再按 `task.json` + 事件重建。`Workspace::last_started_step`（workspace.rs:234）与 `interrupted` 状态已就位。
- [ ] streaming 响应（SSE）：三个协议目前都是 `stream: false` + 整体读取响应。blocking `Response` 实现了 `Read`，可自行解析 SSE，但流式 tool_calls/`function_call`/`tool_use` 分片需要按协议分别累积。价值依赖「实时展示」——否则流没有出口。

### 工程健壮性
- [ ] artifacts 落盘 + 保留策略：`artifacts/` 仍无写入方，而 `read` 截断 64 KB、`bash` 每路截断 256 KB、observation 截断 4 KB——把未截断输出落进 `artifacts/`，既让 `ha artifacts` 有意义，也为 replay 与排障留下证据。保留策略（`ha prune --keep N` 或配置）仍不存在：现在 `sessions/` 会随对话增长，`ha forget` 只能手工逐个删。
- [ ] 事件/DB 兜底重建：启动时 reconcile 只修 `running` 状态。`INSERT OR REPLACE` 按 event_id 幂等（workspace.rs:267），所以「JSONL 为事实源、按 run 重放回 DB」可行，规模小且独立。
- [ ] 并行 tool calls：现为 `for call in &reply.tool_calls` 串行（engine.rs:206）。收益中等，但需要先定哪些工具可并发（write/edit/bash 涉及审批、checkpoint 与顺序语义），不建议先做。
- [ ] 资源限制：策略层已完成（`src/policy.rs`：结构化危险命令禁止），但**没有任何资源限制**——没有 setrlimit，内存/CPU/文件大小都不设上限，只有命令超时与 256 KB 输出上限。这一步与下一步（OS 级沙箱）可分开做。
- [ ] OS 级沙箱（需先定威胁模型）：当前是 denylist，不是 containment boundary——未识别的命令仍以用户权限执行。可选 Linux landlock / macOS seatbelt / 默认拒绝 shell，三者工作量差一个数量级。注意 workspace context 会把仓库内容发给模型，prompt injection 是活路径。

### 协议与模型
- [ ] 协议能力的**能力差异**处理：`detect_protocol` 按模型家族在白名单内探测（opencode.ai 主机）；网关新增模型或改名时需要同步，且 `max_tokens`/`max_output_tokens` 目前是固定常量而非按模型上限。若某网关把三种协议挂在不同 base path 下，探测表需改为可配置。
- [ ] 会话的上下文预算：跨轮会把全部历史消息原样回传，没有任何裁剪或摘要；长对话会持续增长并可能超模型上下文。需要 token 估算 + 保留策略（滑窗或摘要）。

### 清理
- [x] 删除死代码：`deepseek::chat`（一次性、无工具）全仓库无调用点，已删除。
