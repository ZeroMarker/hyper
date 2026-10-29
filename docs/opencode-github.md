# OpenCode Issue 自动开发

本仓库提供两个 GitHub Actions 入口，使用 OpenCode 官方 GitHub 集成和
OpenCode Go 的 `deepseek-v4.1-flash` 模型，在 GitHub 托管 Runner 上修改代码并创建 PR。人工检查 Diff、
测试结果和 CI 后再合并。无需本机或 VPS 常驻 Agent。

## 一次性配置

1. 为 `ZeroMarker/hyper` 安装
   [OpenCode Agent GitHub App](https://github.com/apps/opencode-agent)，确认 App
   获得创建分支、提交、Issue 回复和 PR 所需的权限。
2. 在 [Actions Secrets](https://github.com/ZeroMarker/hyper/settings/secrets/actions)
   添加 `OPENCODE_API_KEY`，使用 OpenCode Go 的 Key，按需限制预算。
3. 在 [Actions Variables](https://github.com/ZeroMarker/hyper/settings/variables/actions)
   添加 `OPENCODE_MODEL`，当前值为 `opencode-go/deepseek-v4.1-flash`。更换模型时
   先在供应商目录确认模型可用、支持工具调用且有足够额度。
4. 个人仓库的新建 Issue 入口默认只接受仓库所有者；其他可信账号需加入可选变量
   `OPENCODE_ALLOWED_ACTORS`，例如 `["trusted-login"]`。组织仓库必须配置账号
   白名单；组织名不代表真人账号。账号仍需具有仓库写权限。
5. 确认工作流已进入默认分支、Actions 允许执行依赖的 Action，并按需要配置默认
   分支规则：要求 PR、人工审批、实际 CI 检查通过，Agent/App 不得绕过。

项目自身的模型配置与 OpenCode 的 Provider 配置独立；本机 Hyper 的模型设置不会
自动传到 GitHub Runner。本次从本机已保存的 OpenCode Go 凭据通过标准输入写入
仓库 Secret，并单独设置模型变量；工作流通过环境变量 `OPENCODE_API_KEY` 使用它，
不依赖 Runner 上的登录文件。不要在 Issue 或日志中粘贴模型密钥。

## 使用

由仓库所有者或白名单账号新建普通 Issue，即自动启动 OpenCode。直接在标题和
正文中写明需求、复现步骤、预期行为和验收条件，无需评论命令或添加标签。
入口为 `issues: opened`，只接受真人账号，OpenCode 还会检查实际仓库写权限；
非授权账号创建的 Issue 会跳过 Agent 任务。

现有 Issue 不会补跑；编辑正文、重新打开 Issue 或添加 `ai-task` 标签均不会
自动触发。需求不明确时，Agent 会先询问而不修改文件。

评论入口保留，供已有 Issue 手动派单或重试。请先检查运行记录和已有 PR，避免
与新建 Issue 的自动任务重复。在未关闭的普通 Issue 中发表评论，命令从正文
开头开始，后接空格和任务内容：

```text
/oc 先复现这个 Bug，再修复并补充回归测试，创建 PR。
```

也支持 `/opencode`。只发命令没有任务正文时，Agent 会总结并询问具体任务。
入口只接受真人 `OWNER`、`MEMBER`、`COLLABORATOR`，OpenCode 还会检查实际仓库
权限；组织成员身份本身不保证拥有写权限。PR 评论和编辑已有评论不会触发。

两个入口对同一 Issue 使用共同并发组，保留正在执行的任务，默认仅保留一个等待
任务；连续派单可能替换先前等待任务。它不提供去重或可靠任务队列。

## 执行与验收

Runner 安装 Rust 1.94.0、rustfmt、Clippy、Node.js 22 和 ripgrep，Agent 依据
[AGENTS.md](../AGENTS.md) 运行项目检查。现有
[Rust CI](../.github/workflows/ci.yml) 继续验证 Linux 格式/Clippy/全量测试、离线
评测、release 构建及 macOS/Windows 的原生边界；不调用付费真实模型评测。

每个任务最多 45 分钟。OIDC 获取 App 写令牌，Runner 的 `GITHUB_TOKEN` 仅有
`contents: read`；检出不保存凭据，并关闭 OpenCode 会话分享。CI 和人工 Review
才决定是否合并，不能把“Action 成功”或“有 PR”当成全部测试通过。

首次验收由可信账号新建一个需求明确的小型 Issue，无需追加命令或标签，逐项确认：

1. 配置预检查、App OIDC 换令牌、模型调用均成功。
2. 生成独立分支和含预期修改的 PR，没有直接更新默认分支。
3. PR 记录真实检查命令与结果，现有 CI 成功，人工 Review Diff。
4. 非授权账号创建的 Issue、非授权评论、PR 评论均跳过任务；编辑、重新打开和
   标签变更不会启动自动任务。

需要排障时查看 [Actions](https://github.com/ZeroMarker/hyper/actions)，按入口条件、
Secret/Variable、App 授权、模型认证、测试、推送、PR、CI 的顺序检查。没有配置
Key 或模型变量时，任务在调用 Agent 前明确失败，不输出密钥。

工作流固定 OpenCode Action 包装层到
`77fc88c8ade8e5a620ebbe1197f3a572d29ae91a`，但该版本仍动态下载 CLI，不能据此
认为整个运行时已锁定。项目规则和提示词也不是强制安全边界，模型与 Shell 在
运行期间能访问注入的凭据；应限制派单者、密钥权限与预算。

参考：[官方 GitHub 集成](https://opencode.ai/docs/github/)、
[OpenCode Go Provider 定义](https://github.com/anomalyco/models.dev/blob/dev/providers/opencode-go/provider.toml)、
[DeepSeek v4.1 Flash 模型定义](https://github.com/anomalyco/models.dev/blob/dev/providers/opencode-go/models/deepseek-v4.1-flash.toml)、
[Action 实现](https://github.com/anomalyco/opencode/blob/77fc88c8ade8e5a620ebbe1197f3a572d29ae91a/github/action.yml)、
[GitHub 并发说明](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)。
