# xXASSXx · 0.1.0 Alpha

首个 Alpha 版本，程序版本号为 `0.1.0-alpha`，对应发布标签 `v0.1.0-alpha`。功能与限制见 [首版说明](docs/releases/v0.1.0-alpha.md)，构建和安装见 [版本构建说明](docs/releasing.md)。各阶段文档保留开发与验收历史。

本机 Apple Silicon Mac 安装包位于 `dist/v0.1.0-alpha/`，离线安装：

```sh
sh dist/v0.1.0-alpha/install.sh --from dist/v0.1.0-alpha
```

本地不可变知识版本、独立成员 local agent 之间的持久通信，以及经过任务校验的 Codex 委托闭环。面向十人以下的小团队。

```text
Codex A ↔ local agent A（自己的 SQLite / 内容库）
                  ↕ HTTPS
             团队信箱（SQLite）
                  ↕ HTTPS
Codex B ↔ local agent B（自己的 SQLite / 内容库）
```

Rust 内核处理存储、校验、通信和执行状态；local agent 收集意图、上下文和澄清，协调成员，将大部分实质任务交给 Codex。Codex 使用自身原生工具处理任务，xXASSXx 不重建通用文件搜索工具链。DeepSeek 为 local agent 提供推理能力，Codex 仍使用用户现有的官方登录和执行配置。两者独立。

**第三阶段已通过验收：**持久事件调度、按明确会话 ID 续接、任务范围的固定版本正文读取、受限澄清/回信、最终 JSON 保存和 local agent 服务管理已完成验证。macOS/Linux 59 项默认测试通过；2026-10-06，用户选定的“实验数据质控”案例在 Trader/Lakota 使用真实 Codex + DeepSeek 首次运行通过，独立检查确认 8 个样本中 4 个通过，S02、S03、S06、S08 需复测，文件读取、原会话续接和清理证据完整。该次第三阶段验收依赖 Mac 的 SSH 隧道；第四阶段现已完成独立公网 HTTPS 验证。操作见 [第三阶段使用说明](docs/phase3.md)，证据见 [第三阶段验证记录](docs/phase3-validation.md)。

第二阶段历史结果：47 项默认测试在 macOS/Linux 通过；跨机器验证使用模拟 Codex；真实 DeepSeek 单独通过。历史记录见 [跨机器验证](docs/cross-machine.md)、[第二阶段验证](docs/phase2-validation.md) 和 [第一阶段验证](docs/validation.md)，不替代第三阶段真实双模型验收。

**第四阶段安装与公网通信已验证：**同一个 Rust 程序提供 `server` 与 `client` 两种入口，支持私人邀请文件、个人端初始化、DeepSeek/Ollama 配置、显式模型工具探针和临时断网重连。Trader 的 HTTPS 常驻信箱已上线，同一质控任务通过公网完成 3+2 轮真实 Codex 往返及独立验收。安装包与个人端配置见 [第四阶段说明](docs/phase4.md)，测试与部署证据见 [验证记录](docs/phase4-validation.md)。Apple Silicon Mac 可先使用本地安装包；GitHub Release 尚未发布，在线安装入口暂不可用。启动时检查更新仍是待办。

个人端可从任意目录启动；业务文件来源由 `client init --allow-dir` 或 `client roots add/list/remove` 显式选择，默认空白名单。添加目录不自动上传文件；Codex 可在这些目录内按本地或团队请求只读查询，返回答案和相对文件引用。`client whoami` 显示邀请绑定的用户名、显示名称与团队。最新 macOS 回归 71 项通过，详见 [目录与身份说明](docs/phase4.md)。

## 打开自己的工作终端

初始化个人端后，`xxassxx` 直接打开自己的 Rust 终端界面，连接/启动后台 local agent。普通输入给自己的 local agent，`@test` 明确联系 test local agent；Tab 只查看成员，不改接收方。启动界面不调用模型或 Codex。

```sh
xxassxx                              # 当前项目的 TUI
xxassxx open "$HOME/Research/project"
xxassxx -C "$HOME/Research/project"
xxassxx codex "$HOME/Research/project" # 显式保留官方 Codex + 临时 local agent MCP
```

聊天、任务、未读和来源持久保存。首次在未授权的目录启动时，会询问是否加入文件白名单并记住选择；以后用 CtrlR 管理。CtrlO 查看会话，CtrlT 选择任务，CtrlG 逐文件授权，CtrlD 看详情，F1 看帮助。@ 请求不能扩大对方已有的白名单。退出界面后后台继续工作；`xxassxx client status` 查看后台，`xxassxx client stop` 请求停止，`xxassxx client start` 只启动后台。个人端尚未配置开机自启。

直接输入“找一下与 history matching 有关的文件”，或“@test 请找 scripts 中与 misfit 有关的文件，告诉我路径和相关原因”。local agent 保存原始意图后委托 Codex，结果回到原会话，不要求 object ID 或先导入文件。该通用委托目前支持白名单内只读查找、查看和解释；修改文件与执行项目程序尚未启用。原有固定快照、逐文件授权的任务流程继续保留。

原生工具任务使用 Codex 的命名权限配置：仅允许既有白名单和必要运行文件的读取，禁止写入和命令联网，失败不降级到无限制模式。实现依据 [官方权限说明](https://learn.chatgpt.com/docs/permissions)。本次新增数据库 schema 6；升级时应停止旧后台并备份个人端数据，旧二进制不能直接读取升级后的数据库。

从仓库启动两个隔离个人端和本地信箱的真实体验：

```sh
cargo build --locked
python3 scripts/phase5_demo.py --real
```

脚本只创建测试目录和合成材料，复用现有私有模型配置与官方 Codex 登录；另一个终端可运行输出的 `open-test.sh`。可以输入“这轮讨论的代号是银杏”，再追问“刚才的代号是什么？”；联系同伴可输入 `@test 请说明你有哪些已共享材料。只列元数据。`。文件阅读任务先等待主人通过界面选择文件，跨端授权也无需手工拼底层 ID。

完整操作、应用层测试适配器和限制见 [Phase 5 说明](docs/phase5.md)，本次模拟、实际 PTY 和真实模型证据见 [Phase 5 验证](docs/phase5-validation.md)。未安装到正式个人端，未更新 Mac/Lakota/Trader 常驻服务，未发布 Release。下文第二至第四阶段说明保留为底层功能与历史记录；旧默认 Codex 启动方式已迁移到显式 `codex` 子命令。

## 开发构建与演示

需要 macOS/Linux 和 Rust 工具链；测试和演示还需要 Python 3.11+。当前实测 macOS arm64、Linux x86_64、Rust 1.99.0。无需安装本地模型。

```sh
cargo build --locked
cargo test --locked                # 默认不访问真实模型，两个真实测试 ignored
python3 scripts/demo.py            # 启动实际信箱、独立 A/B 常驻进程、模拟模型与 Codex
```

演示发布 V1/V2、比较和取回 V1；让 B 离线后领取请求，再对 A 的追问创建 Codex 委托，经真实 MCP 提交结果，最后从 A 的 local agent MCP 读回。它不直接修改业务数据库。输出 `smoke-output/phase2-demo-UUID/report.json`，同目录保留 A/B 独立数据库、信箱、内容库和进程日志。完成后自动停止演示进程；再次运行创建新目录。

如果本机尚未安装 Rust，可用本次验证留下的临时工具链，位置与命令见 [环境说明](docs/validation.md#环境与复现)；临时目录可能被系统清理。

两台机器的演示使用 `scripts/ssh_demo.py` 和已有 SSH 别名，不需要开放业务公网端口；控制端负责建立 SSH 隧道，结束后自动停止测试服务。部署路径、复跑命令、实际证据和当前限制见 [两台 Linux 机器上的验证](docs/cross-machine.md)。

## 一台电脑上的手动操作

以下命令在仓库根目录、同一终端执行。`a`、`b` 是本例的便捷函数，所有业务也能直接用 `xxassxx --db PATH ...`。路径应当是新目录，以免和已有演示混用。

```sh
export XXASSXX_CLI="$PWD/target/debug/xxassxx"
export XXASSXX_DEMO="$PWD/.xxassxx/team-demo"
mkdir -p "$XXASSXX_DEMO/a/work" "$XXASSXX_DEMO/b/work" "$XXASSXX_DEMO/dataset"
cp examples/mailbox.toml "$XXASSXX_DEMO/mailbox.toml"
cp examples/member-a.toml "$XXASSXX_DEMO/a/member.toml"
cp examples/member-b.toml "$XXASSXX_DEMO/b/member.toml"

# 独立成员凭据，仅通过进程环境提供。不要将实际值放入配置或提交到仓库。
export TEAM_A_TOKEN="$(openssl rand -hex 24)"
export TEAM_B_TOKEN="$(openssl rand -hex 24)"

a() { "$XXASSXX_CLI" --db "$XXASSXX_DEMO/a/tasks.sqlite3" "$@"; }
b() { "$XXASSXX_CLI" --db "$XXASSXX_DEMO/b/tasks.sqlite3" "$@"; }
id() { python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'; }
mid() { python3 -c 'import json,sys; print(json.load(sys.stdin)["message"]["message_id"])'; }

a member init --id a --name Alice --team lab
b member init --id b --name Bob --team lab
a member configure "$XXASSXX_DEMO/a/member.toml"
b member configure "$XXASSXX_DEMO/b/member.toml"

"$XXASSXX_CLI" --db "$XXASSXX_DEMO/relay.sqlite3" mailbox serve \
  --config "$XXASSXX_DEMO/mailbox.toml" --listen 127.0.0.1:7788 \
  >"$XXASSXX_DEMO/mailbox.log" 2>&1 &
XXASSXX_RELAY_PID=$!
```

信箱就绪后日志会出现 `listening`。若端口占用，可同时修改监听地址和两个成员配置中的 `mailbox_url`，然后重新 `member configure`。示例联系人双向配置，local agent 模型默认 `off`、执行器默认 `queue`。

成员/团队 ID 一旦初始化不能替换；同一身份可更新显示名和连接配置。`member configure` 将 TOML 配置保存到该成员数据库，修改文件后必须再次执行。`workdir` 和带目录的 `codex` 相对路径以配置文件目录解析，启动时保存为绝对路径。配置文件只包含**凭据的环境变量名称**。

### 数据集两版、比较、校验、取回

```sh
printf 'sample,value\na,1\n' > "$XXASSXX_DEMO/dataset/data.csv"
mkdir "$XXASSXX_DEMO/dataset/empty"
b roots add "$XXASSXX_DEMO/dataset"
OBJECT_ID=$(b object create --title '实验数据集' --kind dataset --shared | id)
V1=$(b object publish "$OBJECT_ID" --expected none --request-id dataset-v1 \
  --note '初版' --source "$XXASSXX_DEMO/dataset" | id)

printf 'sample,value\na,1\nb,2\n' > "$XXASSXX_DEMO/dataset/data.csv"
printf 'measurement units\n' > "$XXASSXX_DEMO/dataset/units.txt"
V2=$(b object publish "$OBJECT_ID" --expected "$V1" --request-id dataset-v2 \
  --note '补充样本与说明' --source "$XXASSXX_DEMO/dataset" | id)

b object show "$OBJECT_ID"
b object history "$OBJECT_ID"
b version show "$V2"               # 包含相对路径、字节数、SHA-256 清单
b version diff "$V1" "$V2"          # added / removed / changed
b version verify "$V1"
b version verify "$V2"
b version export "$V1" "$XXASSXX_DEMO/restored-v1"  # 目标必须不存在
cat "$XXASSXX_DEMO/restored-v1/data.csv"

DECISION_ID=$(b object create --title '研究决策' --kind decision | id)
DECISION_V1=$(b object publish "$DECISION_ID" --expected none --request-id decision-v1 \
  --note '选择分析方法' --text '保留原始数据，使用第二版作为分析输入。' | id)
b version text "$DECISION_V1"       # JSON 字符串形式返回原文
```

对象默认私有；只有 `--shared` 或 `object share ID` 明确开放后，同伴请求才能查询其元数据。`object share ID --private` 关闭后续查询，不能撤回已经发送的消息。共享清单会包含相对文件名和发布说明，请按实际需要开放。

每个版本最多一个父版本，必须属于同一对象；`--expected none` 表示显式预期尚未发布，否则传旧 main 的 ID。内容先可靠落盘，随后在同一个 SQLite 事务内创建版本并比较更新 main；并发发布只有一个胜者。同一 `request_id`、对象、父版本、发布说明和内容重试返回原版本；不同请求复用该 ID 会冲突。重试要继续使用原请求的参数，不要把 `--expected` 改成新 main。

源文件修改、移动或删除不会改变快照。内容流式读写、按 SHA-256 复用；版本 ID 与内容哈希分开。内容库固定为数据库旁的 `<数据库文件名>.content/`，与启动目录无关。空目录作为 `sha256: null, bytes: 0` 的清单项保存。符号链接、特殊文件、不安全路径和非 UTF-8 文件名会报错；禁止把自身数据库、WAL/SHM、内容库、任务锁及包含它们的目录收进快照。

发现读取期间源文件/目录变化会提示重试，但不保证正在写入的整棵目录具有原子快照。检查包含 ctime；macOS 等系统后台更新文件属性，也可能触发保守拒绝。此时停止写入，按原 `--expected` 和 `--request-id` 重试失败的发布即可，已完成的旧版本不受影响。文件内容不保留权限位、属主或时间戳。每个清单最多 10000 项，文字记录最多 1 MiB。校验会检查全部 blob；导出先校验且只写不存在的新目录，拒绝路径逃逸和覆盖。磁盘错误或导出期间内容损坏可能留下新的不完整导出目录，可检查后删除；不会覆盖原文件。失败发布允许留下未引用 blob，暂不做垃圾回收。

### 离线请求、回复与追问

先保持 B 没有运行 local agent 进程。请求先进入 A 的本地发件箱，再经 HTTP 到信箱；不读取 B 的本地数据库。

```sh
REQUEST_ID=$(a message send --to b --body '请告诉我这个数据集当前发布的版本与清单。' \
  --operation metadata --object-id "$OBJECT_ID" | mid)
a butler sync
a message show "$REQUEST_ID"       # sent：信箱已持久接收，尚非 B 完成

# B 上线，显式处理一轮；明确 metadata 请求由 Rust 回答，不调用模型。
b butler tick
a butler sync
a message inbox
CONVERSATION_ID=$(a message show "$REQUEST_ID" | python3 -c \
  'import json,sys; print(json.load(sys.stdin)["message"]["conversation_id"])')
a conversation show "$CONVERSATION_ID"
REPLY_ID=$(a message inbox | python3 -c \
  'import json,sys; print(json.load(sys.stdin)[-1]["message"]["message_id"])')

# 追问沿用同一会话，默认继承被回复消息的对象与已固定的 V2。
FOLLOWUP_ID=$(a message send --to b --reply-to "$REPLY_ID" \
  --body '请对这版数据提供专业分析，信息不足时说明限制。' --operation analysis | mid)
a butler sync
b butler tick
b delegation status "$FOLLOWUP_ID" # queued；还没有 Codex 完成结果
```

省略 `version_id` 的请求在 B 实际处理时固定具体版本并保存 `resolved_version`，后续发布不会悄悄改掉已固定的分析输入。回复保存 `reply_to`、来源成员、对象和版本；重复 `message_id` 的相同负载只入箱一次，不同负载冲突。需要生产者重试时，提前生成 UUID 并传 `message send --message-id UUID`。

人工回复也可以用 `b message reply REQUEST_ID --body '回复正文'`，随后 `b butler sync; a butler sync`。每个请求只接受一个有效回复，相同内容幂等重试，内容冲突拒绝覆盖。自动 Codex 回复由执行器写入，不把人工回复或模型摘要当作 Codex 成功证据。

`conversation close ID` 在本地关闭会话；后续收到的消息仍保存，但不自动处理或继续发起请求。这不是向对方发送的撤回通知。收件确认、普通状态查询、收到回复本身均不触发新的模型讨论。

### 执行 Codex 委托与恢复

```sh
codex --version
codex login status                 # 使用已有官方登录；未登录时通过官方 codex login 登录
b delegation run "$FOLLOWUP_ID"
b delegation status "$FOLLOWUP_ID"
b butler sync
a butler sync
a conversation show "$CONVERSATION_ID"
```

委托记录关联原消息 → 唯一任务 → 多次执行 → 官方会话 ID。执行复用第一阶段的独立监护进程、任务文件锁和 `run-once`。同一请求重试不新建任务；失败后再次 `delegation run` 自动使用已保存的 ID 恢复，成功任务不重跑。每个任务和每个委托串行执行，绝不用 `--last`。

只有任务 MCP 初始化、读取回执、有效结果、匹配的会话 ID、完成事件和正常进程退出都通过验收，才生成含 `codex_task_id`、`codex_session_id` 和明确版本的回复。提交结果后进程失败仍然失败。通过 `delegation status` 查看各次执行诊断，用 `show TASK_ID --events` 查看原始 Codex 事件。

执行器保留第一阶段的只读沙箱、禁止 shell 和固定工作目录，本阶段复杂委托使用请求及已授权元数据分析，**没有自动赋予任意文件处理/代码修改权限**。数据不足时应返回限制；文件内容不会自动发送给 local agent 模型或另一成员。原有 `init / submit / list / show / run-once / mcp-serve` 均保留，详见 [第一阶段操作参考](docs/phase1.md)。

### 两个常驻 local agent

在上面的同一环境中执行；也可以在分别配置好各自环境变量的终端以前台运行。

```sh
"$XXASSXX_CLI" --db "$XXASSXX_DEMO/a/tasks.sqlite3" butler run --poll-secs 2 >"$XXASSXX_DEMO/a/butler.log" 2>&1 &
XXASSXX_A_PID=$!
"$XXASSXX_CLI" --db "$XXASSXX_DEMO/b/tasks.sqlite3" butler run --poll-secs 2 >"$XXASSXX_DEMO/b/butler.log" 2>&1 &
XXASSXX_B_PID=$!

# 模拟 B 离线：停止 B 后照常从 A 发送消息，信箱会保留。
kill -INT "$XXASSXX_B_PID"
wait "$XXASSXX_B_PID"
# 恢复时继续使用同一个数据库和身份。
"$XXASSXX_CLI" --db "$XXASSXX_DEMO/b/tasks.sqlite3" butler run --poll-secs 2 >"$XXASSXX_DEMO/b/butler.log" 2>&1 &
XXASSXX_B_PID=$!

# 用完后停止三个进程；数据库保留。
kill -INT "$XXASSXX_A_PID" "$XXASSXX_B_PID" "$XXASSXX_RELAY_PID"
```

local agent 依次同步、处理请求、同步回复；不安装系统服务。将成员 TOML 的 `[executor] mode` 改为 `"auto"` 再 `member configure`，新委托即可自动执行。`"queue"` 保留人工 `delegation run` 入口。自动执行会使用官方 Codex 额度。失败请求保留为 `failed`，修复后用 `butler process MESSAGE_ID` 或 `delegation run MESSAGE_ID` 显式重试，避免不断消耗模型额度。

## local agent 模型配置

基础通信、按 ID 查询、明确 `--operation metadata` 和 `--operation analysis` 都能在模型关闭时工作。默认 `--operation auto` 才需要模型理解自然语言；关闭状态会保存可见的 `model_disabled`，不会伪造回复。

修改 **B 的成员配置**中的 `[model]`（替换原来的表，不重复追加）：

```toml
[model]
provider = "deepseek"
base_url = "https://api.deepseek.com"
model = "deepseek-flash"       # 可配置；使用账号可用的、支持工具调用的模型
api_key_env = "DEEPSEEK_API_KEY"
timeout_secs = 30
max_model_calls = 4
max_tool_rounds = 3
max_tool_calls = 12
max_tokens = 2048
thinking = false
```

使用安全方式在运行 local agent 的进程环境中设置 `DEEPSEEK_API_KEY`，例如已配置的本机密钥管理工具或终端隐藏输入；不要粘贴到聊天、写入 TOML 或 Git。若使用 `DEEPSEEK_API` 这个名称，将配置改为 `api_key_env = "DEEPSEEK_API"`。`.env` 文件不会自动成为进程环境，需要由启动环境安全加载；本程序不会执行 `.env` 内容。然后重新 `b member configure ...`，重启没有继承新环境的常驻 local agent。`b butler model-history MESSAGE_ID` 可查看调用次数、工具调用 ID、参数/结果、附加摘要和错误。模型名称不写死在业务逻辑中。

实现使用真实 HTTP `POST /chat/completions`，按 DeepSeek 官方 [工具调用](https://api-docs.deepseek.com/guides/tool_calls/) 和 [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) 规范保留每个 `tool_call_id` 的结果对应及工具轮次所需的 `reasoning_content`。默认关闭 thinking；没有自动降级到另一个模型。

模型仅可使用联系人、当前请求相关且已开放的知识元数据、相关会话、向请求者追问、提交回复、创建/查询本机委托七类工具。Rust 验证参数、对象范围、版本、联系人、状态与闭合会话；不提供 shell、文件、代码执行或 SQL 工具。上下文保留最多 12 条相关原始消息，超过 128 KiB 报错；清单每页最多 100 项，不读取 blob 正文。模型摘要独立存储，不替换原文或冒充对方回复。

缺 key、鉴权错误、额度/速率限制、请求超时、无效响应、无效工具参数和轮数上限均可见。无效工具参数返回给模型修正并记录；如最终没有有效业务动作，处理失败。已经提交的有效业务动作不会因随后模型总结失败而回滚。完整供应商错误响应、Authorization 和实际 key 不进入日志或业务记录。

本地模型配置示例：

```toml
[model]
provider = "compatible"
base_url = "http://127.0.0.1:8000/v1"
model = "YOUR_LOCAL_MODEL"
api_key_env = ""              # 仅不要求鉴权的本地服务可留空
max_tool_rounds = 3
```

支持范围是非流式、文本 Chat Completions、`tools`/function calling、字符串 JSON 参数和 `tool_call_id`。服务必须实现这些能力；不支持 SSE、图片/音频、Responses API 或厂商特有格式。`compatible` 不发送 DeepSeek 专用 `thinking` 参数。已测兼容 mock HTTP 服务，未安装本地模型，也未宣称 vLLM、Ollama 等具体服务已实测。

## MCP 与权限范围

CLI 和两种 stdio MCP 共用 Rust 业务逻辑；不自动修改用户全局 Codex 配置。[examples/codex-mcp.toml](examples/codex-mcp.toml) 给出主人本人使用的配置示例，替换绝对路径并按需接入。

| 入口 | 能力与限制 |
| --- | --- |
| `mcp-serve --task-id ... --run-id ...` | 第一阶段临时执行授权；仅 `get_task` / `submit_task_result`，严格校验 task/run、读取回执和执行状态 |
| `butler mcp-serve` | 受信任的主人本地能力；查询自己全部对象元数据、发送联系人请求、读取会话/回复、查询状态和同步信箱；不能提交任务结果 |
| local agent LLM 业务工具 | 当前同伴请求范围；可列明确 shared 的材料元数据，指定对象请求仍限制在绑定对象，不能读正文、私有对象或联系第三方 |

`run-once` 为配置了成员的**主人本地任务**临时注册独立的 `xxassxx_butler` 服务；来自同伴消息的委托只得到任务 MCP 及已授权上下文，不注入主人全部元数据/会话能力。local agent 模型 key 不传入实际 Codex 执行进程。

local agent MCP 工具为 `butler_context`、`knowledge_objects`、`knowledge_metadata`、`collaboration_send_request`、`collaboration_read_conversation`、`collaboration_read_replies`、`collaboration_status`、`collaboration_sync`。发送只写入发件箱，运行中的 local agent 或 `collaboration_sync` 完成投递。发起方 Codex 通过 `collaboration_read_replies(message_id)` 可靠读回；不向正在进行的 Codex 回合强行插入消息。

主人本地 MCP 依赖本机进程权限，不是面向远程同伴的服务。不要把它暴露给不受信任客户端。宿主能直接读写 SQLite 的程序仍属于本地信任域，执行授权不抵御恶意同用户进程。

## 持久化、通信与跨机器

主数据库增量迁移到 schema 5；重复 `init`/`member init` 不清空任务、结果、执行、事件和会话。新增表分别保存对象/不可变版本、身份/联系人、原始消息/会话、委托和模型运行/工具记录。Codex `events` 继续独立保存。

发送方先持久化发件箱；信箱按成员凭据确定发送身份，只允许读取、确认自己的收件。接收方提交本地去重记录后才 ACK；ACK 丢失时可重复投递。消息状态区分 `pending`（本地待发送）、`sent`（信箱已接收）、`received`（对方落盘）、`waiting`（待处理）、`delegated`、`awaiting_peer`、`replied`、`failed` 和 `closed`。`received` 不能证明 Codex 完成。不声称跨进程 exactly-once。

静态信箱配置支持 2–9 个成员及各自至少 16 字节、不同的凭据；数据库仅保存凭据摘要。重新启动信箱会按配置更新成员和凭据，消息保留。正常重启要继续提供原来的凭据环境，不要重新生成随机 token 后忘记更新客户端。

跨机器时，将信箱放在团队可达的主机，配置监听地址、成员的 `mailbox_url`、各自环境凭据和联系人。Rust 服务提供 HTTP，生产跨机器连接需在外层配置 HTTPS 反向代理；受信任 VPN/内网若明确接受明文，可设置 `allow_insecure_http = true`。非回环 HTTP 默认拒绝，禁止 URL 内嵌凭据且不跟随重定向。没有内置证书、注册平台或多租户管理。

信箱只传消息和对象/版本元数据，没有原始文件、模型密钥或本地路径字段，不上传文件全文；明显的绝对路径文本会被拒绝。自由正文的检查属于防误传，不能识别所有编码/嵌入式敏感内容，不是完整 DLP。

## 测试与真实冒烟入口

```sh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
python3 scripts/demo.py

# 显式启用真实 DeepSeek：使用进程环境的 DEEPSEEK_API_KEY 和实际额度。
# 可选 DEEPSEEK_MODEL 选择模型；默认使用文档示例模型。
cargo test --locked --test real_deepseek -- --ignored --nocapture
# 若已导出的环境变量名为 DEEPSEEK_API，可只在这次测试中映射：
# DEEPSEEK_API_KEY="$DEEPSEEK_API" cargo test --locked --test real_deepseek -- --ignored --nocapture

# 独立的真实 Codex 接入与按保存 ID 恢复，两轮实际模型调用。
cargo test --locked --test real_codex -- --ignored --nocapture
# 可选 XXASSXX_REAL_CODEX=/path/to/codex XXASSXX_REAL_MODEL=MODEL
```

测试要能绑定回环 HTTP 端口。DeepSeek 冒烟验证自然语言请求 → 实际工具调用 → 元数据查询 → 持久回复 → HTTP 回到 A，并保存 `smoke-output/deepseek-UUID/report.json`。无 key 时明确打印 `NOT RUN` 并写入 `status: not_run`；测试框架此时返回正常结束不代表真实验证通过。真实测试失败或中断不会写入 `passed`。证据目录默认被 Git 忽略。

## 本阶段限制

- macOS/Linux 默认测试和第三阶段真实双机质控验收已通过：Trader/Lakota 分别使用官方 Codex 0.159.3/0.159.2，两端 local agent 均使用真实 `deepseek-flash`，自动完成澄清、回答、分析及汇总。第三阶段证据使用 SSH 隧道；第四阶段另有公网验证。Phase 5 本次只在本机两个隔离个人端验收，未升级这些服务；Windows 不支持。
- 元数据和消息跨成员，原始内容只在所属成员本地；没有跨机器文件传输、共同编辑、全文/向量搜索、GUI、文件监听或语义合并。
- 同伴任务在单个执行循环中串行处理，本人对话与收件/心跳分别运行；一次模型调用有超时和工具预算，失败需显式重试。没有消息过期、后台压缩/清理、完整退避策略或高可用信箱。大量历史消息的查询/轮询还需分页优化。
- 关闭会话、撤销共享不能撤回已发送信息；已经开始的外部模型调用也无法撤回。每次后续工具调用仍检查当前会话和共享权限。
- 委托的“成功”证明身份、结果持久性和执行完整性，不判断科研结论的语义正确性；默认元数据上下文不够时必须说明局限。消息正文最多 32 KiB，超过此长度的 Codex 结果不会自动发往同伴，会留下可见的回复失败状态。
- 内容落盘与 SQLite 依赖本机可靠文件系统；备份应同时包含数据库、内容库及官方 Codex 自己的会话存储。项目不读取或复制 Codex 凭据；会话文件由用户删除后无法恢复，不能以新会话冒充。

## 已记录的后续需求

- **启动时检查 GitHub 更新（待实现，2026-10-06）：**面向从 GitHub 安装的团队成员，每次应用启动时检查是否有新版本，并向用户提示。本次仅记录需求；版本发布、安装和更新机制后续一起设计。检查失败不应阻止正常启动，提示不应干扰 MCP 协议输出；自动下载或安装更新不属于当前已确定的需求。
