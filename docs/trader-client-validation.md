# Trader 个人客户端部署与验收

日期：2026-10-14（America/Denver；Trader 终端使用自己的本地时区）。公开包为 [v0.7.1-alpha](https://github.com/shenyaojin/xXASSXx/releases/tag/v0.7.1-alpha)，精简状态见 [JSON 证据](trader-client-evidence.json)。

## 用户选择与安装

用户要求在 Trader 建立个人客户端，指定成员名 `trader`，开放整个 `/home/ubuntu/projects/crypto_playground`，随后明确要求保留 Full access。实际通过该客户端的 `/access` 选择 Full access，退出终端、停止并重新启动个人后台，再打开实际终端确认模式保留。它允许本机读写、运行、联网及免逐次批准发送；任务明确要求只读时仍须遵守。Lakota 保持 Ask for approval。

先保存信箱数据库、配置及凭据文件的可恢复副本，再登记新成员并生成 mode 600 的独立邀请。旧四位成员的登记与凭据哈希逐项一致，HTTPS 身份校验通过，成员目录包含五位。邀请在 Trader 直接导入；产品尚无网页邀请链接、客户端邀请按钮或成员增删命令，不把管理员本次操作描述成已有产品功能。私密文件位于 Trader 的 `~/.local/share/xxassxx/server/invitations/trader.json`，不入库、不公开正文。

个人客户端安装在 `~/.local/xxassxx-client/bin/xxassxx`，使用公开的签名 Linux musl 包。登录 shell 的 PATH 已指向它，数据保存在 `~/.local/share/xxassxx/client`。信箱继续使用独立的 `~/.local/bin/xxassxx` 0.7.0，原 systemd 进程未重启。原配置和登录文件均有备份，目录为 `~/.local/share/xxassxx/backups/add-trader-20261014/`。

真实 DeepSeek 工具往返检查成功；官方 Codex 0.162.1 已登录。实际 Codex 是 standalone Linux 程序，并不依赖该机器旧版系统 Node。doctor、信箱连接和个人数据库完整性检查通过，个人 schema 10、信箱 schema 2。

## 真实终端与任务

通过实际可交互 PTY 启动发布包，亲自输入仅阅读项目根目录 README 的自然语言任务。任务 `7fb47620-36e4-5ead-a5fb-6f27e8d1aa67` 第 1 版，发起方和执行方均为 `trader`；本轮不是跨成员业务传输验收。

起初 Ask 模式的三次 Codex 调用均报告 shell 启动阶段 `bwrap` 的 Permission denied，任务如实进入等待，没有读到文件。数据库 run 显示 succeeded 仅说明提交成功，不能据此声称读取成功。用户随后明确要求 Full access，实际切换、重启确认持久，再在原任务回答继续；没有重建任务或改数据库伪造成功。两次后续调用的内核事件均记录 `full_access`：先读取单个 README 的元数据，再读取其冻结副本完成分析。

最后亲自在终端看到任务“已完成”、待处理数 0，以及正确的项目名称、用途和原文行号依据。执行记录中成功 shell 命令只有对 README 的 `stat` 和对冻结副本的 `nl -ba`；原文件 4984 字节，SHA-256 前后相同。没有项目脚本或交易运行，没有向其他成员发送消息或附件。详细终端画面、模型输出和项目原文仅保存在 Git 忽略的本地证据中，不复制到公开文档。

最终答复最后一句声称没有阅读 README 的其他内容，措辞不准确：实际分析打印了这一个 README 的全文。项目名称、用途和来源行号已核对；不能把“仅一个文件”夸大成“仅读指定行”。此模型表述问题及 Ask 模式下 standalone Codex 的启动问题均作为观察项保留，本轮没有修改源码或宣称修复。

## 留存状态与边界

- Trader 个人后台 0.7.1 运行中，Full access 持久；整个项目目录已登记，退出验收终端后后台继续运行。
- 信箱 0.7.0 原进程保持 active/running，Lakota 0.7.1 后台也运行中。
- 未执行量化项目脚本、写入项目源文件或向团队发送测试内容；本次不证明量化策略或数据的正确性。
- 当前仅后台常驻，未新增个人客户端的系统重启自启动功能。Mac mini 未直接检查或升级；本轮执行电脑的 MBA 日常 0.5.0 安装未改。
- 自动回归和发布构建证据见 [发布证据](releases/v0.7.1-alpha-evidence.json)，与本次真实 DeepSeek/Codex/PTY 证据分开记录。

用户登录 Trader 后可执行：

```sh
xxassxx ~/projects/crypto_playground
```
