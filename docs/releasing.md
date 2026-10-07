# 版本构建

首版为 `0.1.0-alpha`，对应 Git 标签 `v0.1.0-alpha`。版本来源是 `Cargo.toml`；`Cargo.lock` 中本包版本保持一致，CLI 和 MCP 自动使用该版本。

## 本机构建

需要 Rust 工具链、Python 3.11+（测试使用）及平台编译工具。在 Apple Silicon Mac 的仓库根目录执行：

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
./target/release/xxassxx --version
sh scripts/package.sh target/release/xxassxx aarch64-apple-darwin dist/v0.1.0-alpha
cat dist/v0.1.0-alpha/*.tar.gz.sha256 > dist/v0.1.0-alpha/SHA256SUMS
cp scripts/install.sh dist/v0.1.0-alpha/install.sh
```

依赖已缓存时可给 Cargo 的测试、检查和构建命令追加 `--offline`。打包目录按版本隔离，避免把旧预览包当作首版产物。

离线安装验证可指定临时前缀，不修改日常个人端：

```sh
sh dist/v0.1.0-alpha/install.sh --from dist/v0.1.0-alpha --prefix /tmp/xxassxx-alpha-check
/tmp/xxassxx-alpha-check/bin/xxassxx --version
```

本次构建的检查日志保存在 `dist/v0.1.0-alpha/validation/`。该目录和所有分发包均被 Git 忽略。

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

## GitHub 构建与预发布

`.github/workflows/release.yml` 支持手动构建，以及推送 `v*` 标签触发：

1. 验证标签与 `Cargo.toml` 完全对应，且存在 `docs/releases/<标签>.md`。
2. 在 Apple Silicon macOS、Intel macOS、Linux x86_64 musl 上测试、优化构建并检查二进制版本。
3. 汇总三个安装包与 `SHA256SUMS`，标签构建创建含安装器和版本说明的草稿 Release。
4. 带 `-alpha` 等预发布后缀的标签自动标记为 prerelease；检查产物后再发布草稿。

手动触发仅生成 Actions 产物。本次本地构建不代表 GitHub 工作流已运行或 Release 已发布；发布标签必须指向包含完整实现和版本配置的提交。

Alpha 用户安装时显式传入 `--version v0.1.0-alpha`。后续版本同步修改清单、锁文件、版本说明及对应安装示例，再按相同流程构建。
