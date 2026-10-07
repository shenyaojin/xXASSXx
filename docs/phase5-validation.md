# Phase 5 验证记录

验证日期：2026-10-06（America/Denver）；真实闭环 UTC 时间为 2026-10-07 02:41:07–02:42:47。后续真实 TUI 两轮对话与后台继续执行检查在同一晚完成。

**结论：80 项默认自动测试通过；实际 PTY 通过；本机两个隔离个人端的真实 DeepSeek + 官方 Codex 闭环通过。** 两个旧阶段 opt-in 真实 Rust 测试仍被默认测试标为 ignored；本阶段真实验收使用专门的 Phase 5 脚本，不能把 ignored 记作通过。

## 环境与构建

- 本机 macOS 26.6.2 / Apple Silicon，Rust 1.99.0（`b940084d7`）。构建：`target/debug/xxassxx`。
- 官方 Codex CLI：**0.160.1**，`codex login status` 确认既有 ChatGPT 登录；没有读取或复制凭据文件。
- 管家：**DeepSeek `deepseek-flash`，thinking=false**，现有私人配置 `model.secrets_file`，密钥由 Rust 在调用时读取；不在命令参数、报告、日志、仓库中保存密钥。
- 新终端依赖锁定 Ratatui 0.29.0、Crossterm 0.28.1、unicode-segmentation 1.12.0、unicode-width 0.2.0；Ratatui/Crossterm 声明的 MSRV 分别为 1.74/1.63，保留项目 `rust-version=1.85`。本次实际编译使用 1.99，未另跑 1.85 编译。
- `cargo build --locked --offline`、`cargo fmt --all -- --check`、`cargo clippy --locked --offline --all-targets -- -D warnings` 全部通过。

本机采用隔离工具链：

```sh
CARGO_HOME=/private/tmp/xxassxx-cargo RUSTUP_HOME=/private/tmp/xxassxx-rustup \
  /private/tmp/xxassxx-cargo/bin/cargo test --locked --offline
```

临时工具链可能被清理；正常安装 Rust 后直接使用 `cargo` 即可。构建/测试记录与二进制校验和见 [证据清单](evidence/phase5/manifest.json)、[测试输出](evidence/phase5/mock-tests.txt)、[构建](evidence/phase5/build.txt)、[Clippy](evidence/phase5/clippy.txt)。格式检查成功时没有输出。

## 模拟与自动测试

本阶段新增 `tests/app.rs` 的 9 项测试，原有 71 项回归继续通过：

| 检查 | 实际证据 |
| --- | --- |
| 身份与入口一致性 | 拒绝其他绑定成员；相同 UUID/相同内容只入队一次，内容变化拒绝；第二渠道补充与 TUI 使用同一任务/历史 |
| 多轮模型与失败 | 实际回环 HTTP 模拟工具调用；自然追问包含前文；后续排队输入不能提前影响当前指令；工具失败不接受成功文字 |
| @ / Tab / Unicode | Tab 只改查看对象；候选没有 Codex；未知/多收件人拒绝；邮件和引用保留；Unicode 字素编辑及 100×30、38×12、10×4 渲染 |
| 本地文件任务 | 无白名单时授权失败；确认后通过真实 Rust MCP 读固定快照；源文件改变不改变已授权内容；有效来源与结果文件保存 |
| 跨端文件任务 | 请求授权 → 对方主人选择文件 → 准备材料 → 发起人采用 → 澄清 → 第二渠道补充 → 原会话恢复 → 结果回原任务 |
| 重复与恢复 | 重复请求/授权/事件不增加 Codex 运行；持久游标恢复；服务中断中的本人指令标记 needs_attention，不自动重放 |
| 等待期间收件 | 模拟模型 HTTP 挂起时，实际信箱消息已落到本端，心跳仍显示 busy；SIGKILL 后重启不重复模型调用 |
| 状态边界 | 未上报=unknown；超期=expired；旧信箱 404 清除推测状态；这些操作不调用模型 |
| 界面授权与停止 | 真实按键处理走 CtrlN/CtrlR/CtrlG/空格/CtrlS，不拼底层 ID；CtrlX 确认后任务 stopped；Esc 不会自动重新关联旧任务 |

旧回归包括消息认证和去重、断线重连、schema 迁移、路径/符号链接/来源/读取预算、Codex 错误分类、超时、进程中断、结果幂等、错误会话拒绝、按 ID 恢复，以及 HTTP 请求中收到 SIGINT。原来默认启动 Codex 的测试迁移到显式 `codex` 子命令并保留 MCP 接入覆盖。

## 实际终端检查

这些是系统 PTY 中运行真实 Rust 界面，不是菜单样图：

- 无参数、`-C`、`open` 三个入口；初始没有本人指令/模型调用，退出不终止服务。
- Tab / ShiftTab / @ 候选、多行括号粘贴、中文、emoji、邮件地址；粘贴前后数据库验证没有自动发送，Enter 后实际 recipient 仍为 owner。
- 110×30 → 38×12 → 110×30 缩放；关闭重开恢复历史。
- CtrlQ、SIGTERM、SIGINT 三种退出，比较前后 termios 完全一致，并检查 alternate-screen 与 paste-mode 关闭序列。
- 在模拟 Codex 正在运行时关闭真正的 TUI：退出时任务 `running`，后台随后通过 Rust MCP 读取固定材料、提交来源、变为 `completed` 并写出结果。
- 在真实 DeepSeek 环境中用 PTY **直接输入两轮自然语言**：“这次终端界面体验的代号是松柏……” → “刚才……代号是什么？”；实际回复为“「松柏」。仅作讨论，未建立任何新任务。”。之后通过按键展开真实任务/来源、同伴历史和原始记录，再退出，后台仍活着。

证据：[PTY 断言](evidence/phase5/pty/pty.json)、[后台继续执行](evidence/phase5/pty/background.json)、[真实 TUI 对话](evidence/phase5/pty/real-tui.json)。原始终端输出：[默认入口](evidence/phase5/pty/default.ansi)、[-C](evidence/phase5/pty/C.ansi)、[open](evidence/phase5/pty/open.ansi)、[真实对话与详情](evidence/phase5/pty/real-tui.ansi)。ANSI 文件含真实控制序列，可在隔离终端重放；没有凭据。

## 新运行的真实双模型闭环

独立环境：`/private/tmp/xxassxx-phase5-real-final`。团队 `phase5-isolated`，owner/test 是新生成邀请凭据的测试身份，各自独立 SQLite、内容库、执行目录、服务进程。信箱仅监听本机回环地址。使用 20 字节合成 CSV：`x=4, y=5`，没有真实科研文件或脚本执行。

1. 启动环境后数据库确认没有模型运行或 Codex 任务。
2. owner 与 DeepSeek 两轮讨论，后文正确回答前文代号“银杏”。
3. owner 发送给 test 管家的普通材料查询。test 的真实 DeepSeek 调用 `shared_objects → knowledge_metadata → submit_reply`，实际 HTTP 回信回到原 owner/test 会话，带对象/版本/清单引用。
4. owner 的真实 DeepSeek 从自然语言创建文件任务。卡片先等待授权。主人通过相同应用接口显式选择 tiny.csv，现有工作流启动本机 Codex，实际 MCP `get_task → read_task_file → submit_collaboration_turn`，返回总和 **9**。原授权指令重放后仍只有一次运行。
5. owner 创建对方文件请求，test 没有自动读取文件。测试以 test 主人身份明确添加合成目录并选择 tiny.csv，owner 从可读的准备任务采用材料。
6. test 的真实 Codex 读取授权 CSV，发送真实澄清问题。owner 通过模拟第二渠道补充“系数 3”，真实 DeepSeek 使用 `answer_peer` 回答。后台按同一个官方 Codex 会话 ID 恢复，得到 **9 × 3 = 27** 和对应 SHA-256 来源，结果归入原应用任务。
7. 双方在线状态由服务端时间租约得到，不依赖模型文本。随后又在真实 TUI 中完成上述“松柏”两轮对话。

跨端 Codex 两轮的会话 ID 都是 `01a1143d-3d84-73f1-b9bd-aacb3158dca5`；第二轮 `resumed_session_id` 精确等于它。三次 Codex 运行（本地一次、远端两次）均 `mcp_initialized=1`、`reads=1`、`submissions=1`、`turn_completed=1`、`exit_code=0`、`state=succeeded`；总计 3 条实际文件读取审计，每次 20 字节。每轮保存了三个 MCP 工具的结构化开始/完成事件。

最终成功环境连同追加的真实 TUI 两轮，共 22 次 DeepSeek HTTP 调用：本人对话 12 次、同伴元数据 4 次、两个工作流协调器各 3 次。Codex 3 次运行。这是调用次数，不是 token/费用统计；不含此前排错环境。

完整记录：[真实验收](evidence/phase5/real/real.json)、[owner 模型与 MCP 证据](evidence/phase5/real/owner.json)、[test 会话恢复及 MCP 证据](evidence/phase5/real/test.json)。导出脚本只读取允许列，不导出数据库凭据摘要、私有配置或认证文件；原 SQLite、结构化 Codex 事件、任务结果保留在独立测试目录。

## 首次失败与修正

没有把最初失败覆盖成通过：

- 初次 DeepSeek 对话直接生成自然语言，未调用保存回复工具，指令准确失败。修正为本人循环在产生有效回复/操作前使用 `tool_choice=required`，并保留模型预算与工具校验。[原失败记录](evidence/phase5/real-initial-failure.json)。
- 第一次普通同伴查询列出了元数据，但把对象版本放入只适用于绑定对象请求的 reply version 字段，被业务层拒绝。改为按原请求是否绑定对象动态提供工具参数；原消息 ID 显式重试后成功。该轮脚本还错误地期待应用事件名 `reply`，实际应用投影叫 `peer_message`，因此验收脚本超时；已修正断言，并在全新目录完整重新运行通过。[排错记录](evidence/phase5/real-peer-retry-record.json)。
- PTY 验收脚本起初误读 service 状态层级，并未考虑宽字符间可能包含光标控制序列；修正断言后，对最终界面重新完成三个入口检查。

## 复现与未验证范围

```sh
cargo build --locked
cargo test --locked
python3 scripts/phase5_demo.py --real --smoke --root /ABS/NEW_TEST_DIRECTORY
python3 scripts/check_phase5_real_tui.py /ABS/NEW_TEST_DIRECTORY /ABS/EVIDENCE
python3 scripts/collect_phase5_evidence.py /ABS/NEW_TEST_DIRECTORY /ABS/EVIDENCE/real

# 不消耗真实模型的 PTY / 后台执行检查
python3 scripts/phase5_demo.py --prepare-only --root /ABS/NEW_PTY_DIRECTORY
python3 scripts/check_phase5_pty.py /ABS/NEW_PTY_DIRECTORY /ABS/EVIDENCE/pty
python3 scripts/check_phase5_background.py /ABS/NEW_PTY_DIRECTORY /ABS/EVIDENCE/pty
```

常规体验一条命令：`python3 scripts/phase5_demo.py --real`，会进入自己的 TUI，并给出第二个 test 窗口与停止脚本路径。

本阶段没有在 Linux 真终端、真实第二台机器、不同终端模拟器/输入法组合、Rust 1.85 上新增实测；不能用此前阶段的 Linux/双机记录替代。UTF-8 已通过真实 PTY 输入，图形输入法候选窗由用户终端负责。Ollama/其他兼容供应商本阶段未做真实聊天验收，必须支持所用工具模式。没有 TG、Windows、任意代码编辑、科研脚本执行、自动更新或 Release。常驻生产 Mac/Lakota/Trader 未被升级或重置。验收用服务结束后停止，记录与材料保留；新体验命令会创建自己的新环境。
