# 固定任务评测

这套脚手架衡量 Hyper 与指定模型组合的行为，不是竞品排名，也不替代 Rust 的协议/隔离测试。10 个任务的文件由 `fixtures.py` 确定性生成，任务提示、允许改动的原始文件和多轮场景在 `suite.json`。每次重复使用全新仓库、独立会话；运行不需要安装 fixture 的第三方依赖。

| 任务 | 判定 |
| --- | --- |
| rust-clamp | 包含边界和退化区间的 clamp 行为 |
| rust-cross-file | 两个模块中的运费公式与费率 |
| python-boundary | 修复已有失败测试，区间包含两端，测试文件不变 |
| python-cross-file | 价格与整数折扣，兼容旧调用 |
| js-empty | 空数组、负数和小数均值 |
| js-repeated-block | 只调整 admin，保持其他路由和顺序 |
| python-deep-file | 64 KB 之后的函数、过滤顺序/对象身份及前部完整性 |
| readonly-plan | 定位文件、常量和值，整个仓库无改动 |
| long-session | 多轮后保留 release 约束，函数行为和回答均正确 |
| checkpoint-recovery | 确认首轮错误编辑不通过独立判定，CLI 恢复到原始状态，再次修复 |

`grade.py` 不进入模型工作区，不使用模型写的测试作为成功判据。构建任务可以增加验证文件，但不能修改允许列表之外的原始文件；只读任务不能新增文件。原始错误实现必须不通过，已知正确实现必须通过，这两种情况都有离线测试。它不是防止恶意模型绕过判定的安全沙箱；命令隔离与审计区保护仍由产品负责。

## 离线验证

需要 Python 3（标准库）、Node 和 Rust，无 API 调用：

```bash
cargo build --bin hyper
python3 -m unittest discover -s evals -v
```

测试覆盖所有 fixture 的正反例、确定性与深文件位置、未知用量、超时，以及真实 Hyper CLI 对本地 HTTP stub 的端到端运行；Linux 另用 PTY 验证 TUI 审批中的 Ctrl-C 取消、继续同一会话和正常退出。可以通过 `HYPER_EVAL_BINARY` 指定其他二进制。

## 真实模型基线

先构建 release，再显式固定模型、端点、协议和预算：

```bash
cargo build --release --bin hyper
python3 evals/run.py \
  --output evals/results/my-baseline \
  --model deepseek-v4-flash \
  --base-url https://opencode.ai/zen/go/v1 \
  --protocol chat \
  --repetitions 3 --jobs 1 --timeout 180 \
  --history-tokens 16000 --context-tokens 128000 --output-tokens 8192
```

API Key 来自 `DEEPSEEK_API_KEY` 或 `--config` 指定的 Hyper 配置；默认 Linux 配置路径为 `$XDG_CONFIG_HOME/hyper/config.json` 或 `~/.config/hyper/config.json`。runner 将读取到的凭据固定到子进程环境，显式模型/端点/协议覆盖用户配置和旧环境值，不启动配置向导。Windows/macOS 可显式传入 `--config`。

`--task` 可重复指定子集。默认每项 3 次；`--jobs` 只控制相互独立的评测进程数量，不代表 Hyper 支持并行工具/Agent。吞吐与延迟对比需保持该参数一致。每一轮都有外部 wall deadline；Linux 超时清理包括 shell 单独创建的进程组。正常构建轮使用 workspace-write，规划轮使用 read-only，显式指定 `--approval allow` 授权 bash/write/edit，不扩展到 unrestricted；这与旧基线的 CLI 自动放行范围相同，但新版本默认 ask 的交互行为需要单独验证。首轮恢复任务明确要求 direct edit，因为 shell 改动没有同等快照保证。

报告记录代码 revision/dirty、二进制和评测文件 SHA-256、工具版本、模型配置、预算与并发参数；无法固定远端模型权重、采样随机性、provider 缓存或服务负载。每轮最多 12 次模型请求，沿用当前产品行为。长会话场景检查约束保留，不声称穷尽了上下文溢出或自动压缩。

## 结果与存储

输出目录必须不存在，权限为 0700。`metadata.json`、`results.jsonl`、`report.md` 是可审阅汇总；其余目录含 fixture 副本、原始事件、回答和 stderr，仅留在本地，`results/` 被 Git 忽略。汇总不包含 API Key、模型正文或 shell 输出。分享报告前仍需检查端点、路径和错误字段。

每次记录独立行为判定、CLI 完成状态、stdout 与落盘事件的一致性、每轮首次文本/工具结果延迟、总耗时、usage、错误、批准/拒绝次数、恢复结果和修改文件。JSONL 消费端计时包含启动、网络、执行和持久化开销，不是 provider 内部延迟。普通 JSON 兼容路径等完整响应后才发布文本，不能把它的首次文本时延当作 SSE 首片时延；没有文本时为 `null`。

缺失 usage 为未知，部分已知用量单列；满足预算的逻辑请求如果没有 model.iteration 回复事件，也算未观测回复（unobserved_model_replies），不能因为前几轮已知就把全轮总数标成完整。预算拒绝的未发送请求不计入缺口；必需计数缺失也标未知。不以零替代，也不臆测价格，成本目前为 `null`。任何任务失败都返回非零退出码，汇总仍保留。评测结果与离线 stub 测试分开统计。

完成失败另记录 `completion_failures`，包含 `model.failed` 的有界结束原因、计数、JSON 位置和完整 `reportedUsage`，不包含参数正文。失败回复的计数不加入完整总用量，缺失接受回复仍标未知；已接受回复的已知部分保持单列。行为与未完成范围见 [完成语义](../docs/model-completion.md)。

每个 attempt 使用 repo 外独立 `HYPER_STATE_DIR`，根据权威 state 位置验证持久化事件，并在完成后测量外部审计目录的文件数量、逻辑字节和磁盘分配字节，再运行 `prune --runs --keep 1` 并复测。保留策略只作用于该 attempt 的新仓库；会话和最终修改仍保留。单 run 任务本就没有可删除的旧 run，多轮任务可展示事件/checkpoint 清理效果。外部原始 JSONL 副本不计入审计容量，也不由 prune 删除。`.hyper-tmp`/编译产物留在工作区，不计入外部审计存储；这与旧 `.harness/tmp` 的存储口径不同，不直接比较容量。
