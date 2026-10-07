> 第一阶段操作参考。第二阶段功能与当前验证状态见 [README](../README.md) 和 [第二阶段验证记录](phase2-validation.md)。

# xXASSXx · 第一阶段

Rust CLI → 官方 Codex CLI → 本地 stdio MCP → SQLite 的双向接入闭环。
任务、执行记录、有效结果、Codex 会话 ID 和结构化事件均持久化。
本阶段仅验证接入，不包含科研对象、团队协作或后台调度器。

## 运行

需要 macOS/Linux、Rust 工具链和已登录的官方 Codex CLI。
模拟测试另需 Python 3.11+。本次验证环境见 [测试报告](validation.md)。

```sh
cargo build --locked
codex --version
codex login status
# 仅未登录时，由用户通过官方命令登录：codex login

./target/debug/xxassxx init
./target/debug/xxassxx submit '计算 17 × 19，并提交简短结果'
./target/debug/xxassxx list

# TASK_ID 替换为 submit 返回的 id；工作目录必须存在。
./target/debug/xxassxx run-once TASK_ID --workdir "$PWD" --timeout-secs 180
./target/debug/xxassxx show TASK_ID --events

# 后续执行只允许恢复数据库保存的同一个会话。
./target/debug/xxassxx run-once TASK_ID --workdir "$PWD" --resume
```

`submit` 只入队，`run-once` 是本阶段显式触发一次执行的入口。
任务可通过标准输入提交，避免 shell 对任务中的引号、换行和特殊字符做解释：

```sh
./target/debug/xxassxx submit - <<'TASK'
计算 17 × 19，并提交简短结果。
这里的 $(...)、反引号、分号均为普通任务文本。
TASK
```

所有命令支持 `--db /absolute/path/tasks.sqlite3`，默认 `.xxassxx/tasks.sqlite3`。
`run-once` 支持 `--codex /path/to/codex`、`--model MODEL`。
JSON 输出写到 stdout，进度和诊断写到 stderr；执行未验收成功时返回非零退出码。
`init` 可重复运行，不会清空数据。

## 成功标准与恢复

只有以下条件**同时**成立才将任务置为 `succeeded`：

1. 本次执行的 MCP 完成初始化。
2. Codex 通过 `get_task` 读取了本次任务。
3. `submit_task_result` 验证身份、运行状态和结果格式后，在 SQLite 事务中接收结果。
4. 捕获有效的 `thread.started.thread_id`，恢复时该 ID 必须与原会话相同。
5. 捕获 `turn.completed`，没有终止性 `turn.failed`，且 Codex 退出码为 0。

模型的自然语言回复不参与成功判断。收到结果但 Codex 随后失败，任务仍为失败；候选结果保留在对应执行记录里，任务的当前有效结果为 `null`。

`tasks` 保存当前状态；`runs` 保存每次执行和诊断；`events` 保存 JSON 事件。
执行状态为 `running → succeeded / failed / timed_out`，任务初始为 `pending`。
SQLite 使用 WAL、外键、立即事务及“每个任务最多一个运行中执行”的唯一索引。

一旦获得会话 ID，就立即持久化。之后必须使用 `--resume`；适配器仅调用 `codex exec resume SAVED_ID`，绝不使用 `--last` 或静默新建会话。
成功任务也允许 `--resume`：创建新的执行记录，重新通过 MCP 读取与提交，保留旧记录。
尚未得到会话 ID 的失败允许再次普通执行。

`run-once` 由独立的监护进程执行任务，CLI 与监护进程之间保留一条存活管道。
超时会终止 Codex 及其同进程组的 MCP 子进程；Ctrl-C 或 CLI 被 SIGKILL 时，监护进程通过信号或管道关闭发现中断，清理子进程并记录 `interrupted`。启动前的版本/登录检查也受监护。
每个任务还有一把操作系统文件锁，位于数据库旁的 `<数据库文件名>.run-locks/`；监护进程退出时自动释放，不应手动删除锁文件。
只要旧执行仍持锁，即使数据库 `deadline` 已到期也会拒绝启动新的执行；清理完成后可立即重试，有会话 ID 时使用 `--resume`。
若整机崩溃导致状态未能落盘，重启后等待 `deadline` 即可回收旧执行，并记为 `lease_expired`。
原 MCP 执行授权失效，迟到结果不能覆盖新执行。秒精度的数据库租约比实际总超时最多宽限 1 秒。

## MCP 接口

适配器在单次 Codex 启动参数中注册 **required** 的 `xxassxx` stdio 服务，无需改动用户全局配置。
MCP 服务与普通 CLI 共用 `store.rs` 的业务逻辑。

| 工具 | 参数 | 行为 |
| --- | --- | --- |
| `get_task` | `task_id` | 只读取服务绑定的任务；返回 `task_id`、当前 `run_id` 和 `input` |
| `submit_task_result` | `task_id`、`run_id`、`idempotency_key`、`result` | 验证当前执行、此前读取回执及非空结果，然后提交 |

同一执行内，完全相同的 key 和 result 可幂等重试（成功后、被下一次执行替代前也可重试）；不同 key 或内容都拒绝覆盖。
任务和结果各最多 256 KiB，key 最多 128 字节；单个协议帧最多 2 MiB，Codex 事件流最多 16 MiB / 10000 条。

服务启动形式如下，由适配器自动填入数据库、任务、执行及授权环境变量：

```sh
XXASSXX_RUN_TOKEN=RUN_CAPABILITY ./target/debug/xxassxx \
  --db /absolute/path/tasks.sqlite3 \
  mcp-serve --task-id TASK_ID --run-id RUN_ID
```

这里的 `RUN_CAPABILITY` 是执行器生成的临时授权值，**不是 Codex 登录凭据**。数据库仅保存其 SHA-256 摘要。
它绑定单一 task/run，通过进程环境传递，不出现在模型提示或持久事件里。
普通用户无需手动启动或生成此授权。服务只实现所需的 stdio MCP 子集：初始化、ping、工具发现与调用；stdout 只输出逐行 JSON-RPC。

## 工作目录、权限与登录

`--workdir` 必填并解析成绝对路径，Codex 及 MCP 子进程均使用该目录。
执行使用 `sandbox_mode="read-only"`、`approval_policy="never"`；关闭 shell、unified exec、多代理和网页搜索。
本阶段只开放任务 MCP 数据操作，不授予模型修改项目文件的权限。
MCP 服务是受信任的宿主进程，其 SQLite 写入由业务校验约束；Codex 的只读沙箱不会自动约束 MCP 宿主能力。

适配器通过 `std::process`/Tokio 的参数数组启动官方 CLI，固定控制提示由 stdin 传递。
任务正文仅经 MCP 返回，绝不拼接为 shell 命令或 CLI 参数。
使用 `--ignore-user-config` 和 `--ignore-rules` 使本次自动执行的设置可预测；仍保留官方 `HOME`/`CODEX_HOME` 登录与会话存储。
不会读取或复制凭据文件，不创建 API key；移除子进程的 `OPENAI_API_KEY`/`CODEX_API_KEY` 覆盖，并让官方 `codex login status` 诊断登录状态。
宿主/企业强制策略仍可能使执行被拒绝。

## 诊断

`show TASK_ID` 的 `runs[].error_code/error_message` 提供持久诊断。

| 错误码 | 含义与处理 |
| --- | --- |
| `not_installed` | 找不到 Codex；安装官方 CLI 或指定 `--codex` |
| `not_logged_in` | 官方 CLI 报登录不可用；执行 `codex login` |
| `quota_limited` | 官方输出表明额度或速率限制；根据官方提示等待或处理额度 |
| `timeout` | 版本/登录检查或实际执行超过总时限；检查后按保存的 ID 恢复 |
| `process_failed` | 非零退出或终止性失败事件；保留退出码及诊断 |
| `mcp_initialization_failed` | 必需 MCP 启动失败，或没有初始化回执；检查当前可执行文件、数据库和目录 |
| `missing_result` | 未通过 MCP 读取并提交有效结果 |
| `incomplete_execution` | 缺少会话事件或完成事件 |
| `protocol_error` | JSON 格式错误、事件超限或恢复返回了不同会话 ID |
| `interrupted` / `lease_expired` | 用户中断 / 运行者消失后租约到期 |
| `storage_error` | 持久化失败；检查磁盘和 SQLite 文件权限 |

额度与登录错误来自官方 CLI 的结构化错误和 stderr 文本，没有稳定的统一错误码保证；未识别文本归为 `process_failed`，不会猜测为“额度不足”。

## 测试

```sh
# 不访问模型；真实测试明确显示 ignored。
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings

# 显式运行真实测试：使用现有登录与实际额度，两轮调用。
cargo test --locked --test real_codex -- --ignored --nocapture
# 可选：XXASSXX_REAL_CODEX=/path/to/codex XXASSXX_REAL_MODEL=MODEL
```

模拟进程会启动真正的 Rust MCP 子进程，并经 stdio 获取/提交任务。
真实冒烟测试同时核对数据库回执和 `mcp_tool_call` 完成事件，再按已保存 ID 恢复并重复验证。
证据保存在 `smoke-output/<UUID>/`，包括 SQLite、两轮事件和 `report.json`；失败也会保存报告，执行中断时保留 `running`，不会误报通过。
这些包含本地任务内容的文件默认被 Git 忽略。

## 范围与限制

- 已真实验证官方 `codex-cli 0.144.5` 和 `0.160.1`；不同版本的参数/事件变化需重新跑真实冒烟测试。仅测试了本机 macOS；Linux 采用同样的 Unix 进程组实现，Windows 暂不支持执行器。
- 本地单用户信任模型；执行授权防止任务串写和迟到提交，不能抵御可直接修改 SQLite 或读取同用户进程环境的恶意宿主程序。请使用可信工作目录。
- 进程监护覆盖用户启动的 `run-once` 被结束的情况；监护进程本身也被单独强杀、或子进程主动脱离进程组的情况不提供清理保证。
- “有效结果”指格式、任务归属和执行完整性，未验证答案的语义正确性。
- 没有后台队列监听、自动重试/退避、多机分发、科研对象或团队协作。一次命令执行一个指定任务。
- 会话恢复依赖官方 CLI 自己持久化的会话文件；会话已被删除时会明确失败，不另建会话冒充恢复。
- 两个 MCP 工具可以串行处理；未实现 HTTP、资源/提示词、长任务取消或通用 MCP SDK 的完整功能。

实现依据：[官方非交互模式](https://learn.chatgpt.com/docs/non-interactive-mode)、[官方 MCP 配置](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)、[官方配置参考](https://learn.chatgpt.com/docs/config-file/config-reference)、[MCP stdio 规范](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)。实际参数同时以本机 `codex exec --help` / `codex exec resume --help` 核实。
