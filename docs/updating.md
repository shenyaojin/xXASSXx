# 程序更新

更新器从 `0.5.0-alpha` 开始提供。首次启用前，停止旧后台、退出旧界面，使用本版 `install.sh` 安装一次，保留原安装前缀和个人数据目录；不重新初始化身份。此前安装的 `0.4.0-alpha` 没有更新器。

## 使用

```sh
xxassxx update --check
xxassxx update
```

TUI 启动后异步检查新版；每个安装位置缓存 24 小时，断网或检查失败不阻塞聊天。输入 `/update` 后退出原界面，显示检查、下载、等待后台退出、启动新后台等进度，随后自动打开原工作目录和原会话。失败也会回到原会话并显示原因。`update --check` 不打开或迁移个人数据库，也不启动模型。

默认仅提示。可选择在 TUI 没有未发送输入、没有在途工作且 30 秒无操作时自动安装；仅运行无界面的后台不会自行升级：

```sh
xxassxx update --configure --policy idle
xxassxx update --configure --policy notify
xxassxx update --configure --policy off
```

`off` 关闭自动检查，仍可手动运行更新命令。Alpha 安装默认 `preview` 渠道，包含预发布和正式发布；正式版默认 `stable`，只选正式发布。版本用 SemVer 比较，忽略草稿，不按发布时间判断，不自动降级。

```sh
xxassxx update --configure --channel stable
xxassxx update --configure --channel preview
```

GitHub Releases 是默认来源。自托管 HTTPS 目录须包含 `update.json`、`update.json.sig` 及清单声明的安装包，并使用内置公钥对应的发布密钥签名：

```sh
xxassxx update --check --source https://downloads.example.com/xxassxx/release
xxassxx update --configure --source https://downloads.example.com/xxassxx/release
xxassxx update --configure --source github
xxassxx update --from /path/to/signed-release
```

单次 `--source` / `--channel` 只影响本次；加 `--configure` 才保存。`--from` 为单次离线安装，不改变默认来源。使用非默认个人端时，加 `--directory /path/to/client`。

## 停机、恢复和边界

- 下载及校验阶段不停止后台。签名、大小、SHA-256、解包路径、目标平台、内嵌版本和 schema 均通过后，才请求旧后台退出。
- 后台按现有机制完成正在运行的调用后退出。默认最多等待 300 秒，`--wait-secs` 可设置 1–300 秒；超时不替换程序，停止请求仍有效，界面提示等待结束后重试。不会强杀 Codex 或重放不确定任务。
- 安装目录中的更新锁排除并发更新；运行锁覆盖由该安装启动的界面、个人后台、MCP 与信箱。其他界面或另一份个人数据库的后台仍运行时，本次更新暂缓，已停止的目标个人后台会尝试恢复。锁文件不可删除或重建来“解锁”。
- 新程序先暂存在同一文件系统，再原子替换。旧程序保存在安装目录的 `.xxassxx-previous`。新后台启动后检查持续存活及版本；失败则尝试恢复旧程序和后台。如果新后台无法停止或其他进程占用，不强行回退，报告需检查的状态。
- v1 **只自动安装相同 schema 的版本**。数据库兼容信息统一来自 `store::SCHEMA_VERSION`，探测只读。没有实现数据库降级或迁移快照，也不会通过还原数据库丢弃更新后收到的消息。跨 schema 发布需要另行验证迁移与恢复方案；按该版说明停服、备份一致数据，在新安装前缀安装并切换 PATH，不能强行使用本版自动更新。
- 安装信息、设置与检查缓存放在程序旁边的 `.xxassxx-*.json`，与业务数据库分开。保留自定义前缀，不需要 sudo。源码构建或没有安装记录的程序可检查版本，但拒绝自动覆盖。
- 信箱服务不会被客户端更新命令自动停止或重启。管理员先停止服务，执行更新，再通过原 service manager 启动；运行中的信箱会持有运行锁并拒绝替换。
- 进程租约从本版开始生效。首次安装前必须手动停止不认识租约的旧版本后台和界面。安装脚本拒绝直接覆盖已有更新器安装，后续使用 `xxassxx update`。

## 发布维护

受支持的两平台 `.tar.gz` 和 `SHA256SUMS` 保留。`scripts/sign_release.py` 使用 OpenSSL Ed25519 对清单原始字节签名，先核对私钥与 `src/update/public-key.hex` 一致。默认必须具备 M 系 Mac 和 Linux x86_64 两个平台安装包；`--allow-partial` 仅用于隔离验收。

```sh
python3 scripts/sign_release.py dist/v0.5.0-alpha --key /private/path/ed25519.pem
```

私钥不入库、不放进安装包。GitHub Actions 在创建草稿 Release 前，从 `XXASSXX_RELEASE_SIGNING_KEY` secret 读取 PEM 私钥；缺失或公钥不匹配会明确失败。发布者须在首次发布前配置此 secret，并安全保存密钥。此开发环境的密钥保存在 `~/.config/xxassxx-release-signing/ed25519.pem`（文件 600，目录 700）；文档只记录位置，不记录内容。2026-10-14 已首次公开发布 0.7.0-alpha，并配置 GitHub secret；正式签名公钥已写入源码。此前未公开候选包的私钥未在原位置找到，因此没有沿用候选公钥；这些本地候选安装需手动替换一次正式发布包，不能用旧公钥验证新签名。首次公开签名版本为 0.7.0-alpha，发布结果见 [发布记录](releasing.md)。

清单包含格式版本、应用版本、数据库 schema、简短说明，以及各平台的文件名、大小与哈希。只有托管位置与 GitHub 版本发现放在 `update/release.rs`；更新编排、包验证、文件替换分别位于另外三个模块。服务状态、空闲判定和停止仍由 `service.rs` 负责，TUI 不自行下载或解析发布数据。

发布后不能覆盖同一标签的已发布产物；修复需使用新版本。GitHub 草稿转公开前，沿用 `docs/releasing.md` 的两平台检查，并确认六项产物（两个安装包、校验文件、安装器及两份签名元数据）齐全。

## 验收

自动回归见 `tests/update.rs`。真实 PTY 与实际模型的可复跑入口为 `scripts/check_update_pty.py`；独立服务器及 HTTPS 失败路径入口为 `scripts/check_update_server.py`。两个脚本都要求显式的测试安装位置与测试目录，不替换日常安装。实测结果和边界见 `update-validation.md`。

已有安装记录的旧本地候选包会被 `install.sh` 拒绝覆盖。首次正式公钥迁移须由维护人员在停止后台、退出全部旧界面并保存可恢复副本后，替换为已验证的正式程序及安装记录；不能删除运行锁或在旧界面占用期间强行覆盖。此次 Lakota 已完成，Mac 待旧窗口退出。
