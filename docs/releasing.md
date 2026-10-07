# 版本构建与发布

当前已发布版本为 `0.2.0-alpha`，对应 Git 标签 `v0.2.0-alpha`。版本来源是 `Cargo.toml`；`Cargo.lock` 中本包版本保持一致，CLI 和 MCP 自动使用该版本。

## 发布前验收

先按 [调试与终端验收流程](debugging.md) 完成受本轮改动影响的真实用户操作。涉及卡住、授权、执行地点、远程接续或最终答复时，必须亲自启动真实终端，从发起任务看到最终回答，并核对实际文件/运行结果。自动测试成功、后台 completed 或一段模型自述不能代替终端验收。记录精简证据及所用二进制哈希；仅改版本号或文档时说明引用哪一轮真实验证，不冒称重新跑过业务计算。

发布必须同时提供以下三个平台的安装包：

| 系统 | 架构 | 安装包 |
| --- | --- | --- |
| macOS | Apple Silicon | `xxassxx-aarch64-apple-darwin.tar.gz` |
| macOS | Intel | `xxassxx-x86_64-apple-darwin.tar.gz` |
| Linux | x86_64 musl | `xxassxx-x86_64-unknown-linux-musl.tar.gz` |

同时附上 `SHA256SUMS`、`install.sh` 和对应版本说明。不能将某一台机器的本地构建说成所有平台已验证。

## 本机构建

需要 Rust 工具链、Python 3.11+（测试使用）及平台编译工具。在仓库根目录执行：

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
./target/release/xxassxx --version
sh scripts/package.sh target/release/xxassxx aarch64-apple-darwin dist/v0.2.0-alpha
cat dist/v0.2.0-alpha/*.tar.gz.sha256 > dist/v0.2.0-alpha/SHA256SUMS
cp scripts/install.sh dist/v0.2.0-alpha/install.sh
```

示例打包目标为 Apple Silicon；其他平台必须使用对应目标的真实二进制。依赖已缓存时可追加 `--offline`。产物按版本隔离，`dist/` 和含私有运行数据的 `smoke-output/` 不提交。

安装验证使用临时目录，不覆盖日常个人端：

```sh
sh scripts/install.sh --from dist/v0.2.0-alpha --prefix /tmp/xxassxx-020-check
/tmp/xxassxx-020-check/bin/xxassxx --version
```

核对安装后版本、二进制哈希及终端入口。升级日常个人端或团队信箱时，先确认没有在途任务，保留已有身份、配置和数据库，停止旧后台后更换并重新启动。1.0.0 前属于小规模测试，本轮按用户要求不创建备份，不重新初始化。

## GitHub 构建与预发布

`.github/workflows/release.yml` 支持手动构建，以及推送 `v*` 标签触发：

1. 提交所有实现、测试、文档和版本变更。检查远端状态，正常推送；标签必须指向包含完整实现的提交，不覆盖已有发布标签。
2. 验证标签与 `Cargo.toml` 完全对应，存在 `docs/releases/<标签>.md`。
3. 在上述三个平台测试、优化构建、核对版本，并验证对应安装包能离线安装。
4. 三个平台均通过后，汇总包与校验值，创建带安装器和版本说明的草稿 Release；Alpha 标记为 prerelease。
5. 检查草稿的五项产物、校验值、平台/版本对应关系以及安装结果，再发布草稿。用户已明确要求发布时无需再次索要同一项许可；权限或外部环境阻塞则准确报告状态。
6. 确认 Release 已公开、不是 draft、保持 prerelease，并验证公开下载可用，记录提交、标签、CI 和 Release 链接。

手动触发只生成 Actions 产物。标签推送成功不等于 Release 已发布；单个平台成功也不等于发布完成。Alpha 安装显式传入 `--version v0.2.0-alpha`，不依赖稳定版 latest。

## 0.2.0-alpha 发布结果

2026-10-07 22:25:43 UTC，[v0.2.0-alpha](https://github.com/shenyaojin/xXASSXx/releases/tag/v0.2.0-alpha) 已公开发布，标记为 prerelease，非 draft。标签指向代码提交 `fdeae034b1979ec8ac206e326ebebfb5e19a4e3d`。

- [GitHub Actions](https://github.com/shenyaojin/xXASSXx/actions/runs/37694798545) 三个平台全部成功，各 111 项测试通过，两项旧真实模型测试按默认配置跳过；安装后版本均为 `xxassxx 0.2.0-alpha`。
- 三份最终安装包的 SHA-256、文件内容与二进制架构核对通过；公开下载的三个包、校验文件和安装器均返回成功。
- 最终 Mac 发布包在本机独立目录安装通过；Linux musl 发布包在 Lakota 独立目录安装启动通过。此次没有替换日常个人端或信箱服务。
- 本机发布前格式、Clippy、完整测试、优化构建和实际 PTY 入口检查通过。此前真实任务及科研计算的证据按各自二进制保留，没有把本次版本号调整说成重新跑过科研计算。

精简证据见 [发布验证记录](releases/v0.2.0-alpha-evidence.json)。下载产物和完整检查日志保存在被忽略的 `dist/v0.2.0-alpha/`，不提交个人数据。

## 历史构建记录

### 0.1.0-alpha 本次构建结果

2026-10-06 22:18（America/Denver），按最新代码重新构建，Rust 1.99.0 / Apple Silicon macOS。本包替代同日 21:07 的旧构建，版本号保持 `0.1.0-alpha`：

- 优化构建、格式检查、Clippy 均通过，二进制为 Mach-O arm64，版本输出为 `xxassxx 0.1.0-alpha`。
- 84 项默认自动测试通过，0 失败；2 项真实模型测试按默认配置跳过。本次未重新调用真实模型。
- ZIP 解压后，从含空格目录离线安装通过；安装后版本、二进制哈希、帮助及隔离数据库初始化均通过。
- 损坏安装包被 SHA-256 校验拒绝，已有安装保持原样。
- 使用旧安装包创建的 schema 5 隔离数据库，升级到 schema 6 后原有任务记录保留，旧二进制明确拒绝读取迁移后的数据库。
- 构建和打包前后对照全部源码及测试文件哈希，确认对应同一份最新代码。
- 发布流程沿用上一轮已验证的 Alpha 配置；本次未运行 GitHub Actions。

可分发文件为 `dist/v0.1.0-alpha/xxassxx-0.1.0-alpha-macos-arm64.zip`，旁边的 `.sha256` 文件用于校验 ZIP；内部 `SHA256SUMS` 用于校验安装器读取的二进制压缩包。`validation/build-info.json` 保存产物、源码和测试文件的 SHA-256、工具链及检查结果，本次测试结果位于 `validation/tests.txt`。旧构建及其日志归档到 `dist/archive/v0.1.0-alpha-20261006T210739/`，不能与当前分发包混用。
