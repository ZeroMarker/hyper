# Harness Rust 迁移进度

## 当前状态

项目已经全面迁移到 Rust 1.94，核心运行时不依赖 Node.js 或 TypeScript；npm 仅作为预编译二进制的分发渠道。

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
