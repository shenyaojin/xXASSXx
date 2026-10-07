# Lakota Mariner 测试个人端

配置日期：2026-10-06。用户明确指定 `scripts` 为白名单，并授权使用现有 DeepSeek 配置进行测试。

| 项目 | 当前值 |
| --- | --- |
| 团队与用户名 | `xxassxx` / `test` |
| 显示名称 | `test (Lakota)` |
| 信箱 | `https://xxassxx.shenyaojin.com` |
| Linux 用户 | 复用 `shenyaojin`，没有创建系统账户 |
| 程序 | `/rcp/rcp42/home/shenyaojin/.local/bin/xxassxx` |
| 个人端 | `/rcp/rcp42/home/shenyaojin/.local/share/xxassxx/client` |
| 唯一白名单 | `/rcp/rcp42/home/shenyaojin/Documents/bakken_mariner/scripts` |
| 管家 | DeepSeek `deepseek-flash`，复用 Lakota 私有配置 |
| 执行器 | Lakota 已登录的官方 Codex CLI |

个人端已启动，退出部署 SSH 后再次检查仍存活。该个人端尚未配置系统开机自动启动；机器重启后需要重新 `client start`。

`scripts` 目录的非隐藏普通文件清单约 11,933 项、192 GB，其中有大量数据和运行输出。本次仅把目录加入白名单，没有递归导入，也没有改变其中任何文件。

## 实际读取验证

仅导入并授权了 `DSS_history_match/109_revised_misfit_func.py` 的一个固定版本，大小 2300 字节。Codex 通过任务 MCP 读取全文，不执行脚本、不读取其依赖或数据文件。

- Linux 新客户端默认测试：68 通过、0 失败、2 个真实测试默认跳过。
- 单独模型工具探针：真实 DeepSeek 两次调用，通过。
- 真实本地读取任务：DeepSeek 三次调度调用，Codex 一轮成功，退出码 0。
- 原始 MCP 返回内容的字节数与 SHA-256 均与源文件一致；任务返回的项目名称、实际函数定义、时间窗口、深度窗口与独立 AST 检查一致；源脚本保持不变。
- 本地检查 workflow：`9cef2a65-e114-4dae-a286-263c4fdc0893`。
- 源对象：`69cf003c-780f-4463-a8bd-9a7d667cdff1`。
- 固定版本：`6f852b0f-1661-4c59-ac2c-d78c774b148a`。
- 源文件 SHA-256：`5e43e196247482ce6d6788d4323db36b8c6f35ea303a04b5c3a16c8cb7514733`。

已另行准备一个接受 `owner` 请求的协作 workflow：`c2fb6fa7-3008-4ee1-9184-41c88af68283`。截至本次配置结束，它尚未收到请求，不能把安装时的本地检查当作 owner/test 双端协作完成。其他脚本仍需按任务显式导入、授权。

无密钥的完整就绪记录保存在本机 `smoke-output/lakota-mariner-ready.json`，原始检查记录在 `smoke-output/mariner-setup-ca96d595-26c4-4962-bc8d-77dbbcdba15a/`，这些目录被 Git 忽略。构建使用 Lakota 已有的隔离 Rust 工具链和 Python 3.12；未通过系统包管理器安装软件。原有 Phase 4 Linux 验收二进制保留。

新增 test 前备份了 Trader 的配置和邀请。现有四名成员的凭据与四条历史消息保持不变，团队现有五名成员。owner/friend 的本机邀请副本已经更新联系人，包含 test。该次 Lakota 安装未初始化 Mac。

后续本机体验配置已完成：Mac 的 owner 个人端已安装、初始化并启动，显示名称为 `Shenyao "Keith" Jin`，Lakota test 的 owner 联系人已同步。Mac 通过 SSH 安全复用了用户自己的 DeepSeek 配置，只提取所需字段并保存在私有文件中；模型探针与连接检查通过。本机白名单仍为空，没有发起 owner/test 协作，记录见 `smoke-output/mac-owner-ready.json`。

## 试用与管理

本机 Codex 使用 [本次试用提示词](lakota-mariner-try-prompt.md)，可以复用已启动的 owner，完成需求记录、发起协作与结果检查。通用的朋友安装流程使用 [安装提示词](codex-setup-prompt.md)。

查看、启动或停止 Lakota 个人端：

```sh
ssh lakota '~/.local/bin/xxassxx client --directory ~/.local/share/xxassxx/client status'
ssh lakota '~/.local/bin/xxassxx client --directory ~/.local/share/xxassxx/client start'
ssh lakota '~/.local/bin/xxassxx client --directory ~/.local/share/xxassxx/client stop'
```

准备新的读取任务时使用同一个人端数据库和白名单，通过 CLI 导入所需的小文件并创建精确 grant。不要将整个 scripts 目录直接作为一次目录快照；它既超过当前快照条目限制，也包含本次无需读取的大量运行产物。
