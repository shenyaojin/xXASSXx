# xXASSXx

> 我怀着对slack/MS teams和meeting的极度厌恶 设计了xXASSXx --Shenyao "Keith" Jin

加入已有团队，从[成员安装](#成员安装)开始。首次搭建团队，由管理员先完成[团队部署](#团队部署)。

## 成员安装

准备好管理员发给你的**安装包**和**个人邀请文件**（例如 `alice.json`），以及自己的 Codex 登录和 DeepSeek API key。成员电脑使用 macOS 或 Linux；目前不支持 Windows。

### 1. 安装程序

当前测试版为 [0.4.0 Alpha](https://github.com/shenyaojin/xXASSXx/releases/tag/v0.4.0-alpha)，提供 Apple 芯片 Mac、Intel Mac 和 Linux x86_64 安装包。

解压安装包，进入包含 `install.sh` 和 `SHA256SUMS` 的目录：

```sh
sh ./install.sh --from .
export PATH="$HOME/.local/bin:$PATH"
xxassxx --version
```

安装器会校验 SHA-256，并将程序安装到 `~/.local/bin/xxassxx`，不需要 Rust 或管理员权限。安装包必须匹配电脑架构：Apple 芯片 Mac、Intel Mac、Linux x86_64 的包不能混用。

将下面一行加入自己的 shell 配置（zsh 用 `~/.zshrc`，bash 通常用 `~/.bashrc`），让新终端也能找到程序：

```sh
export PATH="$HOME/.local/bin:$PATH"
```

没有对应架构的安装包时，可在目标机器[安装 Rust](https://rust-lang.org/tools/install/) 和平台编译工具后，下载本仓库源码，在仓库目录执行：

```sh
cargo build --release --locked
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/xxassxx "$HOME/.local/bin/xxassxx"
```

### 2. 准备 Codex 和模型密钥

先按[官方说明安装 Codex CLI](https://learn.chatgpt.com/docs/codex/cli)，并在本机完成登录：

```sh
codex --version
codex login
codex login status
```

已登录可跳过 `codex login`。下面使用 DeepSeek 作为 local agent 的模型；它的 API key 与 Codex 登录分别配置。

创建仅自己可读写的密钥文件：

```sh
mkdir -p "$HOME/.config/xxassxx"
chmod 700 "$HOME/.config/xxassxx"
touch "$HOME/.config/xxassxx/secrets.env"
chmod 600 "$HOME/.config/xxassxx/secrets.env"
```

用本机文本编辑器打开 `~/.config/xxassxx/secrets.env`，填入这一行，将占位文字换成实际密钥；已有其他配置时保留原内容：

```dotenv
DEEPSEEK_API_KEY=你的密钥
```

邀请文件和密钥只保存在本机或私下交付，不贴进聊天、群消息或代码仓库。

### 3. 加入团队

将邀请路径换成自己的文件路径：

```sh
xxassxx client init --invite "$HOME/Downloads/alice.json" \
  --provider deepseek --model deepseek-flash \
  --model-secrets "$HOME/.config/xxassxx/secrets.env"

xxassxx client whoami
xxassxx client doctor --probe-model
```

`whoami` 应显示管理员分配的用户名和团队；检查结果应有 `ready: true`，模型的 `tool_roundtrip` 应为 `passed`。`--probe-model` 会实际调用模型并消耗少量额度；日常只检查连接和登录时，用 `xxassxx client doctor`。

初始化只做一次。已有个人端时保留原目录和身份，升级按下文操作。若 Codex 不在 `PATH` 中，初始化时用 `--codex /实际路径/codex` 指定。

### 4. 选择允许读取的目录并启动

将路径换成自己已经存在的工作目录：

```sh
xxassxx client roots add "$HOME/Research/project"
xxassxx client roots list
xxassxx open "$HOME/Research/project"
```

也可以先运行 `xxassxx`，在首次启动提示或界面中的 **Ctrl+R** 添加目录。白名单初始为空；加入后，Codex 可按你的请求或团队请求读取其中的文件并返回答案，无需每次重新授权。添加目录不会自动上传整个目录，但获准读取的文本可能进入模型上下文，回复会经过团队信箱。查找、查看和解释文件默认只读。明确要求写文件或运行程序时，执行方需要为这一项已确认任务单独开放工作目录；原文件仍只读。

进入界面后，直接输入请求，例如“找一下与 history matching 有关的文件”。联系同伴时输入 `@bob` 加请求正文，检查任务卡后按 Ctrl+S 确认，再开始协作；用户名可用 `xxassxx client contacts` 查看。

使用支持成员目录同步的信箱和客户端时，后台会自动从服务器更新团队名单，界面和 `@` 候选随之刷新。管理员登记新成员后，已有成员无需重新导入邀请或重启界面；离线成员也会保留在名单中。邀请里的联系人只是初始缓存，断网时使用上次同步的名单。旧客户端需要升级后才能自动同步。

| 操作 | 命令或快捷键 |
| --- | --- |
| 打开当前目录 | `xxassxx` |
| 查看成员状态 | Tab |
| 管理文件白名单 | Ctrl+R |
| 查看任务 | Ctrl+T |
| 查看帮助 | F1 |
| 退出界面，保留后台 | Ctrl+Q |
| 查看后台状态 | `xxassxx client status` |
| 只启动后台 | `xxassxx client start` |
| 停止后台 | `xxassxx client stop` |

个人端尚未配置开机自启；重启电脑后运行 `xxassxx` 或 `xxassxx client start`。接收并处理团队请求时，自己的电脑和后台需要在线。

### 让执行方写文件并运行程序

在任务里说明要修改什么、运行什么、保留哪些原文件；确认卡会明确提示需要写入或运行。执行成员在自己的终端会看到“有任务需要你允许”。按 **CtrlT → Enter** 查看任务、保存位置和可读取范围，再选 **允许并开始**。默认在自己的工作目录中新建任务文件夹，运行时限为 30 分钟；同一页面可以修改位置、添加只读依赖目录和调整时限。选择“暂时不允许”会保留任务，之后用 CtrlG 再处理。

允许后自动继续，发起方无需重试。发起方会看到正在等哪位成员允许执行，以及对方已允许的通知。未允许期间不会启动 Codex，也不会消耗执行重试次数。

需要通过命令操作时，也可在执行成员自己的电脑上运行：

```bash
xxassxx client tasks
xxassxx client task-access allow TASK_ID --revision 1 --parent /path/to/project --read-dir /path/to/runtime --timeout-secs 1800
```

用实际完整任务编号和当前版本替换示例值。`--parent` 必须位于原有可访问目录内；程序在其中新建独立任务目录，Codex 只能在这个新目录写入。额外的数据或软件目录可重复使用 `--read-dir` 添加，仅供这一项任务只读使用。原有白名单不变，不开放联网或安装软件。

使用命令开放后，同样自动继续。结果会返回原对话，输入、日志和较大的结果文件留在执行方；文件引用附带大小和校验值。中断后继续会先检查已有结果，不应把完成的程序盲目重跑。任务目标重开后需为新版本重新开放目录。

```bash
xxassxx client task-access list
xxassxx client task-access revoke TASK_ID --revision 1
```

撤销会阻止新的访问、停止在途执行并保留生成文件。再次执行请重开确认新版本，不覆盖旧目录。授权面板与上述命令使用相同的本地、逐任务、逐版本检查。

## 交换文件

0.3.0 Alpha 增加 **CtrlF** 附件入口：选择成员和文件后发送，查看接收状态、另存为，或使用收到的附件继续只读分析。任务也可以交付实际结果文件，执行方需单独允许导出。需要双方个人端和信箱均升级到 0.3.0 Alpha；旧版尚不支持附件。详见[附件操作说明](docs/file-transfer.md)。

## 团队部署

以下使用一台带 systemd 的 Linux 服务器、一个域名和 Caddy。服务器只运行团队信箱，不需要 Codex 登录或模型密钥。

准备工作：

- 在服务器按上面的方式安装 Linux 版 `xxassxx`，路径为部署用户的 `~/.local/bin/xxassxx`。
- 按[官方说明安装 Caddy](https://caddyserver.com/docs/install)，并启用它的系统服务。下文使用 `/etc/caddy/Caddyfile`。
- 将团队域名的 DNS 指向服务器，放通公网 TCP 80、443；信箱端口 7788 仅监听本机。
- 确定团队 ID、成员用户名和显示名。目前一个团队支持 2–9 名成员。

下文的 `team.example.com`、`lab`、`alice`、`bob` 都需要换成自己的值。除明确带 `sudo` 的命令外，使用同一个普通部署用户执行。

### 1. 创建团队与邀请

```sh
xxassxx server --directory "$HOME/.local/share/xxassxx/server" init \
  --team lab \
  --public-url https://team.example.com \
  --member 'alice=Alice' \
  --member 'bob=Bob'
```

每名成员重复一个 `--member 用户名=显示名`。团队 ID 和用户名可用字母、数字、短横线、下划线；用户名用于 `@` 联系成员。

默认生成以下文件：

```text
~/.local/share/xxassxx/server/
├── server.toml
├── secrets.env
└── invitations/
    ├── alice.json
    └── bob.json
```

这是首次部署命令。已有团队继续使用原目录，不要重新初始化或重新生成成员凭据。当前没有自助注册或成员增删命令，初始化前先确认名单。

信箱启动时将管理员配置的成员登记到持久化成员表，并向已鉴权的团队客户端同步用户名和显示名；不会下发他人的凭据。后续由管理员更新成员配置并重新加载信箱时，支持同步的客户端会自动取得新名单。更新名单不改变成员本地目录白名单、逐任务执行授权或附件导出许可。

### 2. 让信箱常驻运行

创建 `~/.config/systemd/user/xxassxx-mailbox.service`，父目录不存在时先创建：

```sh
mkdir -p "$HOME/.config/systemd/user"
```

文件内容：

```ini
[Unit]
Description=xXASSXx team mailbox
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=TOKIO_WORKER_THREADS=2
ExecStart=%h/.local/bin/xxassxx server --directory %h/.local/share/xxassxx/server serve
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
UMask=0077
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=default.target
```

启用服务，并允许它在退出 SSH 和服务器重启后继续运行：

```sh
sudo loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable --now xxassxx-mailbox
systemctl --user status xxassxx-mailbox
```

### 3. 配置 HTTPS

在 `/etc/caddy/Caddyfile` 中加入自己的域名配置，保留已有站点：

```caddyfile
team.example.com {
    reverse_proxy 127.0.0.1:7788
}
```

验证并加载配置：

```sh
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
sudo systemctl reload caddy
curl -i https://team.example.com/v1/whoami
```

未携带成员凭据的请求应返回 **401 Unauthorized**，说明 HTTPS 已到达信箱。实际身份验证在成员安装后的 `xxassxx client doctor` 中完成。

### 4. 分发并验证

把对应架构的安装包和该成员自己的邀请文件分别交给每个人。例如，Alice 只收到 `alice.json`，Bob 只收到 `bob.json`。不要把邀请或服务器的 `secrets.env` 放进公共安装包。

成员按本文“成员安装”完成配置后，各自运行 `xxassxx`。双方打开后台，并各自允许一个测试目录；由 Alice 输入 `@bob 请列出你允许读取的目录中有哪些文件`，在任务卡确认后检查 Bob 的结果回到 Alice 的原会话。成员不需要服务器 SSH 权限，也不需要与服务器处于同一局域网。

## 升级

目前需要手动更新。1.0.0 正式版前按小规模测试环境使用，可直接原位升级，保留现有数据和配置，无需重新初始化或重新导入邀请。

1. 先升级团队信箱：运行 `systemctl --user stop xxassxx-mailbox`，安装新程序后运行 `systemctl --user start xxassxx-mailbox`。
2. 成员退出界面，等待正在执行的任务结束，运行 `xxassxx client stop`，用 `xxassxx client status` 确认 `alive: false`。
3. 用匹配本机架构的新安装包重新执行 `install.sh --from .`，或安装本机源码构建的新程序。
4. 运行 `xxassxx client doctor`，然后用 `xxassxx open /自己的工作目录` 打开界面。工作目录用 Ctrl+P 切换，允许读取的目录用 Ctrl+R 单独管理。

当前个人数据库版本为 9，首次打开自动迁移并保留历史记录；未完成的旧任务会暂停，检查执行记录后再显式恢复。旧程序不能读取 schema 9，不能只换回旧程序或手工调低数据库版本。成员数据默认位于 `~/.local/share/xxassxx/client/`，团队数据位于 `~/.local/share/xxassxx/server/`；不要删除后重建。

## 遇到问题

| 问题 | 检查方式 |
| --- | --- |
| 找不到 `xxassxx` | 确认 `~/.local/bin` 在当前终端的 `PATH` 中 |
| 初始化提示目录已存在 | 先用 `xxassxx client whoami` 检查已有身份，保留数据，不要删除后重建 |
| Codex 未登录或不可用 | 运行 `codex --version`、`codex login status`，必要时登录 |
| 模型无法使用 | 检查密钥文件和模型名称，再运行 `xxassxx client doctor --probe-model` |
| 找不到业务文件 | 用 `xxassxx client roots list` 检查文件所在目录是否获准读取 |
| 同伴没有回复 | 双方检查 `xxassxx client status`、`xxassxx client doctor`；后台未运行时执行 `xxassxx client start` |
| 信箱连接失败 | 服务器查看 `journalctl --user -u xxassxx-mailbox -n 100` 和 `sudo journalctl -u caddy -n 100`，检查 DNS 与 80/443 端口 |

个人端后台日志默认位于 `~/.local/share/xxassxx/client/member.sqlite3.service/butler.log`。
