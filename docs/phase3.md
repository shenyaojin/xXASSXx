# 第三阶段：持久协作与任务文件

第三阶段已于 2026-10-06 完成用户选定的“实验数据质控”真实双机验收，真实运行和控制端独立检查均通过。实现状态与真实证据见 [验证记录](phase3-validation.md)；第二阶段记录保留为历史。当前通信仍依赖 Mac/controller 的 SSH 隧道，公网常驻部署属于第四阶段。

## 状态与调度

SQLite 从 schema 1/2 增量迁移至 3，保留旧任务、版本、消息和会话。一个 workflow 绑定一个本地 task、一个 Codex 会话、一个联系人、一个 conversation 和不可变文件授权；每个收到的消息产生唯一的持久事件。`runs` 保存每次唤醒使用的明确 session ID，`workflow_runs` 关联事件与执行，`workflow_models` 保存协调工具调用，`workflow_file_reads` 保存正文读取凭据。

`task.succeeded` 表示一次 Codex 执行通过验收。协作状态 `waiting` 表示等待绑定同伴；只有 `workflow.completed` 才表示整个本地职责已完成。B 的职责完成后发出结果，A 收到结果并结合本地材料提交最终结果后，A 才完成。

| 状态 | 含义 / 后续操作 |
| --- | --- |
| prepared | 本地主人已经授权，等待指定同伴的首次请求 |
| ready / running | 有持久事件待处理 / 当前执行中 |
| waiting | 本轮成功，等待同伴，无空转 Codex 进程 |
| completed | 有通过程序验收的最终 JSON 及来源引用 |
| failed | 模型或 Codex 失败；查看证据后显式 retry |
| needs_attention | 缺失/失效会话或执行结果不确定，需要人工查看 |
| limit_reached / timed_out | 调用预算或任务期限耗尽，不自动重试 |
| stopped | 主人停止或关闭会话，拒绝后续唤醒和提交 |

HTTP 收件先保存原始消息，再建立事件，最后 ACK；重启会扫描遗漏事件。重复消息、重复扫描、重复提交使用稳定 ID 和 SQLite 事务去重。同一 workflow 的调度锁、task 执行锁和保存会话的锁防止并发执行；忙时保留事件。无法判定远端执行结果时记为 needs_attention，不承诺分布式 exactly-once。

Codex 的 `submit_collaboration_turn` 只暂存本轮提案。Rust 必须确认 MCP 初始化、任务读取、有效提交、正确会话、`turn.completed` 和进程正常退出，才在事务内写出消息或完成工作流。模型仅说“完成”或提交后进程失败，均不能产生有效业务结果。

每个 workflow 首次执行可调用协调模型的 `inspect_workflow` 和 `dispatch_codex` 工具。正文处理交给 Codex；后续回信由 Rust 事件调度直接恢复，不为 ACK、查询、重复消息和空闲轮询调用模型。

## 文件授权与边界

授权格式是可重复的 `--grant VERSION_ID:relative/path`，精确到本地对象固定版本的一个文件。`object publish --text` 的路径是 `record.txt`；目录快照使用版本清单中的相对路径。授权后发布新版不改变旧任务。`shared` 只控制原有元数据共享，不授予正文读取。

任务 MCP 提供 `get_task`、`list_task_files`、`read_task_file`、`submit_collaboration_turn`。读取必须先 get_task，匹配有效 task/run/token、授权版本和相对路径，再检查不可变 blob 哈希、长度、UTF-8、符号链接和读取预算。支持 offset/max_bytes 分段，偏移为 UTF-8 字节。暂不解析 PDF/Office 或一般二进制；知识库存储能力不受影响。

两端 workflow 都只获得任务 MCP，不获得主人完整管家 MCP。同伴派生任务只能向绑定联系人和会话发问或回信，不能扩大文件授权、访问第三人或浏览其他对象。来源引用包含 member/object_id/version_id/path/sha256，本地引用必须有实际读取凭据，远端引用必须出现在收到的业务消息中。

Codex 使用空的固定工作目录、read-only、禁止 shell/exec/web/多 Agent 工具，并忽略用户工具配置和规则；继续使用官方登录，不读取登录凭据。管家模型密钥从其子进程环境移除。这是应用工具权限约束，不是完整的操作系统隔离；同一 OS 用户有能力修改自身数据库、文件或进程，不能以此隔离恶意本机用户。获准读取的正文会进入 Codex 云模型上下文，真实演示使用专门测试材料。

## 最小操作

先按 README 配置独立 A/B 成员、联系人、信箱和 executor；每端 workdir 必须存在。B 的主人先授权材料和处理目的，把返回的 workflow ID 告知 A。以下变量代表各自本地版本，不复制数据库或文件到另一端。

```sh
# B 端
xxassxx --db b.sqlite3 collaboration prepare --peer a \
  --goal '依据获准材料回答 A；信息不足时提出澄清' \
  --grant "$B_VERSION:record.txt"

# A 端，REMOTE_WORKFLOW 使用 B 返回的 ID
xxassxx --db a.sqlite3 collaboration start --peer b \
  --remote-workflow "$REMOTE_WORKFLOW" \
  --goal - --grant "$A_VERSION:record.txt" < task.txt

# 两端分别启动：后台运行，日志持久保存；不会安装系统服务
xxassxx --db a.sqlite3 service start --poll-secs 2 --max-failures 3
xxassxx --db a.sqlite3 service status
xxassxx --db a.sqlite3 service logs --lines 100
xxassxx --db a.sqlite3 collaboration list
xxassxx --db a.sqlite3 collaboration show "$WORKFLOW"
xxassxx --db a.sqlite3 show "$TASK" --events
xxassxx --db a.sqlite3 collaboration result "$WORKFLOW"
```

`collaboration show` 返回 peer（等待对象）、原始 messages、events、active_event、task.active_run、每次执行与 session ID、精确授权、读取凭据、协调模型记录和产物路径。完成的 JSON 保存在 `DB.results/WORKFLOW_ID.json`；文件可从数据库恢复，已存在且内容不一致时拒绝覆盖。

```sh
xxassxx --db a.sqlite3 collaboration retry "$WORKFLOW"  # 查看失败原因后显式重试，保持保存的 ID
xxassxx --db a.sqlite3 collaboration stop "$WORKFLOW"   # 拒绝后续唤醒和在途提交
xxassxx --db a.sqlite3 service stop --wait-secs 30
```

服务 stop 写入当前实例的停止请求，不对陈旧 PID 盲目发信号；当前 tick 在网络/模型/执行超时边界结束后退出。等待截止可能返回 `stop_requested=true, alive=true`，应继续查看状态。需要先取消业务时先 collaboration stop。foreground `butler run` 仍支持 Ctrl-C，保留同步期间停止信号的回归覆盖。服务重启从持久事件继续；意外死亡的执行需检查并显式 retry，不静默重跑不确定的业务。

`collaboration run ID` 仅处理一个待办事件，供诊断；完整协作使用常驻服务。`collaboration local` 可验证单端授权文件读写，不能给它 peer 或联系同伴。

## 预算、密钥与失败

`--limits limits.json` 可覆盖默认值：6 次 Codex 唤醒、8 次管家模型调用、6 条发信、1200 秒任务期限、累计 256 KiB 正文读取、每轮 64 KiB、每块 8192 字节、单次输出 8192 字节。所有字段都有硬上限。executor 的 timeout_secs 独立限制每轮 Codex；model 的 timeout_secs/max_model_calls/max_tool_rounds/max_tool_calls 限制协调调用。

服务遇通信故障最多连续失败 max_failures 次，带有限退避；鉴权/凭据错误直接阻断。workflow 模型失败、额度错误、无效响应和 Codex 失败保留明确状态，不自动无限重试。重试仍消耗原预算，不清除原会话 ID；无法恢复时需要人工处理。

在 member TOML 中配置绝对路径：

```toml
[model]
provider = "deepseek"
model = "deepseek-flash" # 此名称以实际测试服务为准，可替换兼容模型
api_key_env = "DEEPSEEK_API_KEY"
secrets_file = "/home/USER/.config/xxassxx/secrets.env"
timeout_secs = 60
max_model_calls = 4
max_tool_rounds = 3
```

启动程序按数据读取指定字段，要求普通文件、当前用户所有、权限 600、不跟随文件符号链接，不 source，不复制/打印密钥。也支持既有 api_key_env 环境变量；不要把真实值放入 TOML、消息或日志。

## 验证入口

```sh
cargo test --locked  # 默认模拟模型；真实测试仍为显式 opt-in
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/ssh_workflow.py --help
```

`ssh_workflow.py` 不指定 `--case` 时只运行两端独立的真实技术检查；指定用户已审核的 JSON manifest 后，由两个 Rust 服务自动推进协作。开发用合成案例应加 `--technical`，报告明确标为技术检查。manifest 结构为 title、a/b（各含 file、text、goal），不是可执行脚本。脚本不逐轮调用 Codex、不代填回复、不改数据库，只通过 CLI 准备、提交、观察和验收，模型有调用/时长预算。每次创建独立目录，收集实际 MCP/完成事件、会话 ID、原始消息和最终 JSON，并停止自己的测试服务和隧道。

当前脚本依赖 Mac/controller 持续维持 SSH 隧道；状态 report 明确记录这一点。信箱只监听 loopback，不公开无 TLS 的业务端口。二进制需先在 Linux 编译；Lakota 既有隔离工具链路径和本轮部署记录见验证文档。
