# 第四阶段：安装与常驻团队信箱

本阶段先交付 macOS/Linux 的安装、服务端/个人端配置、模型接入检查与公网通信。Windows 不在范围内。TG 展示、交互式任务卡、成员心跳和启动更新检查仍是后续工作；不以安装成功代替这些功能的验收。

## 两种角色，共用一个程序

| | 服务端 `xxassxx server` | 个人端 `xxassxx client` |
| --- | --- | --- |
| 运行位置 | Trader 或其他团队服务器 | 每位成员自己的 Mac/Linux |
| 职责 | 成员鉴权、持久信箱、消息投递 | 本地知识与授权、管家、自己的 Codex、任务状态 |
| 模型依赖 | 无需 LLM、API key 或 Codex | 管家选 DeepSeek / Ollama / 兼容接口；复杂任务用自己的 Codex |
| 网络 | HTTPS 入口转发到本机 127.0.0.1:7788 | 主动向团队 HTTPS 信箱发起请求 |
| 默认目录 | `~/.local/share/xxassxx/server` | `~/.local/share/xxassxx/client` |

默认遵循绝对路径的 `XDG_DATA_HOME`，也可用 `server/client --directory PATH` 选择独立目录。已有 `--db`、`member`、`butler`、`collaboration` 等命令继续可用。不要将同一成员邀请用于多台独立设备同时运行；当前身份和收件设计是一位成员一个活动个人端。

## 从哪里启动、允许哪些文件

### 终端交互入口

完成一次个人端初始化后，直接运行 `xxassxx` 会打开当前文件夹的 Codex 终端，并自动接入自己的管家 MCP。也可指定工作目录：

```sh
xxassxx
xxassxx open /path/to/project
xxassxx -C /path/to/project
```

启动器复用已保存的个人身份和 Codex 登录，后台管家未运行时先启动，已经运行则复用。它只为这次 Codex 会话传入 MCP 配置，不改全局 Codex 配置、不生成项目 AGENTS.md、不设置新的模型或跳过权限审批。用户直接在 Codex 输入任务；打开空会话本身不提交业务任务或调用管家模型。Codex 可能显示它自己的目录信任或登录提示。

Codex 能通过管家工具查看自己的身份、联系人、知识对象、发送用户要求的消息及读取回复。`butler_context` 提供本地身份、联系人和状态；联系人列表不是在线状态。普通消息与授权文件分析仍是不同操作；后者使用已有的 `collaboration prepare/start` 与精确 grant，不因打开终端而自动获得别人的文件。

个人端目录与当前项目独立。`xxassxx open PATH --directory CLIENT_DIRECTORY` 可选择另一个已初始化的个人端；不接受 `--db` 混用，以免误用项目目录下的旧测试数据库。未初始化、找不到 Codex 或非交互终端会给出明确错误。诊断可使用 `xxassxx open PATH --check`，只查看启动配置，不打开 Codex或启动服务。

切换项目只是选择 Codex 的工作目录，文件操作遵循 Codex 现有权限。xXASSXx 导入白名单保持不变；如需同时允许导入，显式使用：

```sh
xxassxx open /path/to/project --allow-import
```

该选项仅添加所选目录，不扫描、不生成快照、不共享。白名单不是 Codex 的操作系统沙箱。关闭 Codex 后后台管家继续收发消息；`xxassxx client stop` 才会停止管家。

入口使用官方 Codex 的工作目录与单次配置覆盖能力，MCP 使用本地 STDIO。参考：[官方 Codex MCP 文档](https://learn.chatgpt.com/docs/extend/mcp?surface=cli)；实际兼容性以本机已安装 CLI 的帮助和启动验证为准。

2026-10-06 Mac 验证：71 项默认测试通过，两个真实模型测试保持跳过，格式检查与 Clippy 通过。新增交互测试覆盖真实 PTY、MCP 握手/工具调用、含空格及引号的目录、退出码、退出后服务存活、重复打开复用同一管家，以及白名单不自动扩大。本机真实 Codex 0.160.1 启动画面及 `/mcp` 确认 `xxassxx_butler` connected（8 tools），未发送业务任务或请求模型推理。新版已安装并更新 Mac 预览包，记录为 `smoke-output/interactive-launch-check.json`。Linux 入口尚未重新构建部署，现有 Lakota 管家继续使用此前已验证的版本，通信协议不变。

`client` 命令可以从任何目录启动，始终使用同一个个人端；不会把当前目录自动当作项目或获得整台电脑的访问权限。`--directory` 选择的是个人端的数据与配置目录，业务文件夹通过 `--allow-dir` 和 `client roots` 指定。

初次初始化可重复传入 `--allow-dir`。文件夹必须已经存在，相对路径以执行 init 时的目录解析，再保存成真实绝对路径。例如：

```sh
xxassxx client init --invite ./pengchao.json \
  --provider deepseek --model deepseek-flash \
  --model-secrets "$HOME/.config/xxassxx/secrets.env" \
  --allow-dir "$HOME/Research/project-a" \
  --allow-dir "$HOME/Documents/shared-notes"
```

已经初始化的个人端使用下面的命令，不要重新 init：

```sh
xxassxx client roots list
xxassxx client roots add "$HOME/Research/project-b"
xxassxx client roots remove "$HOME/Research/project-a"
```

白名单默认为空，可包含多个目录及其子目录。它保存在个人端 SQLite 中，不随邀请或消息发送，不会被 `member configure` 重置。添加目录仅允许本地读取并导入版本，不会自动扫描、导入或共享文件。路径比较按真实目录边界进行，`data` 不包括 `data-other`；目录外的符号链接目标不能借此获得权限，目录快照仍拒绝符号链接和特殊文件。

文件使用分为两步：先允许源目录并显式导入不可变版本，再给具体协作任务授权相应版本与文件。管家和远端消息没有修改白名单的工具。移除一条根目录只停止该授权范围内的新导入；如果另一个已授权父目录仍覆盖它，权限仍有效。此前导入的版本及任务授权不会随根目录移除而删除或撤销。

这属于 xXASSXx 文件导入接口的权限，不是操作系统对所有进程的文件隔离；程序自身的配置、凭据、数据库和结果仍按各自路径读写。协作任务的 MCP 文件读取继续检查独立任务授权。

使用底层 `object` 命令时仍需指定个人端数据库：

```sh
xxassxx --db "$HOME/.local/share/xxassxx/client/member.sqlite3" object list
```

旧式独立数据库可用 `xxassxx --db PATH roots add DIRECTORY` 管理相同的白名单。升级到当前客户端时会自动迁移到 schema 4，已有身份、消息和版本保留，白名单初始为空；旧数据库的源文件导入也须先授权目录。迁移后请继续使用新版客户端，旧版程序不支持 schema 4。

## 用户名与身份

每个人有团队内唯一的用户名 `member_id`，以及面向人的显示名称 `display_name`。用户名用于消息路由，在同一团队中区分大小写，允许 1–64 个英文字母、数字、下划线或短横线；显示名称可以使用中文。当前由管理员建立邀请时选择，例如 `--member alice=艾丽丝`。导入邀请不会另造一个身份，也不能靠修改本地用户名变成另一个成员。

```sh
xxassxx client whoami
xxassxx client contacts
xxassxx client send --to alice --body "请帮我看看这个任务"
```

`whoami` 显示用户名、显示名称、团队和信箱地址，不输出凭据。身份在一个信箱内由 `team_id + member_id` 确定，跨信箱还需要服务地址，不是全网唯一用户名。身份认证使用邀请中的独立随机凭据，用户名本身不是密码。当前尚未提供自助注册或团队统一改名；现有 `owner` / `pengchao` 是该团队已经预留的用户名。

## 安装

安装器下载对应架构的预编译程序，核对 SHA-256 后安装到 `~/.local/bin`，不要求用户安装 Rust、不使用 sudo、不自动修改 shell 配置。支持 Apple Silicon/Intel macOS；发布工作流还构建 Linux x86_64 musl 包。初次发布前，GitHub 下载入口还不可用。

首次 GitHub Release 发布后，可以下载该 Release 的 `install.sh` 并运行：

```sh
sh install.sh --version v0.1.0-alpha
export PATH="$HOME/.local/bin:$PATH"
xxassxx --version
```

也可使用离线分发目录，目录须包含对应 `.tar.gz` 与 `SHA256SUMS`：

```sh
sh scripts/install.sh --from dist
```

本次本机构建另提供 `dist/xxassxx-macos-arm64-preview.zip`，适用于 Apple Silicon Mac。解压后进入 `xxassxx-preview` 目录，执行 `sh install.sh --from .` 即可离线安装。该包不含邀请或密钥；Intel Mac 需等待对应架构的发布包。

现有程序在下载或校验失败时保留原样。安装器不会改动数据库和配置。macOS 包目前没有 Developer ID 签名或公证；GitHub 校验值用于核对下载包完整性。

GitHub 的 `release.yml` 在版本 tag 推送后测试并构建三个目标，成功后创建草稿 Release；手动触发只生成构建产物。需要核验草稿后发布，朋友才能从正式下载地址安装。当前仓库未提交的实现必须先整理提交并推送，安装器不可能从 GitHub 下载尚未发布的内容。

## 管理员建立团队

```sh
xxassxx server init --team lab \
  --public-url https://xxassxx.shenyaojin.com \
  --member owner=Owner --member pengchao=pengchao
xxassxx server serve
```

第一次初始化要求目标目录不存在，避免覆盖已有身份和密钥。输出只包含配置和邀请文件位置，不显示凭据。每个人仅领取自己的 `invitations/MEMBER.json`；邀请文件包含长期成员凭据，应通过你信任的方式交付，不能提交到 Git。邀请不会授权任何本地文件。

服务器配置和凭据独立保存，权限为 600，父目录为 700；服务直接解析指定字段，不执行 env 文件。重新启动从同一配置读取稳定凭据和 SQLite 消息。当前成员清单在启动时加载，更改成员后要同步相关个人端联系人并重启信箱；尚未实现自助注册、邀请过期或动态成员管理。

Trader 使用 `xxassxx.shenyaojin.com`，DNS A 记录指向 Trader，Cloudflare 设置为仅 DNS。`deploy/Caddyfile` 由 Caddy 管理公网证书与续期，转发到回环地址。TCP 80/443 需对外可达；不直接公开 7788。

部署模板：`deploy/xxassxx-mailbox.service` 是用户级 systemd 服务；退出 SSH 后持续运行/重启自动启动需要为该用户启用 linger。`deploy/xxassxx-https.service` 是 Trader 专用的系统服务，以 ubuntu 运行，仅获得绑定低端口的能力。Caddy 官方二进制放在独立用户目录，无需系统包管理器。本次已部署并通过公网双机任务与服务重启验证，见 [验证记录](phase4-validation.md)。

## 当前团队与邀请

本次 Trader 实例使用团队 `xxassxx`，入口为 `https://xxassxx.shenyaojin.com`。当前正式成员只有 `owner`、`pengchao`、`test`。2026-10-06 已移除早期验收成员 `a`、`b`，同步服务器、Mac/Lakota 联系人和现有邀请；其旧凭据不再被正式信箱接受，历史记录与备份保留。`test` 在 Lakota 常驻运行，白名单为 Mariner 的 scripts 目录。服务器邀请保存在 `~/.local/share/xxassxx/server/invitations/`。本机有权限 600 的副本 `smoke-output/phase4-private/pengchao.json`，仅交付朋友自己的邀请；该目录被 Git 忽略，不包含在安装包中。

这台 Mac 已导入 `owner.json` 并启动日常个人端，显示名称为 `Shenyao "Keith" Jin`，通信用户名仍为 `owner`；团队服务配置、邀请和 Lakota test 的联系人已同步显示名称，成员凭据保持不变。程序位于 `~/.local/bin/xxassxx`，数据位于 `~/.local/share/xxassxx/client`，本机白名单为空。DeepSeek 工具调用、Codex 登录和公网身份检查均通过，配置记录见本机 `smoke-output/mac-owner-ready.json`；首次 owner/test 业务协作仍由用户发起。

朋友导入 `pengchao.json`，按下面的个人端流程登录自己的 Codex、提供模型配置并启动管家。朋友的安装不需要 SSH，也不要求和你在同一个局域网。此前真实验收仍保留在独立测试目录，不占用日常个人端的数据。

## 朋友加入（先用 DeepSeek）

当前可交付的新版 Apple 芯片包：`dist/xxassxx-pengchao-macos-arm64-20261006.zip`，已验证离线安装、二进制哈希和 schema 6 初始化。此包含本次原生 Codex 只读委托功能；旧 `v0.1.0-alpha` 包不代表已包含后续改动。把新 ZIP 和私人 `pengchao.json` 分别交给朋友，按下述安装提示词配置。Intel Mac 不适用该 ZIP。

也可直接把 [个人端安装提示词](codex-setup-prompt.md) 交给朋友自己电脑上的 Codex，由它处理下面的安装与配置步骤。本机与 Lakota test 的首次体验另有 [已填好参数的提示词](lakota-mariner-try-prompt.md)。

先安装并登录自己的官方 Codex CLI。将 DeepSeek key 放入自己拥有、权限 600 的 `~/.config/xxassxx/secrets.env`，字段为 `DEEPSEEK_API_KEY`；不要把值作为命令行参数或发到群里。

```sh
xxassxx client init --invite ./pengchao.json \
  --provider deepseek --model deepseek-flash \
  --model-secrets "$HOME/.config/xxassxx/secrets.env"
xxassxx client doctor --probe-model
xxassxx client start
xxassxx client status
xxassxx client contacts
```

`deepseek-flash` 是此前真实检查通过的名称，使用时以你账号当前提供的模型为准。个人端导入邀请后保存独立凭据，因此以后启动不需要重新 export 信箱 token。关闭终端不停止管家；`client stop` 请求停止。`client start` 遇到临时网络故障会在有限间隔内继续重连，鉴权或配置错误仍会阻断。网络重连不调用 LLM，也不会重置工作流的模型/时长预算。

`doctor` 默认检查配置、信箱身份与 Codex 登录，不进行模型推理；加 `--probe-model` 明确进行两次管家模型请求，验证真实工具调用及工具返回结果的接收，可能消耗 API 额度。工具探针通过不代表完整协作可靠性，首次实际任务仍需检查结果。

原来的业务命令可对个人端数据库使用：

```sh
xxassxx --db "$HOME/.local/share/xxassxx/client/member.sqlite3" object list
xxassxx --db "$HOME/.local/share/xxassxx/client/member.sqlite3" collaboration --help
xxassxx client tasks
xxassxx client task WORKFLOW_ID
xxassxx client result WORKFLOW_ID
```

`client send --to MEMBER --body TEXT` 将请求加入本地发件箱，由常驻服务投递。元数据查询可加 `--operation metadata --object-id OBJECT_ID`。第三阶段的跨端文件分析仍需接收方先 prepare 并明确授权文件，不因安装或收到消息自动扩大访问范围。`client contacts` 目前是联系人列表，还不是在线状态。

## Ollama

已经有兼容接口，本次只补 `provider=ollama` 的默认值和显式工具探针。默认连接本机 `http://127.0.0.1:11434/v1`，不需要 API key，也不发送 DeepSeek 特有字段。不会替用户安装 Ollama 或下载模型。

```sh
xxassxx client init --invite ./pengchao.json \
  --provider ollama --model YOUR_INSTALLED_MODEL
xxassxx client doctor --probe-model
```

模型名必须替换为本机实际安装的名称，并且模型要支持工具调用。配置示例见 `examples/ollama.toml`。两种 init 示例二选一；已有个人端可以编辑生成的 `member.toml`，再应用到同一数据库，保留既有身份、任务和数据：

```sh
xxassxx --db "$HOME/.local/share/xxassxx/client/member.sqlite3" \
  member configure "$HOME/.local/share/xxassxx/client/member.toml"
```

本轮没有本地 Ollama 模型，因此只能验证模拟兼容端点的请求/工具闭环；不能宣称任意千问模型已通过真实测试。朋友可先用 DeepSeek，随后用自己的具体模型完成探针及小任务验收。

参考：[Ollama 兼容 API](https://docs.ollama.com/api/openai-compatibility)、[工具调用](https://docs.ollama.com/capabilities/tool-calling)、[Caddy 自动 HTTPS](https://caddyserver.com/docs/automatic-https)。
