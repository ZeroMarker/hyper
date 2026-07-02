# Harness 从 0 开始技术选型

## 产品定位

目标是开发一个通用 agent workflow harness，用来定义、执行、观察和复现 agent 任务流。

它不是某个 LLM SDK 的简单封装，而是一个执行编排层：

- 任务可以被结构化定义。
- 步骤可以被确定性执行或半确定性执行。
- 每次运行都能留下可审计事件。
- runner/backend 可以替换。
- 失败、重试、暂停、恢复都应有明确语义。

## 推荐技术栈

### 核心语言

推荐：Python 3.12+

理由：

- agent、LLM、自动化生态最完整。
- 原型速度快，适合快速验证 workflow、runner、tool 调用模型。
- 类型系统已经足够支持 `Protocol`、`dataclass`、`TypedDict`、`pydantic` 等建模方式。
- 后续可以把性能敏感或隔离执行部分拆成独立 worker，而不是一开始用复杂语言实现全部。

备选：

- TypeScript：适合以 Web 控制台和 Node.js 工具生态为主的场景。
- Go：适合做高并发执行器和单文件部署，但 agent 生态和原型速度不如 Python。
- Rust：适合强隔离和高可靠底层 runtime，不适合作为第一版全栈开发语言。

结论：第一版用 Python，先把任务模型、事件模型、执行语义做稳定。

## 多技术框架选择

### 方案 A：Python-first

适合场景：

- 主要服务 agent、LLM、自动化、数据处理任务。
- 需要快速迭代 runner、tool calling、prompt/workflow 实验。
- 团队希望先做 SDK + CLI，再考虑服务端和 UI。

推荐组合：

- Python 3.12+
- uv
- pydantic v2
- typer
- pytest
- ruff
- mypy
- SQLite + JSONL
- 后续 FastAPI

优点：

- agent 生态最好。
- 原型最快。
- 测试 mock runner、tool runner 很方便。
- 单机 CLI 和 SDK 体验好。

缺点：

- 高并发 worker 和强隔离执行需要额外设计。
- 长期服务端部署时，需要更严格的进程、队列和资源隔离。

最佳实践：

- 核心包不要直接依赖具体 LLM SDK。
- 先稳定 Task/Event/Runner 三个接口。
- 把 ShellRunner、LLMRunner、BrowserRunner 做成扩展层。
- 用事件日志作为事实来源，不依赖内存状态还原结果。

### 方案 B：TypeScript-first

适合场景：

- 产品重点是 Web 控制台、可视化 workflow builder、团队协作界面。
- runner 大量调用 Node.js 工具链。
- 希望前后端共享类型定义。

推荐组合：

- TypeScript
- Node.js 22+
- pnpm
- zod
- vitest
- eslint/biome
- SQLite/Postgres
- Hono、Fastify 或 NestJS
- React + Vite

优点：

- Web UI 和 API 类型共享更顺。
- 生态适合构建可视化编排界面。
- 前端、服务端、CLI 可以统一语言。

缺点：

- Python agent/data 生态接入需要跨进程或服务调用。
- 本地自动化、数据科学、LLM 实验的便利性通常不如 Python。

最佳实践：

- 用 zod 定义 Task/Event schema，并生成 JSON schema。
- runner 执行层预留 Python worker 接口。
- UI 只消费事件和 summary，不直接读取执行器内部状态。
- 不要把 React flow builder 的数据结构当作核心任务模型。

### 方案 C：Go runtime + Python SDK

适合场景：

- 需要高可靠、低资源占用、易部署的执行器。
- runner 可能长期运行，任务量较大。
- 希望核心 runtime 是单二进制。

推荐组合：

- Go
- cobra
- sqlite
- zerolog/slog
- OpenTelemetry
- Python SDK 负责 agent 生态接入

优点：

- 部署简单。
- 并发和进程管理强。
- 适合长期运行 worker。

缺点：

- 第一版开发成本更高。
- agent 生态适配需要更多 glue code。
- SDK/runtime 边界需要一开始设计清楚。

最佳实践：

- Go 只做 runtime、事件、调度、隔离。
- Python SDK 负责任务生成、runner 扩展、LLM 接入。
- 用 JSON schema 或 protobuf 固化跨语言协议。

### 方案 D：Rust runtime + Python/TS SDK

适合场景：

- 对安全、隔离、性能、可嵌入能力要求很高。
- 目标是构建长期稳定的底层执行 runtime。
- 团队有 Rust 工程经验。

推荐组合：

- Rust
- tokio
- serde
- clap
- sqlx
- tracing
- pyo3 或独立进程协议

优点：

- 性能和可靠性最好。
- 类型和错误处理严格。
- 适合底层 sandbox、artifact 管理、事件 runtime。

缺点：

- 原型速度慢。
- LLM/agent 生态需要桥接。
- 对早期产品探索不够灵活。

最佳实践：

- 不建议第一版全量 Rust。
- 可以后续把执行隔离、日志采集、sandbox worker 下沉到 Rust。
- 跨语言接口必须稳定后再做 Rust 化。

## 选型决策矩阵

| 维度 | Python-first | TypeScript-first | Go runtime | Rust runtime |
| --- | --- | --- | --- | --- |
| agent 生态 | 强 | 中 | 弱 | 弱 |
| 原型速度 | 强 | 强 | 中 | 弱 |
| Web 产品 | 中 | 强 | 中 | 中 |
| CLI/SDK | 强 | 中 | 强 | 强 |
| 高并发 worker | 中 | 中 | 强 | 强 |
| 执行隔离 | 中 | 中 | 强 | 强 |
| 团队上手 | 强 | 强 | 中 | 弱 |
| 长期底层可靠性 | 中 | 中 | 强 | 强 |

推荐判断：

- 先做 agent harness 核心：选 Python-first。
- 先做可视化 workflow 产品：选 TypeScript-first。
- 先做部署型 worker/runtime：选 Go runtime + Python SDK。
- 先做安全隔离底层平台：选 Rust runtime + Python/TS SDK。

## 最佳实践原则

### 1. 核心模型优先于框架

先稳定这些协议：

- Task schema
- Step schema
- Event schema
- Runner protocol
- Failure model
- Artifact model

框架可以换，协议不要频繁变。

### 2. 事件日志是事实来源

每次运行都应该能从事件回答：

- 什么时候开始。
- 执行了哪些步骤。
- 哪一步失败。
- 输入输出是什么。
- 失败原因是什么。
- 能不能 replay 或 resume。

不要只把状态存在内存里，也不要只依赖最终 summary。

### 3. 执行器和定义层解耦

任务定义不应该知道底层 runner 是：

- LLM
- shell
- Python function
- browser automation
- remote worker

任务只描述意图和参数，runner 负责执行。

### 4. 第一版限制功能范围

第一版只做：

- 线性 steps。
- 顺序执行。
- fail-fast。
- JSON task。
- JSONL events。
- NoopRunner + ShellRunner。
- CLI validate/run。

不要第一版就做：

- 完整 DAG。
- 分布式队列。
- 多租户权限。
- Web workflow builder。
- 插件市场。
- 复杂 retry policy。

### 5. 扩展点要少而稳定

优先设计三个扩展点：

- Runner：怎么执行一步。
- Storage：事件写到哪里。
- Renderer/Reporter：怎么展示运行结果。

不要过早开放过多 hook，否则核心语义会被插件反向绑架。

### 6. 可复现性默认开启

每次运行记录：

- task spec snapshot。
- run config。
- environment summary。
- runner version。
- event log。
- artifacts path。

对于 LLM runner，额外记录：

- model。
- parameters。
- prompt/messages。
- tool calls。
- response metadata。

### 7. 错误模型结构化

错误不要只存字符串。

建议字段：

- `error_type`
- `message`
- `retryable`
- `step_id`
- `details`
- `cause`

这样 CLI、UI、重试策略和报警都能复用。

### 8. 从单机到服务端平滑演进

演进路径：

1. Library + CLI。
2. JSONL + SQLite。
3. FastAPI 查询服务。
4. 后台 worker。
5. Postgres。
6. Web UI。
7. 分布式执行。

不要从第 7 步开始，否则早期会被平台复杂度拖慢。

### 包管理和工程结构

推荐：

- `uv`：作为包管理、虚拟环境、锁文件和命令运行工具。
- `pyproject.toml`：统一管理项目元数据、依赖和工具配置。
- `src/` layout：避免本地路径污染 import，适合发布 SDK/CLI。
- `ruff`：格式化和 lint。
- `mypy` 或 `pyright`：类型检查。
- `pytest`：测试框架。

建议结构：

```text
harness/
  pyproject.toml
  README.md
  src/
    harness/
      __init__.py
      models.py
      engine.py
      runner.py
      events.py
      storage.py
      cli.py
  tests/
  examples/
```

## 核心模块选型

### 任务定义

推荐格式：JSON

理由：

- 标准库原生支持。
- 适合机器生成、版本控制、测试快照和跨语言消费。
- 与事件输出格式一致，降低认知成本。

后续可选：

- YAML 作为可选 extra，用于手写任务文件。
- Python DSL 用于高级用户动态生成任务。

不建议第一版直接设计复杂 DSL。先把 JSON schema 和执行语义定清楚。

### 数据建模

推荐：

- 内部模型使用 `pydantic` v2。
- 公共只读对象可以暴露为 frozen model 或 frozen dataclass。
- JSON schema 由模型导出。

理由：

- 从 0 开始时，输入校验、错误信息和 schema 生成非常重要。
- 任务文件是外部接口，不能只靠手写 `dict` 校验。
- pydantic v2 性能和类型体验都足够好。

适用模型：

- `TaskSpec`
- `StepSpec`
- `RunSpec`
- `RunContext`
- `Event`
- `RunnerResult`
- `Failure`

### 执行引擎

推荐：同步核心 + 可选异步扩展

第一版语义：

- 默认顺序执行。
- 默认 fail-fast。
- 每一步产生 `step.started` 和 `step.finished` 或 `step.failed`。
- 整体产生 `run.started` 和 `run.finished` 或 `run.failed`。
- 所有状态变化都先写事件，再进入下一状态。

后续扩展：

- 并发步骤。
- DAG 依赖。
- 条件分支。
- 重试策略。
- pause/resume。
- human-in-the-loop。

不建议第一版直接上完整 DAG 引擎。先把线性 workflow 做严谨。

### Runner 接口

推荐：`typing.Protocol`

接口方向：

```python
class Runner(Protocol):
    def run_step(self, step: StepSpec, context: RunContext) -> RunnerResult:
        ...
```

理由：

- 不强制继承基类。
- 方便接入函数式 runner、类 runner、mock runner。
- 类型清晰，测试容易。

第一批 runner：

- `NoopRunner`：用于测试和示例。
- `ShellRunner`：执行本地命令，适合自动化任务。
- `PythonFunctionRunner`：调用注册的 Python 函数。
- LLM runner 暂缓，先定义扩展接口。

### 事件系统

推荐：事件溯源风格，但第一版不要引入复杂 event sourcing 框架。

事件格式：JSONL

核心事件：

- `run.started`
- `step.started`
- `step.finished`
- `step.failed`
- `run.finished`
- `run.failed`

每个事件包含：

- `event_id`
- `run_id`
- `task_id`
- `type`
- `timestamp`
- `step_id`
- `step_index`
- `payload`

理由：

- JSONL 适合 streaming、日志、调试、测试断言。
- 事件是最稳定的审计接口。
- 后续可以把同一事件写入文件、SQLite、Postgres、OpenTelemetry。

### 存储

第一版推荐：文件系统 + SQLite

分层：

- JSONL：原始事件日志。
- SQLite：本地索引、查询、运行历史。

理由：

- 文件系统方便调试和归档。
- SQLite 零运维，适合 CLI、本地 harness 和 CI。
- 后续迁移 Postgres 时，事件模型不需要重写。

后续可选：

- Postgres：多用户、服务端部署、并发查询。
- S3/R2：长期归档事件和 artifacts。
- Redis：只用于队列或临时状态，不作为核心事实来源。

### CLI

推荐：`typer`

理由：

- 类型提示友好。
- 命令扩展比 `argparse` 更舒服。
- 自动 help 文档更清晰。

核心命令：

- `harness validate task.json`
- `harness run task.json`
- `harness events <run-id>`
- `harness summary <run-id>`
- `harness replay <run-id>`

如果强依赖最小化，也可以用 `argparse`。从 0 开始且目标是可用产品，推荐 `typer`。

### TUI

推荐：`Textual`

定位：

- TUI 是 CLI 和 Web UI 之间的交互层。
- 它面向本地开发者、agent 调试者、CI/远程终端用户。
- 它不替代 CLI 的脚本化能力，也不承担 Web UI 的多人协作能力。

核心价值：

- 实时查看 agent run 的事件流。
- 快速定位失败步骤。
- 查看 step 输入、输出、错误和 artifacts。
- 对比 run summary。
- 从失败步骤 replay 或 resume。

#### TUI 框架选择

方案 A：Python `Textual`

适合：

- 核心 runtime 使用 Python。
- 希望 TUI 直接复用 `pydantic` 模型、SQLite 存储和事件 reader。
- 第一版需要快速落地。

优点：

- Python 生态内集成最顺。
- 支持异步刷新、布局、表格、树、日志视图。
- 比 `curses` 更现代，开发效率更高。
- 适合做复杂终端应用，而不只是简单菜单。

缺点：

- 终端渲染复杂度比纯 CLI 高。
- 需要额外处理小屏幕、键盘焦点和日志刷新性能。

结论：Python-first harness 的第一版 TUI 推荐 `Textual`。

方案 B：Rust `ratatui`

适合：

- runtime 是 Rust 或 Go，TUI 要做成高性能单二进制。
- 需要非常强的终端渲染控制。
- 团队熟悉 Rust。

优点：

- 性能好。
- 交互可控。
- 适合长期运行的独立 TUI。

缺点：

- 与 Python agent 生态共享模型成本高。
- 第一版迭代慢。

结论：适合后期 runtime 下沉到 Rust 后再考虑。

方案 C：Node.js `ink`

适合：

- TypeScript-first 项目。
- 前后端和 CLI 都希望共用 React 思维模型。

优点：

- React 组件模型熟悉。
- TypeScript 类型体验好。

缺点：

- 和 Python harness 核心之间需要进程通信或 API。
- 对长日志、高频事件刷新要谨慎设计。

结论：只有 TypeScript-first 路线才优先选。

#### TUI 信息架构

推荐主界面：

```text
┌ Runs ─────────────┬ Timeline / Events ──────────────────────────┐
│ latest runs       │ run.started                                  │
│ status            │ step.started  plan                           │
│ duration          │ step.finished plan                           │
│ selected run      │ step.failed   execute                        │
├ Steps ────────────┼ Details ─────────────────────────────────────┤
│ plan      ok      │ selected event payload                       │
│ execute   failed  │ stdout / stderr / artifacts / error details  │
│ verify    pending │                                              │
└───────────────────┴──────────────────────────────────────────────┘
```

主要视图：

- Runs：本地 run 历史、状态、耗时、任务名。
- Timeline：按时间展示事件流。
- Steps：展示 step 状态、耗时、重试次数。
- Details：展示选中 step/event 的 payload、错误、stdout/stderr、artifact。
- Summary：展示成功数、失败数、耗时、runner 信息。
- Task：展示 task spec snapshot。

#### TUI 操作设计

核心快捷键：

- `j/k` 或方向键：移动选择。
- `tab`：切换面板。
- `enter`：打开详情。
- `r`：重新运行当前 task。
- `R`：从失败步骤 resume。
- `f`：只看失败事件。
- `/`：搜索事件、step id、错误文本。
- `e`：打开 event JSON。
- `a`：打开 artifact。
- `q`：退出。

交互原则：

- 默认只读查看，不要误触发执行。
- 重新运行、resume、删除 run 这类操作必须确认。
- 所有 TUI 操作都调用核心 service，不直接改 SQLite 或 JSONL。
- TUI 展示的数据必须能用 CLI 命令复现。

#### TUI 数据流

推荐架构：

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

关键原则：

- TUI 只读事件和索引，不成为事实来源。
- 运行中的事件通过 tail JSONL 或订阅 event bus 刷新。
- 历史 run 通过 SQLite 查询。
- 大 payload 和 artifacts 按需加载，避免终端卡顿。

#### TUI 模块结构

建议结构：

```text
src/
  harness/
    tui/
      app.py
      screens.py
      widgets/
        runs.py
        timeline.py
        steps.py
        details.py
        summary.py
      bindings.py
      formatters.py
```

依赖边界：

- `tui/` 可以依赖核心 query service。
- 核心 `engine/runner/events/storage` 不能依赖 `tui/`。
- TUI 格式化逻辑放在 `formatters.py`，不要混入模型层。

#### TUI 第一版范围

Milestone TUI-1：

- `harness tui` 命令。
- 展示本地 run 列表。
- 展示选中 run 的 timeline。
- 展示 step 状态和错误详情。
- 支持搜索和失败过滤。
- 支持打开原始 event JSON。

Milestone TUI-2：

- 实时跟随当前运行。
- 查看 stdout/stderr。
- 查看 artifact 列表。
- 从失败步骤 resume。
- 比较两次 run summary。

Milestone TUI-3：

- 支持远程 API backend。
- 支持多项目 workspace。
- 支持主题、布局保存。
- 支持导出 run report。

#### TUI 测试策略

推荐：

- 纯格式化函数做单元测试。
- Query service 用 SQLite fixture 测试。
- Textual app 用 snapshot/smoke test。
- 事件刷新逻辑用 fake event stream 测试。

不要测试：

- 终端像素级布局。
- 第三方库内部键盘事件实现。
- 过度脆弱的颜色和边框细节。

#### TUI 最佳实践

- 先做只读 viewer，再做执行控制。
- 先支持历史 run，再支持实时 run。
- 先展示结构化事件，再美化 timeline。
- 大文本默认折叠，按需展开。
- 错误详情优先展示 `error_type`、`message`、`retryable`、`step_id`。
- 保持快捷键和 CLI 命令一一对应。
- 所有危险操作都必须二次确认。

### API / 服务端

第一阶段不做服务端，只做 library + CLI。

第二阶段推荐：

- `FastAPI`：提供 run、events、summary、artifact 查询接口。
- `uvicorn`：开发和轻量部署。
- `pydantic`：与核心模型复用。

不建议一开始就做 Web 控制台。先把执行核心和事件格式做稳定。

### Web UI

第三阶段再做。

推荐技术栈：

- React + TypeScript。
- Vite。
- TanStack Query。
- Tailwind CSS 或普通 CSS modules。
- 若需要复杂 timeline，可用 visx 或自定义 SVG/canvas。

UI 目标：

- 查看 task。
- 查看 run timeline。
- 查看每一步输入、输出、错误。
- 比较两次 run。
- 重新执行或从失败处恢复。

### 队列和并发

第一版不引入队列。

第二阶段推荐：

- 本地并发：`asyncio` 或 `concurrent.futures`。
- 分布式队列：`dramatiq` + Redis，或 Celery。

选择原则：

- 任务执行时间短、本地为主：不需要队列。
- 任务执行时间长、需要后台运行：引入队列。
- 多 worker、多租户：队列和 Postgres 一起引入。

### 可观察性

第一版：

- JSONL 事件。
- CLI summary。
- 结构化错误。

第二阶段：

- OpenTelemetry traces。
- Prometheus metrics。
- structured logging。

不建议第一版直接引入完整 observability stack。先保证事件足够完整。

### 测试

推荐：

- `pytest`
- `pytest-cov`
- `hypothesis` 用于任务 schema 和状态机边界测试
- golden file 测试用于事件 JSONL 输出

测试优先级：

- 任务校验。
- 事件顺序。
- runner 成功、失败、返回空结果。
- fail-fast 语义。
- CLI exit code。
- replay 行为。

### CI/CD

推荐：GitHub Actions

第一版 CI：

- Python 3.12。
- `uv sync`
- `ruff check`
- `ruff format --check`
- `mypy`
- `pytest`

稳定后扩展：

- Python 3.11、3.12、3.13 matrix。
- build wheel。
- README/package metadata check。
- release workflow。

## 不推荐第一版采用

- Kubernetes：过早。
- Temporal / Prefect / Airflow：会吞掉 harness 自身的执行语义设计空间。
- LangChain / LlamaIndex 作为核心依赖：可以做 runner 扩展，但不应绑定核心。
- MongoDB：事件和运行记录更适合 JSONL + SQLite/Postgres。
- 完整插件系统：先用 Python entry points 或 runner registry 解决。
- 多租户权限系统：等服务端阶段再设计。

## 推荐路线图

### Milestone 0：设计冻结

- 定义 Task JSON schema。
- 定义 Event JSON schema。
- 定义 Runner protocol。
- 定义失败、重试、取消、超时的语义边界。

### Milestone 1：本地最小可用

- 实现线性任务执行。
- 实现 NoopRunner。
- 实现 JSONL event writer。
- 实现 `validate` 和 `run` CLI。
- 完成核心单元测试。

### Milestone 2：真实 runner

- 实现 ShellRunner。
- 实现 PythonFunctionRunner。
- 增加 artifacts 目录。
- 增加 run summary。
- 支持超时和环境变量注入。

### Milestone 3：可复现和可恢复

- 增加 SQLite run index。
- 支持查询 run history。
- 支持 replay。
- 支持从失败步骤继续。

### Milestone 4：本地 TUI

- 增加 `harness tui`。
- 支持 run 历史查看。
- 支持 timeline 和 step detail。
- 支持失败过滤、搜索、event JSON 查看。
- 支持实时跟随运行中的 JSONL 事件。

### Milestone 5：服务端和 UI

- 增加 FastAPI 服务。
- 增加 run/event 查询 API。
- 增加 Web timeline UI。
- 支持后台 worker。

## 最终推荐组合

- Language：Python 3.12+
- Package manager：uv
- Core models：pydantic v2
- CLI：typer
- TUI：Textual
- Storage：JSONL + SQLite
- Tests：pytest + hypothesis
- Lint/format：ruff
- Type check：mypy
- Server later：FastAPI
- UI later：React + TypeScript + Vite

这个组合适合从 0 开始快速建立清晰的 harness 核心，同时不会过早绑定具体 agent SDK、workflow 平台或服务端架构。
