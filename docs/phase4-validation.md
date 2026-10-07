# 第四阶段验证记录

日期：2026-10-06。安装与公网通信的 MVP 验收通过。GitHub Release 尚未发布，Ollama 的具体本地模型尚未真实验收。

## 已部署的服务

- 团队入口：`https://xxassxx.shenyaojin.com`，DNS 指向 Trader，Cloudflare 仅 DNS。
- Trader 的 `xxassxx-mailbox.service` 为用户级常驻服务，监听 `127.0.0.1:7788`；`xxassxx-https.service` 使用 Caddy 2.11.7 提供公网 HTTPS。
- 两个服务均已启用开机启动，ubuntu 用户已启用 linger。只重启了本次建立的信箱服务，没有重启服务器或改动其他应用。
- 从 Mac 请求未带凭据的 `/v1/whoami` 返回 401，TLS 验证为 0（成功）；两个已导入邀请的个人端均通过身份检查。
- 真实任务结束后重启信箱，4 个成员、4 条消息、消息正文和收取确认状态均保持不变；重启后通过原成员凭据继续访问 HTTPS。

信箱配置与数据位于 Trader 的 `/home/ubuntu/.local/share/xxassxx/server`，程序位于 `/home/ubuntu/.local/bin/xxassxx`。服务端无需 DeepSeek key、Ollama 或 Codex。团队成员 `a`、`b` 用于验收，`owner`、`friend` 留给日常个人端。

## 真实公网双机任务

运行目录：`smoke-output/phase4-public-qc-1`。运行 ID：`phase4-https-d0f81bc7-4a59-48b3-8b85-cf7ad0648733`。

沿用第三阶段的合成“实验数据质控”材料、任务与标准答案。没有修改 CSV、实验方案或预填任何问答。两端使用相同 Linux 二进制，各自从文件读取自己的 DeepSeek key、使用自己的 Codex 登录和独立 SQLite 数据库。

通信直接使用 HTTPS 信箱。观察脚本的 SSH 命令只用于部署、启动、查看和清理，没有创建 `-L` / `-R` 隧道或 SSH 控制主连接。服务启动后还保留了 12 秒完全不发送 SSH 观察命令的间隔。Rust 服务负责所有消息投递与模型续接。

自动往返依次为：

1. Trader 读取实验方案，要求 Lakota 先确定实际批次并提出规则问题。
2. Lakota 读取实际 CSV，确认 8 个样本、p7 / q4 两个批次，追问校准、阈值、边界与报告规则。
3. Trader 续接原会话，提供实际批次对应的参数与完整规则。
4. Lakota 续接原会话，计算并返回逐样本结果及原始读数。
5. Trader 再次续接，重读方案、复核计算并保存最终汇总。

| 项目 | Trader | Lakota |
| --- | --- | --- |
| Codex 轮数 | 3 | 2 |
| 原会话 ID | `01a113cf-6b5e-7033-b9ac-de72de5fe535` | `01a113d0-1208-7a13-ae3f-badf9cd20a35` |
| 每轮真实文件读取 | 1025 字节 | 162 字节 |
| 工作流 DeepSeek 调用 | 3 | 3 |
| 后续轮次 | 均续接原会话 | 续接原会话 |

五轮均有成功 MCP 读取与提交、`turn.completed`、退出码 0。独立检查逐轮核对原始 MCP 返回正文与材料字节一致，重新计算两个最终结果，检查两端精确来源引用与有序往返。结果为 8 个样本、4 个通过，S02、S03、S06、S08 需复测。

完成后确认空闲状态不再新增模型或 Codex 调用。两个测试管家均已停止，常驻团队信箱与 HTTPS 服务继续运行。未为 owner / friend 自动启动日常个人端。

原始证据：

- [运行报告](../smoke-output/phase4-public-qc-1/report.json)
- [独立验收报告](../smoke-output/phase4-public-qc-1/independent-check.json)
- [Trader 问答与收据](../smoke-output/phase4-public-qc-1/a-workflow.json)、[原始执行事件](../smoke-output/phase4-public-qc-1/a-execution.json)、[最终结果](../smoke-output/phase4-public-qc-1/a-result.json)
- [Lakota 问答与收据](../smoke-output/phase4-public-qc-1/b-workflow.json)、[原始执行事件](../smoke-output/phase4-public-qc-1/b-execution.json)、[最终结果](../smoke-output/phase4-public-qc-1/b-result.json)
- [服务重启检查](../smoke-output/phase4-restart-check.json)

原始输出与私人邀请均位于被 Git 忽略的 `smoke-output`。仓库内保留不含凭据的 [精简证据](phase4-evidence.json)。

## 安装与兼容性

- macOS 默认回归 64 项通过、2 项真实模型测试默认跳过；最后的 Codex 路径助手复用改动之后，5 项安装/配置测试及全目标 clippy 通过。
- 最终源码在 Linux 完整回归：64 项通过、0 失败、2 跳过。使用已有隔离 Rust 工具链，未用系统包管理器安装 Rust。
- Apple Silicon macOS 优化构建、离线安装通过；安装目录含空格也可使用。损坏安装包被拒绝，已有程序保持原样。
- 安装包：`dist/xxassxx-macos-arm64-preview.zip`。无 Rust / sudo 依赖，不含邀请、API key、数据库或个人文件。
- 新增 `doctor --probe-model` 在 Trader 的真实 DeepSeek 上通过了两次工具调用闭环；这两次额外检查不计入上表的工作流调用数。
- Ollama 默认地址、无鉴权请求格式和工具闭环由模拟 HTTP 端点验证；本轮没有安装或运行真实 Ollama / 千问模型。
- GitHub Actions 已配置 Apple Silicon、Intel macOS、Linux x86_64 musl 构建；YAML 解析通过，但工作流未执行，Release 未发布。Trader 实际部署的是 Linux GNU 本机构建，不能当作已经验证的 musl 分发包。

最终 Linux 二进制 SHA-256：

```text
be31f0c75e7f25ec08a501708fbdc7732715171cf2d03e2e0d6e04accdb4d53c
```

## 复核与使用

离线复核本次验收：

```sh
python3 scripts/check_phase4_public.py smoke-output/phase4-public-qc-1
```

服务状态：

```sh
ssh trader 'systemctl --user status xxassxx-mailbox.service --no-pager'
ssh trader 'systemctl status xxassxx-https.service --no-pager'
```

朋友加入、模型选择与安装步骤见 [第四阶段使用说明](phase4.md)。当前 `client contacts` 是静态联系人列表；启动更新检查、成员在线心跳、TG 展示、交互式任务卡和动态邀请管理均未实现。个人端需主动 `client start`，尚未提供 macOS 登录时自动启动配置。

## 后续补充：目录白名单与用户名查看

同日加入 `client init --allow-dir`、`client roots add/list/remove`、底层 `--db DB roots` 和 `client whoami`。个人数据库增量迁移至 schema 4，既有内容保留，旧数据库的白名单也默认为空。白名单在源文件导入入口统一执行，协作任务仍使用原有固定版本文件授权。不会仅因切换启动目录而切换身份或扩大文件来源。

该补充版本的 macOS 完整默认回归为 **68 项通过、0 失败、2 项真实模型测试跳过**，全目标 clippy 无警告。新增四项测试覆盖空白名单、添加/移除与持久性、相似前缀与路径越界、符号链接越界、旧库迁移保留身份与任务，以及从不同目录运行个人端仍使用同一身份。原协作测试与演示材料增加了显式目录授权前置步骤。

Apple Silicon 优化构建与离线安装包已更新，安装后文件哈希与构建产物一致，新命令可用。检查记录见 `smoke-output/directory-allowlist-check.json`，完整日志为 `smoke-output/directory-allowlist-tests.log`。此前公网验收和 Linux 二进制证据保持为上面的历史版本；本次未替换 Trader 服务、未在 Linux 重跑新客户端，也未额外调用真实模型。`phase4-evidence.json` 的原始源文件/安装包指纹对应先前公网验收版本，新的预览包指纹见本次检查记录。
