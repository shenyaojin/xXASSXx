# CI 稳定性修复（2026-10-16）

本轮在第二阶段 A–H 实机验收后独立处理，保留失败日志和备份源码。只运行手动分支构建；发布 job 仍仅对版本标签启用。

## 已核实的问题

- `38858986476` 第一次尝试的 Linux `receiving_and_presence_continue_while_owner_model_waits_and_restart_is_safe` 在 `tests/common/mod.rs` 收到测试信箱 HTTP 500；同提交第二次通过。旧日志未含 SQLite 扩展错误码，因此不能从旧日志唯一确定每次 500 的原因。
- 新增可控制并发的真实 HTTP 回归：持有另一个 SQLite 写锁，再调用信箱发送接口。修复前可复现立即返回 500；修复后等待锁释放，成功发送，重复同一消息仍只有一条记录，收端确实收到。
- 原信箱发送在 deferred 事务内先读取成员、消息 ID 和状态，再升级为写事务。WAL 并发下这种升级可立即失败，busy_timeout 无法挽救。改为 BEGIN IMMEDIATE，在读取前取得写入资格；本地任务事实、信箱定时归并和团队动态缓存的同类读取后写入边界同步修正。
- 本次开始时已有的四个带空格 `.rs` 备份测试文件后来进入仓库快照，Cargo 自动发现后会因非法 crate 名称导致构建失败。改名为 `.rs.bak`，逐文件确认字节哈希未变；其他已有修改和备份保持原样。
- `38814657388` 的 ARM Mac 失败来自升级回滚测试在成员名称同步前后比较不同快照，该问题此前已由 `b909500` 修复并通过 CI，本轮保留该修复。
- 本轮 `38950321596` 的 ARM Mac 构建进一步发现文字成果的临时目录经过系统 `/var → /private/var` 链接，被附件的逐级禁止符号链接检查拒绝。仅对内核自己新建的临时目录取得规范路径后再生成文字附件；用户选择文件的禁止链接检查保持有效。原失败测试继续在 ARM CI 上复验。
- 下一轮 `38950916296` 通过文字成果测试后，在改接收人复用固定产物的临时复制目录遇到同类问题。已检查生产源码中全部系统临时目录用法，修正这第二处，并给回归断言附加实际任务状态，方便以后定位；不会放宽用户附件的路径检查。

工作流增加 `RUST_BACKTRACE=1` 以保留以后失败的调用栈。完整测试与断言继续执行；Linux musl 与 Apple Silicon ARM Mac 仍是仅有的构建目标。

## 验证记录

修复前后定向日志：`smoke-output/stability-phase2-20261016/ci-repro-before.log`、`ci-repro-after.log`；前者 1 项失败（HTTP 500），后者同一测试通过。备份改名及字节哈希：`ci-backup-preservation.json`。历史完整 CI 日志同样保存在这个 Git 忽略目录。

最终源码提交 `404a4602394fe7bb46680d8dfcde9f0ed87b27a3` 的 [Actions 38951976776](https://github.com/shenyaojin/xXASSXx/actions/runs/38951976776) 已全部通过。Linux musl 215 项、Apple Silicon ARM Mac 214 项，均 0 失败、3 项既有忽略；两端均完成构建、打包、隔离安装、版本和命令入口检查。发布 job 按分支构建规则跳过，没有生成版本标签或发布。

下载两个安装包后逐项核对 SHA256SUMS。ARM 包另在本机忽略目录隔离安装，确认 `Mach-O arm64`、`xxassxx 0.9.0-alpha`，没有构建本机 target 或替换日常客户端。

| 安装包 | SHA-256 |
| --- | --- |
| `aarch64-apple-darwin` | `44ddee525128aa6cb1e73497305d2281bd6bdc31cd033bab7acfdc4aea722334` |
| `x86_64-unknown-linux-musl` | `3be10bb641d6242eaac476d18d53a4de90ea40b7cd8f90b6f8d75a876a2612de` |

完整记录见 `docs/stability-phase2-evidence.json` 的 `ci` 字段；原始 job 日志、包及本机验证保留在 `smoke-output/stability-phase2-20261016/`。同一最终源码也完成 Lakota 格式、Clippy、215 项完整回归及真实 A/B/C 执行中换人 D2 验收。文档后续提交不改变该已验证源码标识。
