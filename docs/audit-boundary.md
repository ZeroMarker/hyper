# 审计边界与 P0-3 分阶段交付

2026-10-03。本轮交付共用工具权限和直接文件工具保护；P0-3 尚未整体完成。

## 已交付的边界

CLI/TUI 使用同一个 `ToolPermissions` 判定，默认 read/search allow、bash/write/edit ask。
CLI 没有交互审批处理器，ask 明确拒绝并返回失败；TUI 每次调用单独审批。
显式 `--approval`、`--permissions FILE` 与 `HYPER_APPROVAL` 的优先级、来源和有效值
固定在运行事件中。配置不会从仓库说明自动加载，也不会在同一次运行中重新读取。
旧库 API 保留兼容行为；新 `RunOptions.permissions` 使用相同默认策略。

权限 allow 不能覆盖 plan/read-only、步骤工具白名单、工作区路径校验或 shell OS 边界。
read/write/edit 拒绝 `.harness` 审计/控制路径、解析后的符号链接别名和 Unix 硬链接；
search 及自动上下文也过滤这些文件，文件工具在审批之后重复检查目标。
直接工具暂时拒绝整个 `.harness`，包括 artifacts/tmp。用户仍可通过专门命令查看产物、
恢复检查点和 replay。Windows 硬链接与并发路径替换的描述符级保护尚未交付。
工具权限不是整个进程的文件读取隔离，模型还会收到受过滤和预算约束的工作区上下文。

验收由 [权限集成测试](../tests/permissions.rs)、既有取消/运行测试和 Linux PTY 测试覆盖。
针对边界的关键检查包括实际文件保持不变、没有工具启动/后续调用、权限来源落盘、
逐调用审批、JSONL 不混入交互文本，以及 allow 不扩大执行权限。

## 已复现且未解决的 shell 风险

在本地 Linux 的一次性临时仓库，以明确的 `--approval allow` 运行以下任务：

```json
{"name":"audit boundary probe","steps":[{"id":"probe","mode":"build","instruction":"bash:printf forged > .harness/probe.txt"}]}
```

实测退出 0，`.harness/probe.txt` 内容为 `forged`。这说明当前 workspace-write shell
规则仍包含审计目录；可以继续扩展为 events/DB/task/summary/checkpoint 篡改，
不能将直接工具保护视为完整审计防伪。此次复现只使用临时目录，没有修改真实运行记录。

当前 [sandbox.rs](../src/sandbox.rs) 对工作区根目录授予 Landlock 写权限。
[Linux 内核 Landlock 文档](https://www.kernel.org/doc/html/latest/userspace-api/landlock.html)
描述其文件层级授权和规则层约束。结合当前代码，单个 ruleset 对根目录的授权无法用
一条子目录规则收回；新增命令字符串过滤也不能限制 Python、子 shell 或重定向的实际写入。
这是一项对现有授权结构的判断，不意味着不能通过新的存储层级或独立隔离机制解决。

## 下一项的实现与验收约束

将事实日志、DB、task、summary、session 和 checkpoint 放入 shell 可写范围之外，
同时保留旧工作区迁移、备份/移动、并发打开、崩溃恢复与 replay 的确定性。
候选为外部受保护存储，或能够失败关闭的独立挂载/执行边界；需要实际验证后选择。
不能只移动日志却继续信任可被 shell 替换的 `.harness` 定位文件，也不能将只读 symlink
展示误当成内容和定位都受保护。metadata、硬链接、预开描述符及现有进程权限需要单独测试。

为 shell tmp 和可授权 artifact 写入提供独立目录及范围；审批规则后续支持路径/命令范围。
明确管理命令的授权范围，覆盖符号链接替换、审计伪造/删除、子 shell、重定向、
重复调用和仓库 prompt injection。保持不支持平台拒绝受限 shell 的行为。
本轮只在 Linux 验证，macOS/Windows 的原生边界仍属于待办。
