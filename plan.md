# Harness Rust 产品计划

## 技术基线

- Rust 1.94，单二进制交付。
- Clap CLI。
- Ratatui + Crossterm TUI。
- Serde 数据契约。
- JSONL 事实日志 + Rusqlite 索引。
- 本地 policy、checkpoint 和 undo。

## 产品边界

Harness 是本地 agent workflow 产品：结构化执行 task、记录可审计事件、管理运行历史，并通过 CLI/TUI 提供操作入口。核心数据格式保持稳定，UI 只消费核心层公开模型。

## 后续阶段

1. 引入真实模型 provider 和 tool-calling loop。
2. 为 shell/write 操作加入 TUI 交互审批。
3. 增加 event 实时订阅、diff viewer、replay/resume。
4. 增加配置文件、结构化 tracing 和跨平台发布。
5. 加强 sandbox 与资源限制。
