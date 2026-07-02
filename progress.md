# Agent Harness MVP 进度

## 当前状态

已完成第一版可运行 MVP。项目已从旧 Python skeleton 切换为 TypeScript 终端 coding agent harness。

当前交付形态：

- TypeScript + Node.js 24 本地 CLI。
- `.harness/` workspace。
- JSONL event log。
- SQLite run index。
- read/write/edit/bash/search 工具。
- plan/build 双模式。
- checkpoint + undo。
- 最小交互式 TUI。
- 基础测试和示例。

## 已完成

### 项目结构

- [x] 新增 `package.json`。
- [x] 新增 `tsconfig.json`。
- [x] 新增 `vitest.config.ts`。
- [x] 删除旧 Python 包结构。
- [x] 更新 `.gitignore`，忽略 `node_modules/`、`coverage/`、`.harness/`。
- [x] 更新 README。

### 核心模型

- [x] `TaskSpec` / `StepSpec` zod schema。
- [x] `Event` schema。
- [x] `Failure` schema。
- [x] 重复 step id 校验。
- [x] `plan` / `build` 模式建模。

### Workspace 和存储

- [x] `.harness/` workspace 初始化。
- [x] run 目录结构：
  - `events.jsonl`
  - `task.json`
  - `summary.json`
  - `artifacts/`
  - `checkpoints/`
- [x] SQLite run index。
- [x] event 写入 JSONL。
- [x] event 同步索引到 SQLite。
- [x] run list / run detail 查询。

### 工具系统

- [x] `read`。
- [x] `write`。
- [x] `edit`。
- [x] `bash`。
- [x] `search`。
- [x] shell stdout/stderr/exit code/duration 记录。
- [x] 写文件前 checkpoint。
- [x] diff 生成。
- [x] undo 恢复最近 checkpoint。

### 权限策略

- [x] plan 模式禁止写文件。
- [x] plan 模式运行 bash 需要确认，目前在非交互执行中直接拒绝。
- [x] build 模式允许写入和 bash。
- [x] 危险命令进入确认策略，目前在非交互执行中直接拒绝。
- [x] path guard 防止访问 workspace root 外路径。

### Agent Loop

- [x] 顺序执行 task steps。
- [x] 默认 fail-fast。
- [x] 记录 `run.started`、`step.started`、`tool.*`、`step.finished`、`run.finished`。
- [x] 失败时记录 `tool.failed`、`step.failed`、`run.failed`。
- [x] 支持 instruction prefixes：
  - `bash:`
  - `read:`
  - `search:`
  - `write:`
  - `edit:`

### CLI

- [x] `harness init`
- [x] `harness validate <task.json>`
- [x] `harness run <task.json>`
- [x] `harness plan "<query>"`
- [x] `harness build "<command>"`
- [x] `harness runs`
- [x] `harness show <run-id>`
- [x] `harness artifacts <run-id>`
- [x] `harness undo <run-id>`
- [x] `harness tui`

### TUI

- [x] 最小 readline TUI。
- [x] `/help`。
- [x] `/mode plan|build`。
- [x] `/runs`。
- [x] `/new`。
- [x] `/quit`。
- [x] 支持在 TUI 中提交 prompt 并运行 task。

### 示例和测试

- [x] 更新 `examples/hello.json`。
- [x] 新增 `examples/write-file.json`。
- [x] 新增 task schema 测试。
- [x] 新增 run/tool/checkpoint 测试。

## 验证结果

已通过：

```bash
npm run typecheck
npm test
npm run build
```

测试结果：

- 2 个测试文件通过。
- 5 个测试用例通过。

CLI smoke test 已通过：

```bash
node dist/cli/index.js init
node dist/cli/index.js run examples/hello.json
node dist/cli/index.js runs
```

## 当前依赖

运行依赖：

- `@ai-sdk/tui`
- `better-sqlite3`
- `commander`
- `diff`
- `dotenv`
- `execa`
- `fast-glob`
- `ignore`
- `nanoid`
- `zod`

开发依赖：

- `@types/better-sqlite3`
- `@types/node`
- `tsx`
- `typescript`
- `vitest`

## 已知差距

### `@ai-sdk/tui` 尚未正式接入

当前 `harness tui` 是最小 readline TUI，不是 `@ai-sdk/tui` 全屏界面。

原因：

- `@ai-sdk/tui` 需要 AI SDK `Agent` 实例。
- 当前 MVP 的 agent loop 是本地 instruction runner。
- 需要下一步接入 `ToolLoopAgent` 或 `HarnessAgent`，再把 read/write/edit/bash/search 注册成 AI SDK tools。

### 还没有真实 LLM agent

当前 build/plan 的 prompt 会被映射成本地 instruction 或 search/bash 行为，不会调用模型。

下一步需要：

- 接入 AI SDK provider 或 OpenRouter。
- 实现 model 配置。
- 实现 tool calling loop。
- 记录 prompt/messages/tool calls/model response。

### 权限确认还不是交互式

当前策略遇到 `confirm` 会在非交互执行中直接抛错。

下一步需要：

- CLI confirmation prompt。
- TUI approval prompt。
- approval event 记录。

### TUI 还不是产品级界面

当前 TUI 只适合 MVP 验证。

缺少：

- Tool cards。
- Reasoning display。
- Approval prompt。
- Session switcher。
- Diff viewer。
- Slash command palette。

### undo 只恢复最近 checkpoint

当前 `harness undo <run-id>` 恢复该 run 的最近一个 checkpoint。

后续需要：

- 支持按 checkpoint id 恢复。
- 支持整次 run 回滚。
- 展示将要恢复的文件列表。

## 下一步计划

### Step 1：接入 AI SDK ToolLoopAgent

目标：让产品具备真实 LLM agent loop。

任务：

- [ ] 安装 AI SDK provider，例如 OpenRouter/OpenAI provider。
- [ ] 新增 `src/agent/tool-loop.ts`。
- [ ] 将 read/write/edit/bash/search 封装为 AI SDK tools。
- [ ] 支持 `HARNESS_MODEL` 和 provider API key。
- [ ] 记录 user prompt、assistant response、tool call、tool result。
- [ ] 保留当前 deterministic task runner 作为测试和 CI 路径。

验收：

- [ ] `harness build "修改 README 增加一句话"` 能调用模型和工具。
- [ ] 所有 tool call 写入 `events.jsonl`。
- [ ] 无 API key 时给出清晰错误。

### Step 2：正式接入 `@ai-sdk/tui`

目标：把 TUI 从 readline 升级为全屏 agent TUI。

任务：

- [ ] 新增 `src/tui/ai-sdk-tui.ts`。
- [ ] 用 `runAgentTUI` 启动 AI SDK agent。
- [ ] 启用 tool cards。
- [ ] 启用 approval prompts。
- [ ] 配置 reasoning 展示模式。
- [ ] 保留 readline TUI 作为 fallback 或 dev mode。

验收：

- [ ] `harness tui` 进入全屏 TUI。
- [ ] 工具调用以 cards 展示。
- [ ] 危险工具调用需要用户批准。
- [ ] 会话退出后可恢复。

### Step 3：交互式权限审批

目标：将安全策略产品化。

任务：

- [ ] 定义 approval request schema。
- [ ] 在 CLI 中支持 confirm prompt。
- [ ] 在 TUI 中使用 approval prompt。
- [ ] 记录 `approval.requested` 和 `approval.resolved`。
- [ ] 支持配置默认策略。

验收：

- [ ] plan 模式执行 bash 会请求确认。
- [ ] build 模式危险命令会请求确认。
- [ ] 拒绝后 agent 能收到结构化错误。

### Step 4：Session 持久化

目标：支持长期 agent 会话。

任务：

- [ ] 完善 `.harness/sessions/<session-id>.jsonl`。
- [ ] SQLite 增加 sessions 表。
- [ ] `harness sessions`。
- [ ] `harness resume <session-id>`。
- [ ] TUI `/sessions` 和 `/new`。

验收：

- [ ] 退出后能恢复上次会话。
- [ ] 每个 session 能关联多个 run。

### Step 5：Diff 和 Review 体验

目标：让用户看得懂 agent 改了什么。

任务：

- [ ] `harness diff <run-id>`。
- [ ] `harness undo <run-id> --all`。
- [ ] TUI diff viewer。
- [ ] 大 diff 折叠。
- [ ] 改动文件列表。

验收：

- [ ] 用户能查看一次 run 的所有文件变更。
- [ ] 用户能整次回滚。

### Step 6：上下文和 Skills

目标：增强 coding agent 的项目理解能力。

任务：

- [ ] 读取 `AGENTS.md`。
- [ ] 读取 `.harness/context.md`。
- [ ] 定义 skill manifest。
- [ ] lazy skill loading。
- [ ] `/compact` 上下文压缩命令。

验收：

- [ ] agent 能按项目 context 约束工作。
- [ ] skill 不会默认全部塞进 prompt。

## 推荐优先级

最高优先级：

1. AI SDK `ToolLoopAgent`。
2. `@ai-sdk/tui` 正式接入。
3. 交互式 approval。

原因：

- 这三项决定产品是否真正接近 Codex CLI / Claude Code / OpenCode / Pi 这类终端 coding agent。
- 当前底层 workspace、events、tools、checkpoint 已经具备，下一步应打通真实 agent 交互。
