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

GitHub Actions 在 `main` 分支和 Pull Request 上自动运行 fmt/clippy/test/release 构建（`.github/workflows/ci.yml`）。
