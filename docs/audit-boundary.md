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
宿主移动整个目录到工作区外与非 Linux 原生保护仍待验收；Linux restore/undo 的描述符提交已补齐，见下文。

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

## Linux restore/undo 的描述符与原子提交

[restore_file.rs](../src/restore_file.rs) 在读取快照前解析其 parent，并从文件系统根描述符
用 openat2 再打开完整 canonical 路径，拒绝新的 ancestor/final symlink、magic link 和
非 regular file；读取绑定同一源 inode，允许快照存储在不同 mount。
目标 parent 沿工作区描述符用既有 BENEATH/NO_SYMLINKS/NO_XDEV 入口打开，新目录
逐层 mkdirat 后重新打开。操作不重新按完整目标路径找文件。

恢复已有文件时，工作区父目录中的私有 `.hyper-restore-*` 临时目录保存完整副本与快照
permission bits，并 sync 文件，再从该目录描述符 renameat 到目标 parent 描述符。
原子替换最终目录项，不跟随校验后插入的 link，也不改动目标原 inode 的其他硬链接。
依据 [renameat 手册](https://man7.org/linux/man-pages/man2/rename.2.html)，源/目标必须在
同一 mount；不可写的外部 staging parent、挂载点工作区或提交错误均拒绝，不能降级为
工作区内易被同 UID shell 替换名称的 temp。快照本身可跨文件系统，本环境 ext4→tmpfs
实际复制/恢复通过。非 regular/坏源在创建目标目录之前拒绝；失败提交保持原目录项，
已创建的缺失父目录可能留下。普通错误自动清理 stage；进程被强杀可留下外部临时目录。
内容/失败保证实测于本地 ext4/tmpfs，不对网络文件系统或断电后目录项持久性作同等承诺，
也不重建 ACL/xattr/owner。

恢复本次新建文件的 checkpoint 用 parent FD + unlinkat，只删除该目录项；缺失文件/parent
是幂等成功，最终 link 不被跟随，目录不会递归删。`undo` 持有 Workspace lease 直到操作
完成。行政恢复拒绝旧 `.harness` 目标；非 Linux 恢复/独立库快照仍采用旧路径方式。
Linux 独立库 create_checkpoint 已在本轮补齐，见下文，不把它计为前轮恢复交付。

10 个确定性/原生单元测试及 4 个 restore 集成测试覆盖目标/父目录/源父目录替换、打开
parent 后目录改名、删除最终 link、硬链接别名保持、二进制/权限、无目标幂等、失败清理、
跨 mount 快照以及真正受限 shell 对外部 staging 的写/删/移动/硬链接拒绝；CLI restore/undo
实际恢复旧文件和移除新文件。workspace-write 的 metadata 修改仍未被 OS 限制，可导致拒绝服务；宿主整体
移动目录、unrestricted/外部不参与的写者和非 Linux 原生边界仍另列验收。

## Linux 独立库快照与共用提交（本轮已交付）

[checkpoint_file.rs](../src/checkpoint_file.rs) 将 workspace::create_checkpoint 的 root
先 canonicalize，目标经既有路径解析后从工作区目录 FD/openat2 打开；拒绝校验后
parent/final symlink、magic link、跨 mount 与非 regular file，FIFO 非阻塞拒绝。
只有 ENOENT 记为不存在，EACCES/ELOOP 等失败不产生缺失快照；缺失 parent 不创建。
打开的 inode 固定后，路径替换不会把快照改为新路径内容；不保证并发修改该 inode
时得到一致时间点的内容。既有内部 symlink 先解析，普通源 hardlink 仍可用。

独立库与 ToolFile 共用 writer：输出 dir 解析后从 `/` FD/openat2 打开完整 canonical
路径，允许宿主选择跨 mount 存储；新 snapshot/临时清单用 O_EXCL 创建，拒绝同名
文件/link。从既有源 FD rewind/复制，权限取同 FD，sync snapshot 与完整临时 JSON，
最后同输出目录 FD 用 renameat2(RENAME_NOREPLACE) 发布 `.json`，已有清单不覆盖。
[renameat2 手册](https://man7.org/linux/man-pages/man2/rename.2.html) 明确该 flag 需要
文件系统支持，不支持或 syscall 失败均返回错误，不降级为可覆盖的 path 写入。
普通错误通过同 dir FD 尽力 unlink 本次新建条目，不删除既有冲突；新建输出目录
可能留下。成功只留下完整 snapshot/JSON，JSON mode 0600，返回的 snapshot_path
为 canonical 绝对路径，兼容相对 root/dir，不依赖后续 cwd。

六个确定性单元验证 source final/parent 替换、打开后名称替换及 rewind、output 解析
后替换/固定 FD、snapshot/临时 JSON/最终 JSON 冲突保持与复制失败回滚。四个 API 集成
验证 binary/mode/相对路径/内部 link 的完整创建恢复、缺失 parent/幂等恢复、越界/
dangling/目录/FIFO/权限拒绝与无输出副作用，跨 ext4/tmpfs 存储创建和恢复。权限样本
只在非 root 用户运行，跨 mount 样本在无可写独立 `/dev/shm` 时跳过；本环境两者均执行。

该 API 是宿主行政读取，不应用模型工具权限/审批或审计 hardlink 过滤。输出目录须由
宿主选择且受信任；任意 workspace 内可写 dir 不因采用该 API 变为权威存储。打开前
存在的输出 symlink 可以被 canonicalize 成宿主实际选择目录；拒绝的是解析后替换。
宿主整体移动 root/state dir 可使返回路径陈旧；其他不参与的 writer、同 inode 并发修改
及 workspace-write 的 metadata 可用性风险另列边界。SIGKILL 可能留下未发布快照/
临时 JSON；未 sync 输出目录，不承诺断电持久性或网络文件系统失败语义，未复制
ACL/xattr/owner。原生测试仅 Linux/aarch64 的本地 ext4/tmpfs；非 Linux 保留旧实现。

## Linux read-only 显式元数据修改限制（本轮已交付）

read-only shell 在 Landlock 后应用 seccomp BPF，拒绝 chmod/chown、时间戳、xattr
显式修改的 path/fd/at 接口，含 fchmodat2、setxattrat/removexattrat 和 file_setattr。
调用返回 EPERM，任意路径、预打开 FD 和链接别名均同样拒绝。ioctl 和 io_uring
接口也整体拒绝，避免文件属性 ioctl 或异步 xattr 绕过；ptrace/process_vm_writev
拒绝。该过滤继承到线程、fork 与 exec，不在命令字符串上匹配路径。

[内核 seccomp 文档](https://docs.kernel.org/userspace-api/seccomp_filter.html) 要求检查
architecture，且经典 BPF 不解引用用户路径；据此本次按操作整体拒绝，而不是猜测路径。
[内核 syscall 表](https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h)
确认新增接口编号。过滤先检查原生 audit arch，拒绝 compat ABI；x86-64 另拒绝 x32
编号。不支持的 read-only 架构与过滤安装失败均拒绝启动，无静默降级。实现支持 Linux
64 位 x86-64 与小端 aarch64；本地原生验证只有 aarch64，x86-64 分支仅做静态审查与
BPF 判定测试，不宣称已完成 x86 原生验证。

两个 BPF 判定单元与四个原生集成覆盖：工作区/外部/审计 marker、路径/fd、symlink/
普通硬链接、已有 xattr 保持、线程/exec 继承、预打开 FD、现代 raw syscall 和 ioctl/
io_uring 路线拒绝；读取可用，workspace-write/unrestricted 内的 chmod/utime 兼容。
原生测试通过真实 engine 运行，确认工具启动与执行模式事件，不把命令策略拒绝当
内核防护。库调用者应使用 Sandbox::apply_prepared_in_child；既有低层
apply_in_child(ruleset_fd) 只应用 Landlock，保留兼容，不含此过滤。

该交付只覆盖 read-only 的这些显式修改接口。ioctl、异步 I/O 或调试类工具在该模式
内会受限；读取导致的 atime、锁/租约及未参与边界的宿主进程
不由此隔离，也不据 denylist 宣称覆盖未来所有新 syscall。workspace-write 的路径级
metadata 隔离继续待办，需要允许仓库内构建操作而保护外部审计元数据。

## Linux 受限 shell 显式 socket 限制

workspace-write 与 read-only 在 Landlock 后安装共用 seccomp BPF，禁止 socket、
connect/bind/listen/accept、带地址的 sendto/recvfrom、sendmsg/recvmsg 及批量 syscall、shutdown、
setsockopt，返回 EPERM。socket 创建拒绝，不按路径/域猜测外部服务，因此 UDP、
路径/抽象 Unix socket、IPv6、netlink 与其他 socket 家族都不可新建，工作区内也同样
拒绝。仅 AF_UNIX 匿名 socketpair 与地址参数为完整 64 位 null 的 sendto/recvfrom
允许，供 Rust 子进程启动握手使用，不创建命名/外部连接；BPF 同时检查指针高低
32 位，不解引用用户地址。sendmsg/recvmsg（含 SCM_RIGHTS）与 connect 仍拒绝。
io_uring 的 setup/enter/register 拒绝，防止异步 socket 操作绕过；pidfd_getfd、
ptrace 与 process_vm_writev 拒绝，防止导入外部描述符或篡改宿主进程代为执行。

[内核 seccomp 文档](https://docs.kernel.org/userspace-api/seccomp_filter.html)
说明按 syscall/arch 过滤及线程/子进程继承；仍先核对原生 audit arch，拒绝 compat/x32。
不支持的架构或安装失败不启动受限 shell，无静默回退；两种受限模式现在均要求
Linux 原生 64 位 x86-64 或小端 aarch64。unrestricted 不安装此过滤，宿主 provider
请求不在 shell 子进程内，保持可用。

本地 Linux/aarch64 新增一个 BPF 单元与五个原生集成，覆盖两模式、线程/exec 继承、
UDP/Unix 预打开 FD 的带地址/消息式收发拒绝、raw syscall 的 EPERM、管道/普通文件兼容；
宿主接收端确认没有收到字节。unrestricted 在真实 UDP、路径与抽象 Unix 端点正向
送达 marker。原只读 metadata 四项测试仍通过，workspace-write 的合法 chmod/utime
保持兼容。新增 Rust 编译及成功/失败子进程启动回归验证匿名通信；read-only 只验证
status 启动，Rust output 的 ioctl/非阻塞管道操作仍受既有 ioctl 限制。
完整本地 215 个 Rust 测试、14 个离线评测、fmt/Clippy/release 通过；
实现 `b8464f9` 的 [CI](https://github.com/ZeroMarker/hyper/actions/runs/37218002402) 全部通过，
包含 Linux/x86-64 的五个原生 socket/编译集成与 Windows/x64 六项硬链接回归。
[固定模型回归](../evals/baselines/2026-10-04-sockets/report.md) 30/30 通过，45 轮用量
及持久化一致、30 次 prune 均验证；首轮严格过滤的 27/30 结果独立保留，不替换。

这会限制依赖命名 Unix socket 的编译缓存/服务与本地网络测试，
不提供工作区内命名 socket 例外；需要此能力的任务须由宿主显式选 unrestricted。
此交付是显式 socket syscall 边界，不是完整网络 namespace 或宿主描述符隔离。
若库调用者主动继承已连接 socket，普通 read/write/readv/writev/sendfile/splice
及无地址 send/recv 仍可传输字节；宿主预先建立的共享映射/异步队列也不由该过滤自动撤销。调用者须控制
继承资源并使用 apply_prepared_in_child；既有 apply_in_child 仅应用 Landlock。
workspace-write 的路径级 metadata、宿主并发修改、外部读取与其他平台仍待办。
不据 syscall 禁止列表承诺未来所有新接口，完整 P0-3 继续未完成。

## Windows 直接文件工具硬链接检查

Windows 路径解析及 ToolFile 实际打开后，分别通过
[GetFileInformationByHandle](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle)
读取句柄的 nNumberOfLinks；不是使用不稳定的 Rust MetadataExt 接口，也不以
路径拼写判定 alias。计数不为 1 或查询失败即拒绝，read/write/edit、search 与
自动上下文共用此入口；审计存储的 shell 启动前检查也采用同一规则。
这是保守的所有硬链接拒绝：普通仓库硬链接同样不可用，删除别名恢复单链接后可用。
Windows 不再仅因非 Unix 而跳过硬链接检查。

新增三个 Windows 句柄/边界单元与三个工具集成测试，覆盖打开后原路径替换、
解析后新增 alias、外部与旧审计 marker、read/write/edit 的显式 allow 拒绝、
search 内容过滤、普通硬链接拒绝及单链接文件兼容。CI 增加 Windows 原生 job；
本地 Linux 209 个 Rust 测试、14 个离线评测、fmt/Clippy/release 均通过。
实现提交 `9c801d9` 的 [CI](https://github.com/ZeroMarker/hyper/actions/runs/37215937962)
全部成功，其中 Windows 原生 x64 的上述 3 单元 + 3 集成均通过；不推断其他
Windows 架构、网络文件系统、shell 或全部平台测试已验证。

本次不改变状态迁移的新 inode 复制语义，不将源硬链接变成迁移拒绝条件。
没有增加 Windows 受限 shell、目录描述符或恢复/独立快照保护，行政 API 保持原边界。
不保证检查后宿主并发新增 alias、同 inode 修改、reparse point 或目录移动的防护；
支持边界是成功查询到真实计数的文件系统，不据本地 Linux 回归宣称 Windows 原生通过。

## 已验证的剩余限制与下一项

前轮在一次性 Linux workspace，通过 workspace-write shell 对外部审计 marker 执行 `os.chmod(...,0o400)`，
实测退出 0，权限由 0600 变为 0400，原内容未改写；marker 权限随后由宿主恢复。
因此 workspace-write 目前保护的是内容/目录操作，不能宣称抵御所有元数据篡改或拒绝服务；本轮 read-only 的显式修改限制不解决这个默认模式风险。
元数据隔离需另外的 OS 边界；不能用命令字符串规则代替。当前环境的 `unshare -Urnm`
因 uid_map 权限失败，不能把挂载 namespace 当作已可用能力。

P0-3b2a 已交付 Linux 描述符边界，P0-3b2b1 已交付工具范围授权；下一项为 P0-3b2b2：元数据、剩余路径竞争与 OS 范围隔离；显式 unrestricted 仍有
宿主权限。Windows 直接工具已补保守硬链接检查，macOS/Windows 原生受限 shell 继续分平台交付。
仓库 prompt injection、metadata、挂载/预开描述符及源读取边界分别验收。
Linux 整套回归与 Windows x64 六项针对性测试已验证；其余平台及完整隔离未验证。
