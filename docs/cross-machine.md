# 两台 Linux 机器上的验证

使用已经配置好的 SSH 别名 `trader` 和 `lakota`。本轮只创建用户目录中的独立测试实例，不修改系统服务、SSH 配置、防火墙或 Codex 登录。

## 结果：通过

日期：2026-10-06（America/Denver）。

| 检查 | 结果 |
| --- | --- |
| macOS 默认测试 | 47 通过、0 失败，2 个真实模型测试 ignored |
| Lakota Linux 默认测试 | 47 通过、0 失败，2 个真实模型测试 ignored |
| 两台 Linux 运行同一二进制 | SHA-256 一致；Trader 上无需安装 Rust |
| 实际跨机器业务链 | 下述 8 项检查全部通过 |
| 正常退出 | A 管家、B 两次启动、信箱两次启动均退出码 0 |
| 本机代码检查 | fmt、clippy（警告视为错误）、Python 语法检查、diff 检查通过 |

最终运行 ID：`ssh-demo-f2f49b72-0227-4c6c-b778-4b505de86d75`。Linux 二进制 SHA-256：`787424d0729542a51e629c8249680e859ffc8f671dcc4f8e59cf481324a30c22`。

- [可随仓库保存的精简证据](cross-machine-evidence.json)。
- [完整本机报告](../smoke-output/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a/demo-2/report.json)。
- [Linux 完整测试日志](../smoke-output/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a/cargo-test-fixed.log)。

完整报告和日志位于 Git 忽略的 `smoke-output`。程序、测试数据保留，服务和隧道已停止；没有建立开机自启服务。

## 拓扑与范围

```text
Trader：管家 A + 团队信箱（仅监听 127.0.0.1）
                     ↕ SSH 隧道，经过本机控制端
Lakota：管家 B + 独立知识库 + 模拟 Codex
```

业务消息仍通过 xXASSXx 的 HTTP 信箱投递；SSH 只提供加密传输和测试进程管理，不替代消息协议。数据库没有在机器之间复制或共享。此拓扑需要控制端保持连接，适合验证；不是脱离本机独立运行的正式部署。

管家模型关闭，明确的 `metadata` 请求由 Rust 回答；明确的 `analysis` 请求进入委托流程。模拟 Codex 启动真实 Rust 任务 MCP，通过 `get_task` / `submit_task_result` 取任务和交结果。它不证明真实 Codex 推理、订阅登录或 DeepSeek/Codex 双模型协作已经通过跨机器验证。

## 重跑

入口是 [`scripts/ssh_demo.py`](../scripts/ssh_demo.py)。需要：

- 两个 SSH 别名可非交互登录，允许本地和远程端口转发。
- 两台机器各有同一份 Linux x86_64 `xxassxx` 二进制。
- A 的 Python 至少 3.8；B 用于模拟 Codex 的 `python3` 至少 3.11。

本轮 Lakota 的系统 `python3` 为 3.6，另有 `/usr/bin/python3.12`。仅在测试二进制所在的 `bin` 目录建立 `python3` 链接；脚本只为自己的子进程将该目录放在 PATH 前面。没有更改系统 Python。Linux 程序在 glibc 2.28 的 Lakota 编译，再运行于 glibc 2.31 的 Trader。

```sh
python3 scripts/ssh_demo.py \
  --host-a trader \
  --host-b lakota \
  --binary-a /home/ubuntu/xxassxx-tests/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a/bin/xxassxx \
  --binary-b /rcp/rcp42/home/shenyaojin/xxassxx-tests/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a/bin/xxassxx \
  --output "smoke-output/ssh-demo-$(date +%Y%m%d-%H%M%S)"
```

每次在两台机器的 `~/xxassxx-tests/ssh-demo-UUID` 创建新的测试目录，并随机生成仅用于此次测试的成员凭据，通过 SSH stdin 传入进程环境。不会读取或复制 `.env`、Codex 凭据或科研数据。配置中只有凭据环境变量的名字。

脚本自动验证：

1. 两台机器的程序 SHA-256 一致。
2. B 离线时信箱接收 A 的请求；信箱重启后 B 上线仍能回复。
3. A 只收到共享元数据，本地对象列表仍为空，回复不包含测试正文。
4. 停止 B，A 针对 V2 追问；B 发布 V3 并重启后，仍以 V2 处理追问。
5. 模拟 Codex 经真实任务 MCP 完成委托，结果通过 HTTP 回到 A。
6. 重复提交同一消息和再次执行已完成委托，没有新增有效消息或执行。
7. A 的管家 MCP 能读取对方回复。
8. 结束时收到远端测试进程以退出码 0 结束的确认，再关闭 SSH 隧道；超时强杀视为失败。

本地输出目录保存 `report.json`、服务日志和 SSH 日志；远端保留数据库、内容库和配置。演示结束后不会留下常驻测试服务。测试成员凭据不持久保存，重跑应使用新目录，不直接重启旧配置。

## 构建隔离

源码通过允许列表打包，仅包含 Cargo 清单、锁文件、Rust 源码、测试、示例和 README。工具链安装在 Lakota 本轮目录的 `tooling` 下，使用独立 `CARGO_HOME` / `RUSTUP_HOME`，不修改 shell profile。首次编译限制为两个并行编译任务。

源码清单、归档哈希及完整构建日志保存在本轮 `smoke-output/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a` 中，便于区分本轮实际构建的代码与后续修改。

首次快照 `source.tar.gz` 对应 46 项测试；修复后快照 `source-fixed.tar.gz` 对应 47 项测试。后者只修改 `src/cli.rs` 和 `tests/team.rs`；随后更新的验证文档不影响二进制。Linux 复验使用 `CARGO_BUILD_JOBS=2`、`TOKIO_WORKER_THREADS=2`、`RUST_TEST_THREADS=2`，并离线复用首次构建的依赖缓存。

## 实际发现并修复的问题

第一次跨机器演示的业务检查全部通过，但日志显示 B 的第一次停止最终使用了 SIGKILL。当时脚本只检查进程已经结束，未要求退出码为 0，因而不能把该报告中的 `passed` 解读为正常退出也通过。

原因是 `butler run` 只在轮询间隔的等待阶段创建 Ctrl-C 订阅；进入下一轮 HTTP 同步时订阅已被丢弃，期间收到的 SIGINT 可能丢失。修复让同一个信号订阅覆盖完整管家循环，保留待处理信号，在当前轮次结束后正常退出。它不会中途丢弃正在持久化的业务状态；等待时间仍受本轮网络、模型和任务超时影响，不承诺所有操作立即取消。

新增回归测试先让实际 HTTP 请求阻塞，在同步期间发送一次 SIGINT，再释放响应；分别覆盖首次同步和第二次同步。旧代码在第二轮场景可稳定复现信号丢失；修复后这两种场景均正常退出。演示脚本也同步收紧为检查所有服务的退出码，强制结束不再记为成功。
