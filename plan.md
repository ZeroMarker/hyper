# Agent Harness 本地产品开发计划

## 目标

从 0 开发一个可落地使用的 agent workflow 本地产品，而不是先做通用 SDK。

第一阶段交付形态：

- 一个可安装的本地应用。
- 一个可直接运行的 CLI。
- 一个可交互调试的 TUI。
- 一个本地 workspace，用于保存 task、run、event、artifact。
- 一套内置 runner，能执行真实任务。

核心能力：

- 用结构化 task 定义 agent workflow。
- 用 runner 执行 step。
- 用 JSONL 事件记录完整运行过程。
- 用 SQLite 建立本地 run 索引。
- 用 CLI 支持脚本化使用。
- 用 TUI 支持本地调试、观察、失败定位和 replay/resume。
- 用项目级 workspace 管理运行历史和产物。
- 用 `plan` 只读模式探索代码库。
- 用 `build` 执行模式编辑文件、运行命令、验证结果。
- 提供 read/write/edit/bash 四个核心工具。
- 提供 git diff、checkpoint、undo，避免用户失控。

非目标：

- 第一版不做通用 SDK 平台。
- 第一版不追求被第三方库嵌入。
- 第一版不做插件市场。
- 第一版不做多租户服务端。

## 最终技术组合

- Language：TypeScript
- Runtime：Bun 优先，Node.js 22+ 兼容
- Package manager：pnpm 或 bun
- Core schemas：zod
- Agent loop：Vercel AI SDK `ToolLoopAgent` / `HarnessAgent`，或 OpenRouter Agent SDK
- TUI MVP：`@ai-sdk/tui`
- TUI Advanced：React + Ink
- Storage：JSONL + SQLite
- Test：vitest
- Lint/format：biome 或 eslint/prettier
- Type check：tsc
- Local app state：workspace directory + SQLite
- Sandbox：本地 approval gate 起步，后续接 Vercel Sandbox / Web Worker / container
- Server optional later：Hono / Fastify
- Web UI optional later：React + Vite

## 其他技术选型

### CLI 框架

推荐：`commander`

备选：`cac`、`clipanion`

选择理由：

- `commander` 成熟稳定，足够覆盖 `init`、`plan`、`build`、`tui`、`runs`、`show` 等命令。
- `cac` 更轻，但大型 CLI 的 help、子命令和参数校验不如 commander 稳。
- `clipanion` 类型体验好，但生态和熟悉度弱一些。

### SQLite 访问

推荐：`better-sqlite3`

备选：`drizzle-orm` + SQLite、`kysely`

选择理由：

- 本地 CLI/TUI 场景下同步 SQLite API 简单可靠。
- 查询量以本机 run history 为主，不需要复杂 ORM。
- 若后续 schema 复杂，再引入 Drizzle 做 migration 和类型化查询。

### 配置管理

推荐：

- `cosmiconfig`：查找项目级配置。
- `zod`：校验配置 schema。
- `dotenv`：加载本地环境变量。

配置优先级：

1. CLI 参数。
2. 环境变量。
3. 项目配置 `.harness/config.json` 或 `harness.config.ts`。
4. 用户全局配置 `~/.config/harness/config.json`。
5. 默认值。

### 日志与调试

推荐：

- `debug`：开发期 namespace 日志。
- JSONL event log：产品事实日志。
- `pino`：后续服务端或后台 worker 使用。

原则：

- 用户可见状态写入 event log。
- 开发调试日志不要污染 TUI。
- 出错时提供 `--debug` 和 `HARNESS_DEBUG=1`。

### 文件系统与路径

推荐：

- Node `fs/promises`。
- `fast-glob`：项目文件扫描。
- `ignore`：解析 `.gitignore` 和自定义 ignore。
- `chokidar`：后续 watch 模式。

原则：

- 默认尊重 `.gitignore`。
- 默认不读取 `.env`、密钥、二进制、大文件。
- 所有文件访问经过 workspace path guard。

### 搜索与代码上下文

推荐：

- 第一版：调用系统 `rg`。
- 后续：内置 `ripgrep` fallback 或 WASM/JS 搜索实现。

原则：

- 优先使用 `rg`，速度快且符合开发者习惯。
- 搜索结果进入上下文前做截断和去重。
- 大文件只读取片段，不整文件塞入上下文。

### Diff / Patch / Undo

推荐：

- `diff`：生成文本 diff。
- `git diff --no-index` 或真实 git diff：生成用户可审查变更。
- 自定义 checkpoint：每次 write/edit 前保存原始文件快照。

原则：

- edit 工具优先使用 patch，而不是全文件重写。
- 每次文件改动必须可追踪、可展示、可撤销。
- undo 不依赖用户项目一定是 git repo。

### Shell 执行

推荐：

- Node `child_process.spawn`。
- `execa` 作为更友好的封装。

原则：

- 默认流式捕获 stdout/stderr。
- 命令必须记录 cwd、env diff、exit code、duration。
- 危险命令进入 approval flow。
- plan 模式默认禁止写入型 shell 命令。

### 权限与审批

推荐：内置 policy engine

策略维度：

- 文件读取 allow/deny。
- 文件写入 allow/deny。
- shell 命令 allow/confirm/deny。
- 网络访问 allow/confirm/deny。
- 大范围改动 confirm。

第一版实现：

- 基于规则的本地 policy。
- TUI 中展示 approval prompt。
- 所有审批写入 event log。

### 沙箱

第一版：

- 本地 approval gate。
- cwd 限制。
- path guard。
- denylist 危险命令。

后续：

- Vercel Sandbox。
- Docker/container。
- Web Worker 隔离纯 JS tools。
- 远程 ephemeral workspace。

不建议第一版直接强依赖容器。先把审批、记录和撤销做好。

### 模型与 Provider

推荐：

- 第一版优先 OpenRouter 或 AI SDK provider 生态。
- 配置层保留 provider/model 抽象。

原因：

- OpenRouter 适合快速接入多模型。
- AI SDK provider 生态成熟，后续切换成本低。
- 本项目重点是 coding harness 产品体验，不是自研模型适配层。

### 会话存储

推荐：

- `.harness/sessions/<session-id>.jsonl` 保存消息和事件。
- SQLite 保存 session 索引。
- artifacts 保存大 payload、diff、stdout/stderr。

原则：

- 会话可恢复。
- 大内容不塞 SQLite。
- session 和 run 可以关联，但不要强绑定。

### Slash Commands

第一版：

- `/model` 切换模型。
- `/new` 新会话。
- `/sessions` 查看会话。
- `/export` 导出会话。
- `/permissions` 查看/修改权限。
- `/compact` 压缩上下文。
- `/help` 帮助。

后续：

- `/agents` 管理 plan/build/custom agents。
- `/skills` 管理技能。
- `/theme` 切换主题。

### Context / Skills

推荐：

- 项目级 context file：`AGENTS.md`、`.harness/context.md`。
- skill manifest：`.harness/skills/<name>/skill.json`。
- skill instructions：按需加载，不默认全部注入上下文。

原则：

- 借鉴 Pi 的 minimal + lazy skills 思路。
- 系统 prompt 保持短。
- 只在模型需要时加载完整技能说明。

### 打包与发布

推荐：

- npm package 发布。
- `bin` 暴露 `harness` 命令。
- `tsx` 用于开发运行。
- `tsup` 或 `bun build` 用于打包。

安装方式：

- `npm install -g <package>`。
- `pnpm add -g <package>`。
- `bun add -g <package>`。
- 后续提供 `curl | sh` 安装脚本。

### 测试补充

推荐：

- `vitest`：单元测试。
- `memfs`：文件系统工具测试。
- 临时目录 fixture：真实文件操作测试。
- `nock` 或 mock provider：模型/API 测试。
- snapshot：event JSONL 和 TUI 格式化输出。

不建议第一版做端到端真实模型测试作为 CI 必选项，成本和不稳定性都高。

### 性能与稳定性

关键指标：

- 冷启动时间。
- 首 token 时间。
- 工具调用延迟。
- TUI 重绘频率。
- 大仓库搜索耗时。
- event log 写入耗时。

第一版策略：

- 动态加载 provider 和重型模块。
- 大 payload 写 artifacts 文件。
- TUI 默认折叠 reasoning 和 tools。
- 搜索和文件读取设置上限。

## 架构边界

### 核心原则

- 产品体验优先于 SDK 完整性。
- Task/Event/Runner 协议保持清晰，但先服务本地应用。
- JSONL event log 是事实来源。
- SQLite 只做索引和查询加速。
- CLI 和 TUI 都通过 query service 读数据。
- TUI 不直接修改 JSONL 或 SQLite。
- runner/backend 可替换，核心不绑定具体 LLM SDK。
- 内部模块边界清晰，但不为假想第三方扩展过度抽象。
- 优先复用成熟终端 agent TUI 生态，避免第一版手写完整渲染器。

### 数据流

```text
TaskSpec / RunSpec
      │
      ▼
Engine ── EventWriter ── JSONL
      │                    │
      │                    ▼
      └────────────── SQLite index
                           │
                           ▼
                    Query Service
                           │
             ┌─────────────┴─────────────┐
             ▼                           ▼
            CLI                          TUI
```

## 推荐目录结构

```text
harness/
  package.json
  tsconfig.json
  biome.json
  README.md
  src/
    index.ts
    schemas/
      task.ts
      event.ts
      failure.ts
      config.ts
    agent/
      loop.ts
      session.ts
      adapters.ts
      providers.ts
    tools/
      read.ts
      write.ts
      edit.ts
      bash.ts
      search.ts
    policy/
      engine.ts
      rules.ts
      approvals.ts
    config/
      load.ts
      schema.ts
    commands/
      slash.ts
      handlers.ts
    workspace/
      paths.ts
      events.ts
      sqlite.ts
      artifacts.ts
      checkpoints.ts
      sessions.ts
    cli/
      index.ts
      commands.ts
    tui/
      index.ts
      ai-sdk-tui.ts
      ink/
        app.tsx
        components/
          conversation.tsx
          tool-calls.tsx
          diff.tsx
          status.tsx
  tests/
  examples/
```

## 产品形态

### Workspace

每个项目目录下使用 `.harness/` 保存本地状态：

```text
.harness/
  harness.db
  runs/
    <run-id>/
      events.jsonl
      task.json
      summary.json
      artifacts/
```

原则：

- task 可以来自任意路径，但运行时必须保存 snapshot。
- event log 是事实来源。
- SQLite 负责快速查询 run 列表、step 状态、错误、artifact 索引。
- artifacts 和 stdout/stderr 都归档到 run 目录。

### 第一版内置 Runner

第一版不要只提供抽象 runner，要内置可用能力：

- `ToolLoopAgent`：默认 agent loop，用于模型调用、工具执行、多轮迭代。
- `HarnessAgent`：用于接入 Codex、Claude Code、Pi、OpenCode 等现成 harness。
- `ShellTool`：执行 shell command。
- `AgentTUIAgent` adapter：把 agent/session 包装给 TUI 运行时。

第一版可以直接基于 `@ai-sdk/tui` 跑起来，再逐步替换或扩展内部 agent loop。产品落地比 provider-agnostic 完美抽象更重要。

### TUI 技术路径

#### 路径 A：`@ai-sdk/tui` 快速产品化

适合第一版 MVP。

优点：

- 直接提供 interactive terminal interface。
- 内置 streamed response、markdown rendering、tool cards、reasoning sections、scrolling、tool approval prompts。
- 能快速验证 agent loop、工具审批、会话和产品交互。
- 与 AI SDK agent/harness 生态衔接紧。

使用方式：

```ts
import { runAgentTUI } from '@ai-sdk/tui';

await runAgentTUI({
  title: 'Harness',
  agent,
  tools: 'auto-collapsed',
  reasoning: 'collapsed',
});
```

第一版推荐先走这条路径。

#### 路径 B：React + Ink 复杂 UI

适合第二阶段高级 TUI。

优点：

- React 组件模型成熟。
- flexbox 布局适合复杂终端界面。
- 便于组件复用、状态管理、主题和自定义面板。

适合承载：

- Diff 面板。
- Tool Calls 面板。
- Session 列表。
- Approval queue。
- Model/config switcher。
- Slash command palette。

迁移策略：

- 第一版用 `@ai-sdk/tui` 跑通产品闭环。
- 把 agent loop、session、tools、workspace 都独立出来。
- 当 TUI 交互复杂到 `@ai-sdk/tui` 不够用时，再用 Ink 重写 UI 层。

### Agent 模式

第一版至少支持两种模式：

- `plan`：只读模式，只能 read/search/list，不允许文件写入，运行 bash 需要确认。
- `build`：执行模式，可以 edit/write/bash，但所有危险命令和大范围文件改动需要确认。

核心工具：

- `read`：读取文件或片段。
- `write`：创建或整体写入文件。
- `edit`：基于 patch 的局部修改。
- `bash`：运行命令、测试和项目脚本。

产品要求：

- 每次 edit/write 后记录 diff。
- 每次 bash 记录 command、cwd、exit code、stdout、stderr。
- 每次 agent turn 记录 prompt、tool call、tool result、model response。
- 支持 checkpoint 和 undo。
- TUI 中能查看 diff、命令输出和工具调用链路。

## Milestone 0：设计冻结

目标：先把本地产品的数据协议定稳，避免后续 CLI/TUI 反复返工。

交付物：

- [ ] `TaskSpec` / `StepSpec` zod schema。
- [ ] `Event` / `Failure` / `ToolResult` zod schema。
- [ ] `AgentSession` 接口。
- [ ] Event JSON schema。
- [ ] Task JSON schema。
- [ ] 失败、取消、超时、重试的第一版语义说明。
- [ ] Workspace 目录规范。
- [ ] 内置 runner 类型定义。
- [ ] `plan` / `build` 模式语义。
- [ ] read/write/edit/bash 工具 schema。

验收标准：

- [ ] task schema 能校验合法和非法任务。
- [ ] event schema 能覆盖 run/step 成功和失败。
- [ ] agent/session 接口可以被 fake agent 实现。
- [ ] `.harness/` workspace 结构能支撑 CLI 和 TUI 查询。
- [ ] 只读模式能阻止写文件。
- [ ] build 模式能记录 diff 和命令输出。

## Milestone 1：本地最小可用 Core + CLI

目标：能在真实项目目录里初始化 workspace、执行线性任务，并稳定保存事件。

交付物：

- [ ] `harness init`。
- [ ] 顺序执行 engine。
- [ ] 默认 fail-fast。
- [ ] `NoopRunner`。
- [ ] JSONL event writer。
- [ ] SQLite run index。
- [ ] `harness validate task.json`。
- [ ] `harness run task.json`。
- [ ] `harness runs`。
- [ ] `harness show <run-id>`。
- [ ] `harness plan "<task>"`。
- [ ] `harness build "<task>"`。
- [ ] 基础 README 和 example task。

验收标准：

- [ ] `harness init` 能创建 `.harness/`。
- [ ] 成功任务输出 `run.started -> step.started -> step.finished -> run.finished`。
- [ ] 失败任务输出 `step.failed -> run.failed`。
- [ ] run 的 events、task snapshot、summary 都写入 `.harness/runs/<run-id>/`。
- [ ] SQLite 能查询 run list。
- [ ] CLI 对非法 task 返回非 0 exit code。
- [ ] plan 模式不能编辑文件。
- [ ] vitest 覆盖 schema 校验、事件顺序、工具成功/失败。

## Milestone 2：真实 Runner 和 Artifacts

目标：让产品可以执行真实本地任务，不停留在 harness demo。

交付物：

- [ ] `ShellTool`。
- [ ] TypeScript function tool registry。
- [ ] artifacts 目录规范。
- [ ] stdout/stderr 捕获。
- [ ] step timeout。
- [ ] environment 注入。
- [ ] run summary。
- [ ] `harness artifacts <run-id>`。
- [ ] git diff capture。
- [ ] checkpoint/undo 基础能力。

验收标准：

- [ ] shell step 能记录 exit code、stdout、stderr。
- [ ] 超时 step 能产生结构化 failure。
- [ ] artifacts 可被事件 payload 引用。
- [ ] summary 能统计 step 总数、成功数、失败数和耗时。
- [ ] 用户能用 CLI 找到一次 run 产生的所有文件。
- [ ] 用户能查看本次 run 修改了哪些文件。
- [ ] 用户能撤销一次 agent run 的文件改动。

## Milestone 3：最小 Agent Loop

目标：产品具备真正 agent 任务能力，而不是只有 shell workflow。

交付物：

- [ ] `ToolLoopAgent` 或 OpenRouter Agent SDK 接入。
- [ ] model/provider 配置。
- [ ] prompt/messages 记录。
- [ ] tool call 记录。
- [ ] read/write/edit/bash 工具调用。
- [ ] max turns。
- [ ] token/latency metadata。
- [ ] agent step failure 结构化。

验收标准：

- [ ] 一个 task 能调用 agent loop 完成简单 agent step。
- [ ] prompt、tool call、model response 都进入 event/artifact。
- [ ] agent 失败能在 CLI 和后续 TUI 中定位。
- [ ] build 模式能完成读文件、改文件、跑测试的闭环。

## Milestone 4：`@ai-sdk/tui` 本地交互版

目标：先用成熟 TUI 运行时做出可用交互，不急着自研复杂面板。

技术选择：`@ai-sdk/tui`

交付物：

- [ ] `harness tui` 命令。
- [ ] 创建长期 session。
- [ ] 接入 agent/session adapter。
- [ ] 展示 streamed response。
- [ ] 展示 tool cards。
- [ ] 展示 reasoning section。
- [ ] 支持 tool approval prompt。
- [ ] 支持 slash commands：`/model`、`/new`、`/export`、`/sessions`。
- [ ] 会话持久化和恢复。

快捷键：

- `/`：打开 slash command。
- `ctrl+c`：中断当前 agent turn。
- `ctrl+l`：清屏。
- `q` 或 `ctrl+d`：退出。

验收标准：

- [ ] TUI 能完成一次完整 agent 对话。
- [ ] TUI 能展示工具调用和审批。
- [ ] TUI 能保存并恢复 session。
- [ ] TUI 能在工具失败后继续运行或清晰退出。
- [ ] 所有 tool call 都写入 `.harness/` 事件和 artifacts。

## Milestone 5：React + Ink 高级 TUI

目标：当 `@ai-sdk/tui` 无法满足复杂产品交互时，用 React + Ink 建立自定义 TUI。

交付物：

- [ ] Conversation 组件。
- [ ] Tool Calls 组件。
- [ ] Reasoning 组件。
- [ ] Approval Queue 组件。
- [ ] Diff Viewer 组件。
- [ ] Session Switcher。
- [ ] Model Switcher。
- [ ] Slash Command Palette。
- [ ] Theme system。

验收标准：

- [ ] Ink TUI 能复用同一 agent/session/tools/workspace 层。
- [ ] 复杂面板不会破坏 agent loop。
- [ ] 小终端窗口能降级显示。
- [ ] 大量 tool call 不会明显卡顿。

## Milestone 6：服务端和 Web UI

目标：本地产品稳定后，再考虑服务端和 Web。它是产品扩展，不是第一版前提。

交付物：

- [ ] Hono 或 Fastify 查询服务。
- [ ] run/event/summary/artifact API。
- [ ] 后台 worker。
- [ ] React timeline UI。
- [ ] 远程 TUI backend 支持。

验收标准：

- [ ] API 与 CLI/TUI 使用同一 query service。
- [ ] Web UI 只消费 API，不读取本地文件。
- [ ] 本地 JSONL/SQLite 模式仍可独立工作。

## 第一版不做

- 完整 DAG。
- 分布式队列。
- 多租户权限系统。
- 插件市场。
- Web workflow builder。
- Kubernetes 部署。
- 把 LangChain/LlamaIndex 作为核心依赖。
- 完整 provider-agnostic LLM 抽象。
- 面向第三方开发者的公开 SDK 稳定性承诺。

## 附录：终端 Coding Agent 产品对标

这些才是本项目应重点对标的产品：它们不是通用 agent SDK，而是开发者直接使用的 coding agent CLI/TUI。

| 项目 | 产品形态 | 公开技术/架构信号 | 对本项目的启发 |
| --- | --- | --- | --- |
| [Codex CLI](https://github.com/openai/codex) | 本地终端 coding agent，也有 IDE/桌面/云端形态 | 本地运行、读写代码、执行命令、支持 IDE 扩展和云端 Codex | 本项目应优先做本地 CLI/TUI 闭环，再考虑 IDE/云端 |
| [Claude Code](https://github.com/anthropics/claude-code) | 终端 coding agent，可接 IDE 和 GitHub workflow | 读代码库、跨文件编辑、跑命令/测试、处理 git workflow | 必须把文件编辑、命令执行、测试、git diff/review 作为核心体验 |
| [OpenCode](https://opencode.ai/docs/) | 开源终端 TUI、桌面 app、IDE extension | 终端 TUI、可连接多 provider、支持模型配置、undo/customize | TUI 不是附属品，而是主产品界面；provider 配置要产品化 |
| [OpenCode agents](https://github.com/anomalyco/opencode) | 内置 build/plan agent 和 general subagent | build 是全权限开发 agent；plan 是只读探索 agent；general 用于复杂搜索和多步任务 | 第一版也应有 `plan` 只读模式和 `build` 执行模式，而不是单一 agent |
| [Qwen Code](https://github.com/QwenLM/qwen-code) | 开源终端 coding agent | 面向 Qwen 模型优化，生活在终端，帮助理解大型代码库和自动化工作 | 可以先为一个强模型做深度优化，再保留多 provider 配置 |
| [Qwen Code docs](https://qwenlm.github.io/qwen-code-docs/en/users/overview/) | 终端 agent 产品文档 | 强调快速开始、终端内把想法转成代码 | onboarding 要极短：安装、登录/配置、进入项目、开始任务 |
| [Pi](https://pi.dev/) | 极简 agent harness + coding agent CLI | 可通过 extensions、skills、prompt templates、themes、packages 自定义；刻意跳过内置 sub-agents/plan mode | 核心要小，能力通过 skills/extensions 渐进加载；不要把所有说明塞进系统 prompt |
| [Pi monorepo](https://github.com/earendil-works/pi) | TypeScript monorepo，多 npm 包 | coding agent CLI、agent runtime、multi-provider LLM API、TUI library | 若走产品路线，monorepo + 多 package 是合理形态：core、ai、tui、cli 分层 |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | 开源终端 AI agent | 终端自然语言工作流，MCP/Google Search/Code Assist 集成信号明显 | MCP 和外部工具生态要作为产品级集成，不只是内部 runner |

### 对标结论

- 主赛道是 terminal-first coding agent，不是 Python SDK。
- 用户关心的是能不能读项目、改文件、跑测试、看 diff、撤销、恢复、接模型、接工具。
- TUI/CLI 是主产品界面，不是调试辅助。
- `plan` 与 `build` 两种权限模式很重要：先读后写、先分析后执行。
- 文件编辑、shell、git、测试、搜索、上下文压缩是核心能力。
- 安全边界必须产品化：权限提示、危险命令拦截、只读模式、diff 审核、撤销。
- 配置和状态必须项目级落盘：session、history、模型、权限、技能、context files。
- 多 provider 支持是产品卖点，但第一版可以先打磨一个 provider 的深度体验。

### 对本项目的新取舍

采用：

- Terminal-first。
- TUI-first。
- TypeScript-first。
- `@ai-sdk/tui` MVP。
- React + Ink advanced TUI。
- 本地 workspace。
- session persistence。
- plan/build 双模式。
- read/write/edit/bash 四个核心工具。
- git diff 和 undo/checkpoint。
- shell/test 执行闭环。
- skills/context files。
- provider 配置产品化。

暂缓：

- 通用 SDK。
- Web workflow builder。
- 云端多租户。
- 复杂多 agent 编排。
- 完整 IDE extension。
- 分布式 durable workflow。

## 测试策略

优先测试：

- zod schema 校验。
- event 顺序。
- tool 成功、失败、超时。
- JSONL golden file。
- SQLite query service。
- CLI exit code。
- agent session persistence。
- tool approval flow。
- `@ai-sdk/tui` smoke test。
- Ink component formatter/render smoke test。

不优先测试：

- 终端像素级布局。
- 第三方 TUI 库内部键盘事件。
- 颜色、边框等脆弱样式细节。

## 开发顺序

1. 先实现 workspace、模型和 schema。
2. 再实现 engine、JSONL events、SQLite index。
3. 再实现 CLI init/validate/run/runs/show。
4. 再加入 read/write/edit/bash tools、artifacts、stdout/stderr。
5. 再接入最小 agent loop。
6. 再用 `@ai-sdk/tui` 做交互 MVP。
7. 再加入 session persistence、slash commands、approval flow。
8. 需要复杂 UI 时再做 React + Ink。
9. 最后考虑服务端和 Web UI。

## 最小可用版本定义

MVP 完成条件：

- [ ] 能初始化 `.harness/` workspace。
- [ ] 能定义 JSON task。
- [ ] 能通过 CLI 校验 task。
- [ ] 能执行线性 task。
- [ ] 能输出 JSONL event log。
- [ ] 能写入 SQLite run index。
- [ ] 能执行 read/write/edit/bash 工具。
- [ ] 能执行最小 agent loop。
- [ ] 能归档 stdout/stderr/artifacts。
- [ ] 能在 `@ai-sdk/tui` 中完成一次完整 agent 会话。
- [ ] 能展示工具调用、reasoning、approval prompt。
- [ ] 能保存并恢复 session。
- [ ] 核心测试通过。
