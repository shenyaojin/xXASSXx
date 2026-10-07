> 本文件保留第一阶段历史证据；第二阶段和当前完整测试结果见 [第二阶段验证记录](phase2-validation.md)。

# 第一阶段验证记录

验证日期：2026-10-06（America/Denver）。

## 结论

**当前版本的 28 项本地测试通过；真实接入证据来自下述进程监护修复之前的版本。** 两者独立运行和记录；普通 `cargo test` 会明确把真实测试标记为 `ignored`，不能据此宣称真实接入成功。

## 进程监护修复复验

修复后复跑 `cargo test --locked --offline`：28 项通过，0 失败，真实模型测试仍为 `ignored`。这次没有重新调用真实模型，也没有修改登录方式。
本机 macOS 上的 `cargo clippy --locked --offline --all-targets -- -D warnings`、`cargo fmt --all -- --check` 和 `git diff --check` 也通过。

新增 4 项真实进程生命周期回归（Codex 使用模拟进程，MCP 使用实际 Rust 程序）：

- 强杀外层 CLI，确认 Codex 与 MCP 进程停止、任务记录中断，再按原会话 ID 恢复成功。
- 在 Codex 版本预检期间强杀 CLI，确认预检子进程停止，任务可重试。
- 人为提前设置数据库租约到期，确认仍存活的执行持有文件锁，新执行被拒绝；清理后恢复成功。
- 发送 Ctrl-C，确认子进程停止并记录 `interrupted`。

原有超时测试也增加了对子进程确实停止的检查。检查使用进程存活状态，不依赖数据库中的成功标记。

## 修复前的接入验证

| 检查 | 实际结果 |
| --- | --- |
| `cargo test --locked --offline` | 当时 24 个模拟/业务/协议测试通过，0 失败；1 个真实测试默认忽略 |
| `cargo test --locked --offline --test real_codex -- --ignored --nocapture` | 真实测试显式运行通过，包含新建会话与按保存 ID 恢复两轮 |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | 通过，无警告 |
| `cargo fmt --all -- --check` | 通过 |
| `git diff --check` | 通过 |

## 真实证据

最终测试由 CLI 执行初始化与提交，然后两次调用 `run-once`（第二次带 `--resume`）。

- 官方 Codex：`codex-cli 0.160.1`，由每轮启动前的官方 `--version` 输出保存。
- 登录：使用本机现有官方登录；预检 `codex login status`。未读取或复制任何登录凭据。
- 任务 ID：`f1fd5398-f870-4780-b34d-87e2b9b3daaf`。
- 持久会话 ID：`01a112df-5979-7883-a40f-ba948a77d2d2`。
- 首次执行：`297bfa3e-a4cf-4e2b-b27f-8761547b61f2`。
- 按 ID 恢复执行：`4e5612a0-c878-4c3d-b19d-c946c37fc9e8`。
- 两轮都观测到 `item.completed` / `mcp_tool_call`，server 为 `xxassxx`，工具分别为 `get_task`、`submit_task_result`。
- 两轮数据库均记录 MCP 初始化成功、1 次读取、1 次有效提交、`turn_completed=true`、`exit_code=0`、`state=succeeded`。
- 两轮结果均为 `XXASSXX_REAL_SMOKE_OK`；恢复记录的 `resumed_session_id` 与两轮 `session_id` 完全一致。
- 最终测试耗时 24.10 秒。

可随仓库保存的精简证据：[real-smoke-evidence.json](real-smoke-evidence.json)。
本机完整证据：[report.json](../smoke-output/6fe6d59e-9845-4ced-9c40-7ca07ef4f6ab/report.json)、[首次事件](../smoke-output/6fe6d59e-9845-4ced-9c40-7ca07ef4f6ab/initial.events.json)、[恢复事件](../smoke-output/6fe6d59e-9845-4ced-9c40-7ca07ef4f6ab/resume.events.json)。
同目录的 `tasks.sqlite3` 和两轮 stdout/stderr 也保留；完整运行目录被 Git 忽略，不会随克隆传播。

更早的一轮真实测试在 `codex-cli 0.144.5` 上也通过，耗时 22.93 秒。
该轮会话 ID 为 `01a112db-3ebc-71d2-8bca-6c57c8bc0959`，证据位于 `smoke-output/befddc34-adf1-4cc5-9827-f8dcb082b8f7/`。
测试期间本机 CLI 版本发生变化；本项目没有执行 Codex 安装或升级操作，不推断其变化原因。两个版本的结果分别以持久执行记录为准。

## 模拟覆盖

模拟进程实际启动 Rust MCP 子进程，以逐行 JSON-RPC 调用工具，而不是直接写入成功标记。

- 成功与进程重启后的按 ID 恢复；含空格的可执行文件和数据库路径。
- 未安装、未登录、额度限制、进程非零退出、MCP 初始化失败。
- 执行超时、预检超时、超时后的会话恢复、崩溃租约回收。
- 重复提交、内容/key 冲突、成功后原提交重试。
- 错误 task/run 身份、错误授权、旧执行、未读取任务先提交。
- 只输出“完成了”、缺少结果、缺少完成事件、提交后仍失败。
- 损坏的事件、未知事件兼容、大量 stderr 排空。
- 并发领取只允许一个胜者；终止性失败不能被后续完成事件覆盖，临时重连可恢复。
- MCP 初始化前拒绝工具请求、协议版本回退、JSON 解析错误、未知方法、参数校验。
- 含 shell 元字符的任务保持数据形式，不进入模型启动提示或 shell。

## 环境与复现

- 平台：macOS / Darwin arm64。
- Rust：`rustc 1.99.0 (b940084d7 2026-09-28)`；依赖固定在 `Cargo.lock`。
- 模拟进程：Python `3.14.8`；所需最低版本为 3.11（使用标准库 `tomllib`）。
- 初始没有 Rust；本次仅把隔离工具链装入 `/private/tmp/xxassxx-cargo`、`/private/tmp/xxassxx-rustup`，未修改用户 PATH 或 shell 配置。

已构建的 CLI 可直接用 `./target/debug/xxassxx`。在本机临时工具链尚未被系统清理时，也可这样复现：

```sh
export CARGO_HOME=/private/tmp/xxassxx-cargo
export RUSTUP_HOME=/private/tmp/xxassxx-rustup
export PATH="/private/tmp/xxassxx-cargo/bin:$PATH"
cargo test --locked --offline
cargo test --locked --offline --test real_codex -- --ignored --nocapture
```

长期开发请使用自行安装的 Rust 工具链，再按 [README](../README.md) 运行。真实测试需要网络与现有官方登录，并消耗实际额度。

## 未验证与剩余限制

- 未在 Linux、Windows、其他 Codex 版本或企业强制策略环境实测。
- 真实测试覆盖健康登录下的成功与恢复；未通过破坏真实登录、耗尽真实额度来验证异常。异常路径的结论来自模拟测试。
- 未验证长期任务的模型质量；成功标准只证明任务身份、有效持久结果和执行完整性。
- 本阶段没有科研对象、团队协作、常驻调度器或自动重试。手动 `run-once` 触发任务事件。
- 官方错误输出可能变化，未识别错误保留为 `process_failed`；官方会话文件删除后无法恢复。
- SQLite 和 MCP 属于本地单用户信任域，不提供跨用户/恶意本机进程隔离。
