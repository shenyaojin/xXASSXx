# Phase 5：独立终端与持久 local agent 对话

`xxassxx` 现在直接打开 Rust TUI。前台提交持久指令、展示会话和任务；后台负责模型、信箱和执行。无需先进入 Codex，也不用粘贴管理提示词。原始数据库、消息、知识版本、任务授权和会话记录保留，schema 自动增量迁移到 5。

## 启动与独立体验

```sh
cargo build --locked
python3 scripts/phase5_demo.py --real
```

第二条命令创建全新临时目录、两个隔离个人端 `owner` / `test`、回环测试信箱和两份合成材料，然后打开 owner 的 TUI。它复用 `~/.local/share/xxassxx/client/member.toml` 的模型配置路径及官方 Codex 登录，不读取/复制 Codex 凭据；模型密钥仍由 Rust 按需从现有私有 `model.secrets_file` 读取。启动环境本身不调用模型。可用 `XXASSXX_DEMO_MODEL_CONFIG` 指向另一份个人端配置。没有私有模型配置时，省略 `--real` 可体验模型关闭的界面、身份与状态。

脚本输出 `open-test.sh` 和 `stop.sh` 的绝对路径。在第二个终端运行前者即可作为测试成员本人操作。两个终端退出后后台仍运行；运行后者停止**这一个隔离环境**。目录保存聊天与结果，可再次运行其中的 `open-owner.sh` / `open-test.sh`。不会升级真实 Mac/Lakota/Trader 服务，不会安装或发布程序。临时目录可能被系统清理；长期体验可用 `--root /ABS/NEW_DIRECTORY`。

已有个人端：

```sh
xxassxx                         # 当前目录是项目，个人数据库仍使用保存的默认位置
xxassxx -C /ABS/PROJECT
xxassxx open /ABS/PROJECT
xxassxx open /ABS/PROJECT --directory /ABS/EXISTING_CLIENT
xxassxx open /ABS/PROJECT --check   # 不启动服务、Codex 或模型
xxassxx codex /ABS/PROJECT         # 显式使用原来的 Codex + 临时 local agent MCP 入口
```

首次安装仍用 `client init`。不要拿项目目录重新初始化个人身份。未配置个人端、未配置模型/缺少私有密钥、Codex 未安装/未登录等有明确错误；未安装 Codex 不妨碍打开 TUI 和普通 local agent 聊天。本人对话的 DeepSeek 配置需 `thinking=false`；模型必须支持 `tool_choice=required` 和工具结果消费。已核对 [DeepSeek 官方接口](https://api-docs.deepseek.com/api/create-chat-completion/) 的工具模式限制。

首次在尚未授权的目录打开 TUI，会显示完整路径并询问是否加入本机文件白名单。用方向键选择并按 Enter，或按 y 加入、n/Esc 不加入。已有白名单覆盖的目录不再询问；拒绝选择按项目记住，以后可用 CtrlR 修改。选择加入只保存导入范围，不自动扫描、共享或启动分析；具体文件仍由 CtrlG 为任务选择。`--allow-import` 表示显式加入，`--check` 不弹提示、不保存选择。

个人端有前台界面和后台 local agent：`xxassxx` 打开界面并启动或复用该身份的后台，退出界面后后台继续处理收件与任务。服务器上只需常驻个人 local agent 时可运行 `xxassxx client start`；`client status` 查看状态，`client stop` 请求停止，重新 `start` 可继续收件。普通 SSH 断开不要求保留界面；主机休眠、重启或系统清理用户进程会中断后台，个人端目前没有自动安装开机服务。模型与 Codex 按需调用，常驻的是 Rust local agent 进程。Trader 团队信箱是独立服务，负责转发消息，不代替各成员的个人 local agent。

## 可以直接输入

- `你是谁？`：本地读取身份，不消耗模型额度。
- `这轮讨论的代号是银杏。先不要建任务，记住它。`，随后 `刚才的代号是什么？`：真实持久多轮对话。
- `@test 请说明你有哪些已共享、可供协作的材料。只列元数据。`：发送给 test local agent，实际回信回到同一会话。
- `请创建只读任务，分析我将授权的 tiny.csv，计算 amount 总和并引用来源，不运行脚本。`：创建等待授权的任务卡。
- 选中待授权任务后输入 `只看那个文件，不运行。`：把补充保存到该任务；没有明确关联时 local agent 应澄清。

只有开头独立的 `@member_id` 用于寻址；邮件、引号里的 @ 是正文。未知成员或多个外部 @ 被拒绝。候选只有本人及联系人 local agent，没有 Codex。无 @ 时始终给本人，查看别人的卡片或历史不会暗改接收方。

## 界面与快捷键

空白的本人对话面板居中显示“今天要做什么？”。顶部显示完整显示名、通信用户名和项目；大窗口左边显示 local agent/任务，小窗口自动收起侧栏。主聊天只投影用户消息、local agent 回复、授权请求、进展、结果和失败。原始记录按需展开。

| 操作 | 按键 |
| --- | --- |
| 查看自己/同伴/最近任务卡，不改变收件人 | Tab / ShiftTab |
| @ 候选选择；候选打开时 Tab 只确认，不发送 | 输入 @，上下，Tab |
| 发送 / 换行 | Enter / CtrlJ 或 AltEnter |
| 编辑 | 方向键、Home、End、Backspace、Delete |
| 历史滚动 | PageUp / PageDown、滚轮 |
| 列出全部持久会话及未读，查看历史 | CtrlO，方向键，Enter |
| 选择输入关联的任务 | CtrlT，方向键，Enter |
| 解除任务关联 / 关闭面板 | Esc |
| 创建明确的只读任务，不调用理解模型 | CtrlN，输入目标，Enter |
| 来源、执行记录、原始往返、结果文件和错误 | CtrlD |
| 选择项目 | CtrlP；n 输入新路径，a 明确允许该目录导入 |
| 管理导入白名单 | CtrlR；n 添加，Delete 移除 |
| 为所选任务挑具体文件 | CtrlG；Enter 进入目录，空格勾选，CtrlS 授权 |
| 确认采用同伴准备的固定材料 | CtrlU |
| 停止本端后续调度，需确认 | CtrlX，Enter |
| 显式重试原任务，沿用会话/授权 | CtrlY（仅状态机允许时） |
| 帮助 / 退出界面 | F1 / CtrlQ 或 CtrlC |

粘贴不会自动发送；编辑按 Unicode 字素处理，宽字符按终端列宽显示。退出、渲染错误、panic、SIGINT、SIGTERM 都恢复 raw mode、alternate screen、光标和粘贴/鼠标模式。终端崩溃、SIGKILL、断电无法执行清理代码。

## 文件授权的正常路径

选择项目与允许导入是两回事。加目录不会扫描、导入或共享；只有打开文件选择器才列出一层目录，只有确认选定文件才发布快照。

本地阅读：先创建任务 → CtrlR 明确添加材料目录 → CtrlT 选任务 → CtrlG 进入目录、勾选文件 → CtrlS 确认。首版最多 8 个 UTF-8 文本/CSV/JSON 文件，每个不超过 256 KiB。禁止符号链接、越界路径、二进制和自动递归导入。既有执行读取预算仍生效；选择了较大文件不代表整份数据一定能在预算内读完。材料默认为私有对象，只为该任务授予不可变版本的精确文件 grant。

对方文件：owner 用 CtrlN 输入 `@test 请只读分析你的样本……` → owner 卡片“等待对方主人授权” → test 在自己的 TUI 收到任务，按上述流程选择文件 → owner 卡片显示“材料已准备”，CtrlT 选中后 CtrlU 确认。用户无需输入 version、grant、workflow ID。普通 @ 消息不会触发文件授权。对方 Codex 需要信息时，本人卡片变为“等待用户”，选中后自然语言补充，由 local agent 发送到绑定的澄清消息；对方执行按保存的会话 ID 恢复。

本地任务从 `ready → running → completed` 推进；跨端还有等待授权、等待对方、等待用户、失败、超时、限额和 `needs_attention`。只有有效持久结果及执行完成证据才算成功。来源至少包含成员、对象、版本、相对路径、SHA-256；最终结果保存于个人数据库旁的 `.results/app-TASK_ID.json`，任务详情给出绝对路径。跨端结果必须匹配先前准备的材料引用。

停止不等于立刻杀掉模型：它停止本端后续调度、拒绝停止后的新提交；在途调用可能等到有界超时。发起端停止等待不会取消对方进程。重试只用现有 workflow/session/grants，状态机不允许时显示原因；远端执行须由远端主人重试。没有假的暂停/继续按钮。

## 共享应用层

`src/app` 独立于 TUI。`Instruction` 包含 UUID 请求 ID、渠道、会话 ID、可选任务 ID、实际接收方、正文、动作及结构化参数。`Actor` 是另一个可信参数：本机从个人端身份取得，未来适配器必须先认证并解析已绑定成员，再调用 `Actor::bound_member`。正文自称、渠道名称均不提供权限；`Instruction` 不接受 actor 字段。

`app_commands` 是 SQLite 持久队列，唯一请求 ID + 全字段比较保证相同请求重复提交只返回原状态；内容冲突拒绝。每条指令处理有进程锁及 CAS，事件/消息有稳定键去重，模型业务效果有独立回执。中断中的本人指令标为 `needs_attention`，不会自动重跑不确定的模型调用。任务已完成的结果也不会被一句“完成了”覆盖。

本人模型循环重用 HTTP 适配器，携带最近 24 条会话消息（每条最多 6000 字符）、选定任务和受限上下文。工具只允许查看上下文、保存回复、发消息、创建只读任务、补充选定草稿、回答绑定的同伴问题。没有 shell、文件正文、任意 SQL 或文件授权工具。首次有效答复前强制工具调用；工具失败即结束该轮并保留真实错误。Rust 回执展示操作状态，未提交工具的自然语言不作为成功凭据。每轮至多配置的模型调用数且硬上限 6，另有工具轮数、工具数量、输出长度和 HTTP 超时预算。

后台使用独立 SQLite 连接分别运行收件/心跳、本人指令、同伴请求/执行三个循环，网络与模型等待期间不持有数据库事务。UI 不等待模型；后台收件也不等待执行器。现有 `butler tick` 仍用于原有单次诊断，不消费本人应用队列；完整服务使用 `butler run` 或 `client start`。

测试适配器和未来 TG 可复用：

```sh
xxassxx --db /ABS/CLIENT/member.sqlite3 app session --project /ABS/PROJECT
xxassxx --db /ABS/CLIENT/member.sqlite3 app submit --input instruction.json
xxassxx --db /ABS/CLIENT/member.sqlite3 app command REQUEST_UUID
xxassxx --db /ABS/CLIENT/member.sqlite3 app snapshot SESSION_UUID
xxassxx --db /ABS/CLIENT/member.sqlite3 app events SESSION_UUID --after 0
```

`instruction.json` 示例（填入上一步会话 ID；补充已有任务时加入 task_id）：

```json
{"request_id":"b3512a17-7097-4853-a65c-ea292be73c99","channel":"second-test","session_id":"SESSION_UUID","task_id":null,"recipient":"owner","body":"只看那个脚本，不运行","action":"chat","payload":{}}
```

`app process` 显式消费一条指令，通常由后台负责。事件游标可重复读取，`app read` 单调保存已读位置。测试已证明第二渠道补充与 TUI 共享同一任务/历史、权限规则；没有接入真实 Telegram。

## 状态与恢复

新信箱提供经成员鉴权的 `/v1/presence` GET/POST，仅上报 `idle`/`busy`。30 秒租约采用服务端最近时间与接收端观测年龄；不比较双方机器的原始时钟。未知与过期分开：未上报或旧信箱返回 404 显示“未知”；有效心跳显示“在线”；过期后显示“上次活跃 X 秒/分钟/小时/天前”。仅在线时显示空闲/忙碌。最近观测时间按运行 TUI 的机器本地时区显示日期、时间和时区；心跳不携带项目名、路径、任务正文，不进入模型对话。

出站消息先落盘；断线按既有服务退避并重新同步。重连不重建任务/会话。关闭 TUI 不关闭后台；重新打开恢复会话、未读游标、任务与结果。服务中断的非确定指令需查看操作回执后人为决定下一步，没有自动无限重试。

## 限制与验证范围

本阶段只做明确授权的只读文本分析，不提供任意编辑、科研脚本执行或完整 shell；需要这些操作可主动进入 `xxassxx codex PATH`。此入口沿用官方 Codex 自身权限模型，不能与任务文件 grant 混为一谈。

macOS/Linux 是目标平台，Phase 5 的完整终端和真实双模型验证在本机 macOS 的两个隔离个人端进行。随后已更新正式 Mac、Lakota、Trader，并完成轻量启动、TUI 和双方在线状态检查，见 [Phase 5 部署记录](phase5-deployment.md)；此次部署没有重跑完整双机科研任务。旧常驻客户端未上报心跳时显示未知。没有 Windows、TG、自动更新、注册平台、GitHub Release。

数据库历史目前整体加载，事件接口每次最多 500 条并可按游标翻页；长历史分页、检索、消息保留策略仍需后续优化。本人模型上下文是有限窗口，不等于无限长期记忆。不同项目和不同接收方各有稳定会话；自动收到的授权请求归入本人最早登记的项目，详情明确显示，用户可另建明确项目的请求。Schema 5 不支持旧二进制降级读取；先备份数据库与内容库再升级正式环境。本阶段仅提供仓库构建，不自动安装。

复现与本次结果见 [Phase 5 验证记录](phase5-validation.md)。
