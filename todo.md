# Todo

只列待办与剩余工作。已交付功能的细节、测试数和验证记录见 [progress.md](progress.md)；方向、对标与验收口径见 [plan.md](plan.md)。

## 待办

### P0：质量与运行控制

- [ ] **P0-3b2b2 元数据、剩余路径竞争与 OS 范围隔离（受阻，暂缓）**：Linux workspace-write shell 可 chmod 外部审计文件，需独立 OS 元数据边界，不能靠命令过滤。2026-10-10 评估：Landlock 只能授权不能排除子目录且不中介 chmod/chown/utime，seccomp 不能解引用路径，同 UID 属主可改自身权限，`chattr +i`/改属主/独立 UID 需特权，用户 mount namespace 不可用。唯一候选是 `SECCOMP_RET_USER_NOTIF` + 宿主监督进程（复杂、有死锁风险），未实现。剩余：OS 读取/元数据边界；Windows 描述符与恢复/快照保护；审计区 metadata/读取、已有挂载别名、同路径换目录其他形态与旧 source 写者协调。不能扩大 OS 边界。见 [边界说明](docs/audit-boundary.md)。
- [ ] **平台隔离后续**：metadata、外部读取、主动继承资源（继承 socket 的通用 I/O、宿主预建共享映射）及 macOS/Windows 原生 shell 隔离，按平台报告支持范围；非 Linux、ABI/架构不支持或过滤安装失败仍拒绝受限 shell。与 P0-3 分阶段交付。
- [ ] **取消的原生 I/O 后续**：Windows stdout 背压、终端渲染背压和 OS 阻塞文件 I/O 的有界取消；自定义同步 event writer 需自行可中断。脱离进程组的 daemon/继承管道场景需另测并纳入平台隔离。

### P1：提高任务完成率

- [ ] **真实模型基线后续**：`1125252` 已按既有口径跑通 30/30（见 [报告](evals/baselines/2026-10-14-tui-usability/report.md)）。剩余：压缩（`HYPER_AUTOCOMPACT`/`HYPER_LOOP_COMPACT`）与 verify 闭环在默认关闭时未被套件覆盖；long-session 最后一轮请求数与 token 波动需更多重复次数判断；成本仍未知。
- [ ] **P1-2 相关上下文**：剩余按 prompt/路径/语言的相关性摘录选择、Python/JS 覆盖与 repo map/语法索引，以固定任务集比较收益。（行/字节范围读取已交付。）
- [ ] **P1-3 编辑和验证闭环**：剩余 Hashline/patch 选型；原子写入、重复块、并发变更、快照与 Unicode 已验证。（唯一匹配、陈旧 hash、verify 有限重试与评测指标已交付。）
- [ ] **P1-4 项目说明**：剩余目录 override 合并语义（当前只按固定顺序拼接）与按需 skills。（根与目录 `AGENTS.md` 加载已交付。）
- [ ] **P1-5 模型能力与完成语义**：剩余可配置 provider/model 上限、协议路径与能力，可用的 tokenizer/服务端计数及明确回退；统一 Chat/Responses 的独立非流式策略；缺失/未知停止原因的 provider 策略；重复失败调用的可审计阈值（避免误伤分页/测试重跑）。恢复及快照基线的 Python 深文件残缺参数失败保留为回归样本。见 [完成语义](docs/model-completion.md)。

### P2：按实测需求扩展

- [ ] **P2-1 自动化契约**：事件 schema/版本和兼容规则、最终结果 JSON Schema；共享取消稳定后再做双向 RPC/ACP、会话 fork。JSONL 输出流不等于 RPC。
- [ ] **P2-2 按需 skills/MCP**：从一个外部工具场景起步，定义来源、信任、权限继承、超时和断连恢复，再评估 hooks/插件系统。
- [ ] **P2-3 并行与代码智能实验**：先只读 read/search 并发，再隔离会话/worktree 子 Agent；write/edit/bash 保留审批、快照与顺序语义。LSP/调试器从单语言真实任务起步，用成功率、耗时、usage 与冲突证明收益。全量插件内核、Web UI 和多人协作暂留候选池。

## 评测与存储口径

早期约 2 KB trivial run / 10 KB 工具 run 是加入 SSE、历史快照和预算事件前的样本，不能作为当前容量估算。现在每个 delta 都持久化，历史/system prompt 固定到事件；输入、SQLite 页、artifact 和 checkpoint 另占空间。旧报告记录 `.harness` 体积，新报告按外部审计目录统计（不含工作区 `.hyper-tmp`），两种口径不直接混用；均记录 `prune --runs --keep 1` 效果，外部 trace 副本不计入。

## 已完成

细节见 [progress.md](progress.md)。

- 核心：tool-calling agent loop（read/search/bash/write/edit）、三协议适配与 SSE 流式、会话与多轮上下文、事件级 replay、artifacts 落盘、`ha prune`、事件/DB 兜底重建、崩溃恢复、SQLite WAL。
- Provider：DeepSeek 默认、`hyper config` 持久化、OpenCode Go 接入、命名 profile 与 list/use/test、三协议 endpoint 覆盖和有效配置审计。
- 运行控制：危险命令策略、路径/符号链接隔离、TUI 审批、进程组终止、超时区分、管道防死锁、资源限制、CLI 实时 JSONL、实时 event stream。
- 上下文：会话历史预算、总请求预算、P1-1 可审计压缩（手动 `ha compact`、`HYPER_AUTOCOMPACT`、循环内 `HYPER_LOOP_COMPACT`）。
- P0-1 固定任务集和评测脚手架；评测环境与失败样本扩展。
- P0-2 CLI/TUI 共享取消。
- P0-3a 共用工具权限；P0-3b1 外部审计存储与迁移；P0-3b2a Linux 描述符边界；P0-3b2b1 路径与命令授权；Linux restore/undo 描述符提交；Linux read-only 元数据限制；Linux 独立库快照；Linux socket 限制；Windows 硬链接检查；macOS 描述符入口与行政快照/恢复；工作区根 inode 校验。
- 完成语义：Chat/Responses 未完成拒绝与流式诊断；Messages stop_reason 拒绝。
- 分发：多平台 Release、npm 包、`hy` → `ha` 改名。
