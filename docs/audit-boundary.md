# 审计边界与 P0-3 分阶段交付

2026-10-04。已交付共用权限、外部审计内容存储、Linux 描述符入口和工具范围规则；P0-3 的元数据与完整 OS 范围隔离仍未完成。

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
恢复检查点和 replay。Linux 直接工具的描述符级保护已交付；Windows 硬链接及非 Linux 描述符边界尚未交付。
工具权限不是整个进程的文件读取隔离，模型还会收到受过滤和预算约束的工作区上下文。

验收由 [权限集成测试](../tests/permissions.rs)、既有取消/运行测试和 Linux PTY 测试覆盖。
针对边界的关键检查包括实际文件保持不变、没有工具启动/后续调用、权限来源落盘、
逐调用审批、JSONL 不混入交互文本，以及 allow 不扩大执行权限。

## 前轮风险与本轮内容保护

在本地 Linux 的一次性临时仓库，以明确的 `--approval allow` 运行以下任务：

```json
{"name":"audit boundary probe","steps":[{"id":"probe","mode":"build","instruction":"bash:printf forged > .harness/probe.txt"}]}
```

在前轮 `7824c5b` 实测退出 0，`.harness/probe.txt` 内容为 `forged`。这说明当前 workspace-write shell
规则仍包含审计目录；可以继续扩展为 events/DB/task/summary/checkpoint 篡改，
不能将直接工具保护视为完整审计防伪。此次复现只使用临时目录，没有修改真实运行记录。

前轮 [sandbox.rs](../src/sandbox.rs) 对工作区根目录授予 Landlock 写权限。
[Linux 内核 Landlock 文档](https://www.kernel.org/doc/html/latest/userspace-api/landlock.html)
描述其文件层级授权和规则层约束。结合当前代码，单个 ruleset 对根目录的授权无法用
一条子目录规则收回；新增命令字符串过滤也不能限制 Python、子 shell 或重定向的实际写入。
这是一项对现有授权结构的判断，不意味着不能通过新的存储层级或独立隔离机制解决。

## 外部存储与显式迁移（本轮已交付）

权威记录已迁出 checkout，默认位于宿主用户状态目录。`ha state` 返回位置，
`HYPER_STATE_DIR` 可指定工作区外的绝对基目录；工作区 key 按 canonical path 的 SHA-256
派生，身份文件也在外部，仓库中的 `.harness`/symlink/JSON 均不是定位依据。
Unix registry/store 目录 owner-only；状态落在 workspace 内、registry symlink、预先准备的
审计文件硬链接会拒绝。metadata 与宿主创建的其他挂载别名不由此自动隔离。

`ha migrate-state --from` 显式导入旧 `.harness` 或外部目录备份，永不覆盖已有目标。
先锁定目标，拒绝活动源 run 和本版本仍打开的源 workspace，复制新 inode，SQLite 在线
备份捕获已提交 WAL，检查 schema/完整记录，拒绝 DB 路径标识越界及 trigger/view/virtual table，修复 session registry，重绑 checkpoint
快照并将 target 规范为相对路径，再原子 rename 提交；源数据不修改。非完整的最后一个
事件片段在目标保留为 import-partial-event artifact，完整末条记录补换行，避免恢复终结事件粘连，完整畸形事件/坏 session/越界 checkpoint/symlink 则拒绝导入。
旧 binary/非参与 lease 的写者必须先停止。备份/移动前记录 `ha state` 的路径，停止写者、
复制外部目录和项目，在目标第一次使用前显式导入；Git 不携带审计状态。

shell tmp 单独置于 workspace 的 `.hyper-tmp`，遵守 workspace-write/read-only 模式；
artifact 仅由 harness 将捕获输出写入外部目录，没有把外部 artifact 目录授予 shell 写入。

[状态集成测试](../tests/state.rs) 实测阻止对 events/DB/task/summary/session/checkpoint 的
写入、删除、rename 和 hardlink，且命令确已进入 shell，不靠文本过滤。
另覆盖预制 inode 别名、继承/父进程审计 FD、伪造旧定位信息、8 个并发运行、并发导入仅
一次提交、活动源拒绝、WAL/会话保存、失败回滚、绝对 checkpoint 迁移后恢复、真实请求
replay 完全一致和同会话续聊。原有取消、索引重建、崩溃恢复与 prune 回归也保留。

## Linux 直接工具的路径替换保护（P0-3b2a）

[tool_file.rs](../src/tool_file.rs) 在 Linux 用工作区目录描述符逐层打开目标，
`openat2` 使用 BENEATH、NO_SYMLINKS、NO_MAGICLINKS、NO_XDEV；新目录用 mkdirat
及描述符再次打开，新文件用 O_EXCL，避免缺失检查与创建之间被插入文件/link。
策略先解析已有内部 symlink，再打开规范路径；校验后替换的 symlink 会拒绝。
检查实际已打开 inode 是否为审计硬链接，再读取、快照及修改；写工具不开启 O_TRUNC，
通过校验后才写入。O_NONBLOCK 加 regular-file 检查避免 FIFO 等特殊文件阻塞。
依据 [openat2 官方手册](https://man7.org/linux/man-pages/man2/openat2.2.html)，
NO_XDEV 也拒绝 nested bind mount；本环境无法创建挂载，未宣称实际挂载攻击测试完成。

read/write/edit、search 和自动上下文共享此入口。search 使用 respect-ignore 的原生
文件枚举和固定字符串匹配，每条内容通过安全描述符读取，不再让 rg 子进程重新打开
已校验的路径。内部 symlink 和普通硬链接保持可用，审计硬链接拒绝；未提供 Linux
不支持 openat2 时的降级打开。其他平台仍保留原路径校验，不宣称同等边界。

六个确定性测试覆盖最终 link、父目录 link、打开后路径替换、校验后审计硬链接、
正常内部链接以及无写者 FIFO。打开后替换路径不会重定向读/快照/写，操作绑定原 inode；
这不保证路径名称仍指向该 inode，也不防并发内容写者造成陈旧编辑。
宿主移动整个目录到工作区外、admin restore/undo 的路径竞争与非 Linux 原生保护仍待验收。

## 工具路径与精确命令授权（P0-3b2b1）

显式宿主配置支持 `rules`：read/write/edit 配 literal 相对 `path`，目录尾 `/` 表示子树；
bash 配完整 `command`。不匹配时取工具默认值，匹配规则中 deny > ask > allow；
路径同时按请求拼写与 canonical 目标判定，二者取更严结果。拒绝绝对/上级路径、glob、
未知工具/字段和混合 path/command，库构造的配置也在创建运行前验证。
Linux 路径范围工具在实际打开 inode 后拒绝 nlink > 1，避免普通硬链接越范围；
非 Linux 的 path rules 明确拒绝，不以字符串匹配宣称原生同等边界。

`--approval allow` 只改基础 mutation 值并保留范围 ask/deny；ask 同时收紧范围 allow，
deny 禁止所有 mutation。运行固定完整有效规则和来源，每次 tool.policy 记录匹配索引
与解析目标；审批显示该目标并将其固定给描述符入口，审批期间别名变化不会转移授权。
搜索和自动上下文按 read allow 过滤，ask 不自动弹出审批，显式 read 才逐次申请。
默认无规则与旧配置继续兼容，工具 allow 仍不能突破 plan/read-only、白名单或审计路径。

本地测试覆盖字段/路径校验、范围重叠、路径段边界、read/write/edit 独立判定、精确 shell
及复合命令拒绝、别名和硬链接、审批期间替换、CLI 参数优先级、搜索和实际 context 投影。
范围不限制 bash 内部读写到单个目录，不固定 PATH 可执行程序或仓库脚本内容，也不清除
先前会话中的内容/用户输入；完整 OS 读取边界和历史投影的保密策略需另外定义。

## 已验证的剩余限制与下一项

在一次性 Linux workspace，通过受限 shell 对外部审计 marker 执行 `os.chmod(...,0o400)`，
实测退出 0，权限由 0600 变为 0400，原内容未改写；marker 权限随后由宿主恢复。
因此目前保护的是内容/目录操作，不能宣称抵御所有元数据篡改或拒绝服务。
元数据隔离需另外的 OS 边界；不能用命令字符串规则代替。当前环境的 `unshare -Urnm`
因 uid_map 权限失败，不能把挂载 namespace 当作已可用能力。

P0-3b2a 已交付 Linux 描述符边界，P0-3b2b1 已交付工具范围授权；下一项为 P0-3b2b2：元数据、剩余路径竞争与 OS 范围隔离；显式 unrestricted 仍有
宿主权限。Windows 硬链接及 macOS/Windows 原生受限 shell 继续分平台交付。
仓库 prompt injection、metadata、挂载/预开描述符及源读取边界分别验收。
本轮仅在 Linux 实测，不宣称跨平台原生验证完成。
