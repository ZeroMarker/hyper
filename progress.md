# Harness Rust 迁移进度

## 当前状态

项目已经全面迁移到 Rust 1.94，不再依赖 Node.js、npm 或 TypeScript。

## 已完成

- [x] Clap CLI，生成 `hyper` 主命令和 `hy` 短命令：`init`、`validate`、`run`、`plan`、`build`、`runs`、`show`、`artifacts`、`undo`、`tui`。
- [x] Serde task/event/failure/summary 数据模型及重复 step id 校验。
- [x] JSONL 事件事实日志和 Rusqlite 本地索引。
- [x] `.harness` workspace、run artifacts、sessions 和 checkpoint/undo。
- [x] `read`、`write`、`edit`、`bash`、`search` 工具。
- [x] plan/build 策略、路径越界防护和危险 shell 命令拦截。
- [x] Ratatui + Crossterm 全屏 TUI，直接调用 Rust 核心，无子进程桥接。
- [x] 默认接入 DeepSeek OpenAI-compatible API；默认模型 `deepseek-v4-flash`，支持环境变量覆盖。
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

Rust 集成测试覆盖 task 校验、shell event、plan 只读、shell 失败、路径隔离以及 checkpoint 恢复。
