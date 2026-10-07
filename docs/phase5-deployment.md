# Phase 5 重新部署记录

2026-10-06，按用户要求更新现有 Mac、Trader、Lakota，只做构建与启动检查，没有重跑全套测试或科研协作任务。本记录补充此前隔离环境的 [Phase 5 验证记录](phase5-validation.md)。

## 已部署

| 机器 | 角色 | 检查结果 |
| --- | --- | --- |
| Mac | `owner` / `Shenyao "Keith" Jin` 个人端 | 独立 TUI 打开成功；输入“你是谁？”收到真实管家回复；退出界面后后台仍运行 |
| Trader | `https://xxassxx.shenyaojin.com` 公网信箱 | 信箱及 HTTPS 服务 active；经成员认证的在线状态接口可用 |
| Lakota | `test` / `test (Lakota)` 个人端 | SSH PTY 下独立 TUI 打开成功，正常退出，后台仍运行 |

两端个人端配置检查通过，均能经公网信箱看到对方的近期在线状态。没有发送新的同伴任务，没有运行新的 Codex 科研工作流。

两个个人端数据库由 schema 4 升级到 5。身份、原有任务数量和文件白名单保留，Trader 的成员凭据保留。Mac 的白名单仍为空；Lakota 仍仅允许 `/rcp/rcp42/home/shenyaojin/Documents/bakken_mariner/scripts`。登记项目不会自动扩大白名单。

Mac 与 Linux 均使用现有 Rust 工具链构建 release；没有通过系统软件包安装 Rust。Linux 在 Lakota 构建后将同一二进制部署到两台 Linux 主机。

## 现在打开

Mac：

```sh
~/.local/bin/xxassxx open ~/Documents/gitproject/xXASSXx
```

Lakota（先 `ssh lakota`）：

```sh
~/.local/bin/xxassxx open ~/Documents/bakken_mariner
```

Tab 查看同伴状态；开头输入 `@test` 或 `@owner` 给对应管家发消息；CtrlQ 退出界面，后台继续运行。Mac 离线预览安装包已更新为 `dist/xxassxx-macos-arm64-preview.zip`。

## 备份与证据

三台的旧二进制和完整数据目录均已在升级前备份。备份位于各自用户目录下的：

```text
~/.local/share/xxassxx/backups/phase5-deploy-13757b42-5aca-4ed5-9b1d-10cbf8984327/
```

Mac/Lakota 使用其中的 `client/`，Trader 使用 `server/`。备份包含私有配置，应留在原机器私有目录。Schema 5 不支持直接换回旧二进制读取；如需回退，应停止相应服务后成套恢复备份。

本地部署报告：[phase5-deployment.json](../smoke-output/phase5-deployment.json)。报告记录安装二进制摘要、各机器备份位置、身份、服务状态、双方在线状态和检查范围，不包含密钥。

- Mac 二进制 SHA-256：`21c9b68b3100bd20c963b5cd5a254da295b9a1685ec7c0098a857346c368cfa9`
- Linux 二进制 SHA-256：`0a31f483aacbf57c92b4af630ea58bd2e88cd090c182a55eb689935fa15a9f66`

这些记录证明本次部署后的启动和连接可用，不替代完整功能验收。

## 启动目录提示增量更新

同日按用户反馈，在 Mac 和 Lakota 安装了启动目录确认提示。首次进入未覆盖的目录时询问是否加入白名单，记住拒绝选择；已允许的目录及其子目录不重复询问。退出界面后打印后台仍在运行以及停止命令。

这次只替换个人端程序，未重启正在运行的后台或界面，未修改现有白名单、身份和 schema；Trader 信箱继续运行原服务。关闭旧界面并重新打开即可使用。旧程序备份在各自用户目录的 `~/.local/share/xxassxx/backups/startup-folder-8ed06d4f-98b9-456c-a5fe-b3077e5865e5/ui-only/`。

验证包括一项针对授权选择的测试，以及隔离、关闭模型的实际终端检查：拒绝不授权、拒绝后重开不重复询问、同意只加入所选目录、已授权目录不重复询问、终端恢复和退出后后台继续运行。测试环境均已停止。没有重跑全套测试或调用真实模型。本次增量未重新制作安装压缩包；上述安装包和初次部署摘要对应前一版。

安装摘要与证据见 [startup-folder-deployment.json](../smoke-output/startup-folder-deployment.json)。

## local agent 界面调整

随后在 Mac、Lakota 更新了显示名称、空白页和成员状态：界面称呼统一为 `local agent`，已有会话的默认标题在显示时兼容转换；Tab 提示简化为“Tab 切换查看”；空白的本人对话居中显示“今天要做什么？”。最近观测显示运行界面的机器本地日期、时间和时区。有效心跳显示“在线”，过期显示“上次活跃 X 秒/分钟/小时/天前”，从未观测到则显示“未知”。

已有不同终端尺寸的渲染检查通过；隔离终端实际检查了空白页、旧会话标题、本地时区及三种成员状态，没有调用模型或重跑全套测试。两端程序已更新，空闲后台已平稳重启以应用新的自称；原有界面、聊天、身份和白名单保留。退出旧界面并重新打开即可看到更新。旧程序备份目录为各自用户目录的 `~/.local/share/xxassxx/backups/local-agent-ui-2eaaec8d-c2d9-48c8-bd63-2b4078223b37/ui-only/`。

本次证据：[local-agent-ui-deployment.json](../smoke-output/local-agent-ui-deployment.json)。
