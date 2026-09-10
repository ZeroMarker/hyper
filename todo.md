# Todo

## 已完成

- [x] 默认接入 DeepSeek provider 与环境变量模型配置（`DEEPSEEK_API_KEY` / `DEEPSEEK_MODEL` / `DEEPSEEK_BASE_URL`）。
- [x] 实现 tool-calling agent loop：模型自主调用 `read`/`search`/`bash`/`write`/`edit`，观测回传，上限 12 轮；plan 模式只暴露只读工具。
- [x] 增加 Windows/macOS/Linux 发布流水线（tag `v*` 触发，4 平台构建并发布 GitHub Release）。
- [x] 路径隔离（含符号链接逃逸防护）、危险 shell 命令拦截、`tools` 白名单。
- [x] 修复 `undo` 按随机文件名取快照的问题（改为按创建时间取最新）。
- [x] TUI approval prompt：`bash`/`write`/`edit` 执行前弹窗确认（`y` 允许 / `n`/`Esc` 拒绝），agent loop 内同样生效。
- [x] shell 进程组终止：超时/取消时杀死整个进程组（Unix `process_group`+`SIGKILL`，Windows `CREATE_NEW_PROCESS_GROUP`+`taskkill /T /F`）。
- [x] `hy diff <run>` 打印文件 diff；`hy artifacts <run>` 列出产物；`hy checkpoints <run>` 列快照；`hy restore <run> <checkpoint-id>` 恢复到指定快照。
- [x] 修复 `bash` 管道死锁：输出超过管道容量（约 64KB）的命令会卡到超时；现在边运行边抽干管道，并对每路输出设 256KB 上限。
- [x] 超时与普通失败区分：记录 `timedOut` / `TimeoutError`，`retryable` 不再恒为 false。
- [x] `hy run` / `hy plan` / `hy build` 在 run 未 finished 时返回退出码 1。
- [x] `write:` 缺内容行不再静默清空文件。
- [x] 崩溃恢复：run 锁 + 启动时把无人持锁的 `running` run 修复为 `interrupted`；bash 进程组随 harness 退出而终止（Linux `PR_SET_PDEATHSIG` + Drop 守卫）。
- [x] `bash_timeout_kills_entire_process_group` 的不稳定断言改为按子进程 pid 检查。
- [x] `cargo clippy --all-targets -- -D warnings` 恢复干净（engine test module 移到文件末尾）。
- [x] agent loop 复用 reqwest client，避免每轮重新握手。
- [x] `hy plan fix the bug` 等多词 subcommand prompt 可解析。
- [x] npm 分发渠道：`hyper-agent` 主包 + 5 个平台子包（`optionalDependencies`，按 `os`/`cpu` 自动择一），发布流水线新增 `npm` job，需要仓库 secret `NPMJS_TOKEN`；build 矩阵新增 `linux-arm64`（原生 arm64 runner）。

## 下一步（按优先级）

### 可观测性
- [ ] 实时展示运行中的 event stream（TUI 订阅事件，不再等任务结束一次性显示）。
- [ ] 实现 replay/resume：从指定事件/step 重放或断点续跑。
- [ ] agent loop 的观测结果按「头 + 尾」截断：构建/测试日志的错误在尾部，当前只保留前 4000 字节。

### 工程健壮性
- [ ] streaming 响应（SSE）与 API 瞬时错误的退避重试（`retryable` 已正确标记，但尚无重试逻辑）。
- [ ] agent loop 支持并行 tool calls（一次返回多个调用并行执行）。
- [ ] 跨平台运行时：Windows 下 `sh`/`rg` 缺失问题——捆绑依赖或回退实现（`cmd`/内置搜索）；`rg` 缺失目前会让 agent 直接失败。
- [ ] 会话与事件表治理：sessions 文件去重、事件表保留策略（避免 `.harness` 无限增长）；`artifacts/` 目前没有任何写入方。
- [ ] 事件/DB 一致性：JSONL 与 SQLite 索引的兜底重建（`hy repair` 的部分能力已由启动时的 reconcile 提供）。
- [ ] 更强 sandbox：当前危险命令检查是子串黑名单，绕过容易且会误伤；资源限制（内存/CPU/输出大小上限）尚未实现。
- [ ] 模型 provider 抽象（trait + 可注入 HTTP），使 agent loop 可被 mock 测试。
- [ ] SQLite 并发：启用 WAL 并显式设置 busy_timeout。
