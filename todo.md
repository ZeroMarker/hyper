# Todo

## 已完成

- [x] 默认接入 DeepSeek provider 与环境变量模型配置（`DEEPSEEK_API_KEY` / `DEEPSEEK_MODEL` / `DEEPSEEK_BASE_URL`）。
- [x] 实现 tool-calling agent loop：模型自主调用 `read`/`search`/`bash`/`write`/`edit`，观测回传，上限 12 轮；plan 模式只暴露只读工具。
- [x] 增加 Windows/macOS/Linux 发布流水线（tag `v*` 触发，4 平台构建并发布 GitHub Release）。
- [x] 路径隔离（含符号链接逃逸防护）、危险 shell 命令拦截、`tools` 白名单。
- [x] 修复 `undo` 按随机文件名取快照的问题（改为按创建时间取最新）。

## 下一步（按优先级）

### 交互与安全
- [ ] 在 TUI 中实现 approval prompt：`bash`/`write`/`edit` 在执行前需用户确认（尤其 agent loop 内）。
- [ ] 增加 shell 进程组终止：超时/取消时杀死整个进程组，避免残留子进程。
- [ ] 更强 sandbox：工作区外路径写入的细粒度控制、命令注入防护、资源限制（内存/CPU/输出大小上限）。

### 可观测性
- [ ] 实时展示运行中的 event stream（TUI 订阅事件，不再等任务结束一次性显示）。
- [ ] `hy show <run>` / TUI 中浏览 checkpoint 的 diff，支持恢复到指定 checkpoint（非仅最新）。
- [ ] `hy artifacts <run>` 列出实际产物文件（当前只打印目录路径）。
- [ ] 实现 replay/resume：从指定事件/step 重放或断点续跑。

### 工程健壮性
- [ ] streaming 响应（SSE）与 API 瞬时错误的退避重试，减少 agent loop 首字延迟。
- [ ] agent loop 支持并行 tool calls（一次返回多个调用并行执行）。
- [ ] 跨平台运行时：Windows 下 `sh`/`rg` 缺失问题——捆绑依赖或回退实现（`cmd`/内置搜索）。
- [ ] 会话与事件表治理：sessions 文件去重、事件表保留策略（避免 `.harness` 无限增长）。
- [ ] 事件/DB 一致性：JSONL 与 SQLite 索引的兜底重建（`hy repair`）。
