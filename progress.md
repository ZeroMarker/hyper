# Harness Rust 迁移进度

## 当前状态

项目已经全面迁移到 Rust 1.94，不再依赖 Node.js、npm 或 TypeScript。

## 已完成

- [x] Clap CLI，生成 `hyper` 主命令和 `hy` 短命令：`init`、`validate`、`run`、`plan`、`build`、`runs`、`show`、`artifacts`、`undo`、`tui`。
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
- [x] 快照/diff 命令：`hy diff`、`hy artifacts`、`hy checkpoints`、`hy restore <run> <checkpoint-id>`。
- [x] 跨平台发布流水线（Windows/macOS/Linux，tag `v*` 触发，自动上传 GitHub Release）。
- [x] 兼容原有 task JSON、workspace 目录和 SQLite schema。
- [x] 删除 TypeScript 源码、npm manifest、Vitest 和 Node 构建产物。

## 缺陷修复（本轮）

代码审查加实测发现的缺陷，已全部修复并补上回归测试：

- [x] **`bash` 管道死锁（严重）**：stdout/stderr 原先在子进程退出后才读取，命令输出超过管道容量（Linux 约 64KB）时双方互锁，只能等到超时被杀。现在边运行边抽干两条管道，每路最多保留 256KB（超出部分丢弃并标记 `truncated`），读写均在 `capture()` 中完成。
- [x] **超时与普通失败不可区分**：超时现在记录 `"timedOut": true`，失败信息为 `command ... timed out after Nms and was killed`，`errorType` 为 `TimeoutError` 且 `retryable` 为 true。
- [x] **失败任务退出码为 0**：`hy run` / `hy plan` / `hy build` / 直接 prompt 在 run 未 finished 时以退出码 1 结束（stdout 仍打印 summary）。
- [x] **`write:` 缺内容行会静默清空文件**：缺少内容行直接报错；`write:a.txt\n` 仍表示写入空文件。同时 `write:` / `edit:` 内容改用未 trim 的原始 instruction，避免尾部换行被吃掉。
- [x] **崩溃后 run 永久停留在 `running`**：新增 `runs/<id>/lock` 咨询锁（进程退出即释放，含 SIGKILL），启动时把无人持锁的 `running` run 修复为 `interrupted`，并补写 `run.interrupted` 事件与 `summary.json`；仍在运行的 run 不受影响。
- [x] **孤儿进程**：bash 进程组增加 Drop 守卫（提前返回/panic 也会清理），Linux 上通过 `PR_SET_PDEATHSIG` 让 shell 随 harness 一同退出。
- [x] **脆弱测试**：`bash_timeout_kills_entire_process_group` 原先用 `pgrep -f "sleep 60"` 判定，会命中宿主上无关进程；现在由命令自报子进程 pid 并检查 `/proc/<pid>`。
- [x] **clippy**：`engine.rs` 的 test module 移到文件末尾，`cargo clippy --all-targets -- -D warnings` 通过。
- [x] **HTTP 连接复用**：`DeepSeekConfig` 持有 `reqwest::blocking::Client`，agent loop 各轮不再重复建连/TLS 握手。
- [x] **CLI 一致性**：`hy plan fix the bug`（多词不引号）现在可以解析。
- [x] 新增 14 个回归测试（大输出不死锁、输出截断、超时分类、write 缺内容行、崩溃修复、存活 run 不被误修复、harness 被杀后命令不再存活、cli 退出码、多词 prompt 等）。

## 发布渠道：npm

- [x] 新增 npm 分发渠道（`npm/`），采用 esbuild/biome 的「主包 + 平台子包」结构：`hyper-agent` 只含一个 Node shim（`npm/bin/hyper.js`，约 3KB 打包体积），各平台二进制放在 `hyper-agent-linux-x64`、`hyper-agent-linux-arm64`、`hyper-agent-darwin-x64`、`hyper-agent-darwin-arm64`、`hyper-agent-windows-x64`，由主包的 `optionalDependencies` 按 `os`/`cpu` 自动择一安装。shim 不硬编码平台矩阵：它遍历已声明的平台包，用包内 `os`/`cpu` 确认匹配后，从包内 `hyper.binary` 字段取二进制路径。
- [x] Windows 平台包命名为 `hyper-agent-windows-x64`（不是 `hyper-agent-win32-x64`）：后者被 npm 反垃圾启发式确定性拒绝（`403 Package name triggered spam detection`，疑似与 `@esbuild/win32-x64` 这类平台包命名撞形），实测重试无效、其余 4 个包同一秒内发布成功。
- [x] shim 只做定位与转发：`stdio: inherit`（TUI 保有真实 TTY）、退出码原样传递、SIGINT/SIGTERM/SIGHUP 转发；缺少平台包时给出可操作的报错而不是堆栈；无生命周期脚本、安装期不下载任何东西；支持 `HYPER_BINARY_PATH` 指向自编译二进制；`hy` 与 `hyper` 链接到同一 shim。
- [x] `npm/platforms.json` 作为平台矩阵唯一来源（npm 包名、`os`/`cpu`、release artifact 后缀、目标三元组），`npm/scripts/publish.mjs` 据此暂存并发布；平台包先发、主包后发。
- [x] release 流水线扩展：build 矩阵新增 `linux-arm64`（使用公开仓库免费的 `ubuntu-24.04-arm` 原生 runner，避免交叉编译 bundled SQLite / ring）；新增 `npm` job，在上传前先暂存并 smoke test，已存在的版本自动跳过（可重复执行），手动触发默认只做演练。
- [x] `npm/scripts/smoke.sh`：打包真实 tarball → 用用户路径安装（`npm pack` + `npm install`）→ 校验 `hyper`/`hy` 可执行、版本正确、失败 run 退出码透传、缺平台包时的报错。
- [x] 本地已完整验证该链路（aarch64 Linux + 真实 release 二进制）：平台包 3.9MB 压缩 / 9.1MB 解压，主包 3.3KB / 4 个文件，`hyper --version` 经 shim 输出 `hyper 0.1.0`。

npm 发布顺序陷阱：npm 对 publish 是异步处理的——命令返回成功、日志里打印 `+ pkg@ver`，此时包可能仍在队列中（`Your package is being processed and may take a few minutes to become available`）。而安装器对「暂时解析不到的 optionalDependency」是**静默跳过**的，于是会出现「主包已可见、平台包还没可见」的窗口，用户装完只有 shim 没有二进制。已在 `publish.mjs` 中修掉：平台包全部发布后轮询 `registry/<pkg>/<version>` 直到可见，才发布主包；超时（10 分钟）则直接失败并提示重跑（已发布的包会被跳过），确保不会出现无二进制的版本。

npm 包名：主包必须叫 `hyper-harness` —— `hyper-agent` 被 npm 相似度检查永久拒绝（`403 Package name too similar to existing package hyperagent`，后者是 2022 年的无关包），`hyper-coding-agent`/`hyper-agent-cli` 这类变体归一化后仍含 `hyperagent`，风险高；平台子包沿用首发时的 `hyper-agent-*` 前缀不动。

发布前置条件：仓库需要配置 `NPMJS_TOKEN` secret，workflow 会以 `NODE_AUTH_TOKEN` 传给 npm。必须是 classic **Automation** token（或勾选 Bypass 2FA 的 granular token）：classic *Publish* token 会在 CI 里以 `EOTP`（需要一次性验证码）失败——首次发版即因此失败过一次。


## 模型配置

```bash
export DEEPSEEK_API_KEY="sk-..."
# 可选：DEEPSEEK_MODEL、DEEPSEEK_BASE_URL
```

## 验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Rust 集成测试覆盖 task 校验、shell event、plan 只读、shell 失败、路径隔离（含符号链接越界防护）、edit 指令校验、tools 白名单、checkpoint 恢复以及 undo 恢复最新 checkpoint。

本轮新增覆盖：bash 大输出不死锁与输出上限、超时分类与 `retryable`、`write:` 缺内容行被拒绝、崩溃 run 的修复、存活 run 不被误修复、harness 被杀后命令不再存活、`hy run` 退出码、多词 subcommand prompt。

当前测试数量：12 单元 + 25 集成（run.rs）+ 3 集成（task.rs），全部通过；`cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 均干净。

GitHub Actions 在 `main` 分支和 Pull Request 上自动运行 fmt/clippy/test/release 构建（`.github/workflows/ci.yml`）。
