# 成员收件箱与成员 MCP 验收

更新：2026-10-13（America/Denver）。保留 10 月 12 日同机验收记录，新增 Trader 隔离 OAuth/MCP 部署、Linux 与 Intel 目标构建、跨机真实 PTY，以及真实 OpenAI 账户连接和个人 dot 往返。随后经用户授权，将实际本机 owner、Lakota test 和 Trader 信箱升级为 0.6.0-alpha，新增真实留言往返演示。源码未提交、推送或公开发布。

## 自动化与构建

Rust 1.99.0，Apple Silicon macOS。完整测试 **153 通过、0 失败、2 项既有 opt-in 忽略**；格式检查、Clippy `--all-targets -- -D warnings`、本机 release 构建和 diff whitespace 检查通过。新增 6 项收件箱回归及 1 项 HTTP MCP 集成回归。两个旧迁移断言改为实际 SCHEMA_VERSION，没有删除迁移检查。

覆盖普通留言持久化、跨入口幂等、并发重复发送一次入库、运输队列与任务隔离、后台回执无反馈循环、本人未读与 dot 读取分离、阅览期间新消息、三成员逐条权限和摘要隔离、隐藏回复目标、旧版本责任事实、历史未知阅览基线、重复本地缓存批次、设备 HTTP 鉴权、成员撤销、任务调度不倒写信箱版本。已有任务、授权、文件传输和更新器回归保持通过。

MCP 集成使用官方 SDK 和真实本机 HTTP，授权 introspection 为模拟服务。实际验证 initialize、六工具发现、身份／联系人／线程业务、发送重试、普通回复、设备令牌拒绝、过期／签发者／受众／scope、未绑定主体及错误 client_id、只读范围、连接撤销和成员退组。模拟 introspection 不验证授权码、PKCE 登录、刷新及外部 provider 的真实撤销。SDK 固定 rmcp 3.5.1，测试协议为 2025-06-18；最低 Rust 声明提升到 1.88，未另跑该最低版本构建。为保留原模块的嵌套 if 风格，单独允许 collapsible_if 样式 lint，其他检查仍以 warnings 为错误。

## 亲自操作的真实终端

使用可交互系统 PTY，分别启动测试 A、B 两个真实 TUI，配独立数据库、白名单、工作目录和本机信箱，不接管用户原有窗口。原始画面和本地控制脚本保存在 Git 忽略的 `smoke-output/inbox-20261012/`，提交用精简事实见 [inbox-evidence.json](inbox-evidence.json)。

1. A 输入 `/inbox`，选 B，填写主题和正文并发送。先看到“待发送”，后变为“已进入对方收件箱”；B 看到了原话及 A 的归属。
2. B 进入详情并普通回复，未读仍为 1。A 收到关联原条目的回复。旧构建打开线程时先落在自己旧条目，已修为优先定位发给本人的未读；重启最终构建后亲自复查，直接显示 B 的回复。
3. 停止隔离信箱，B 明确标记当前条目已查看，并发出离线补充。界面显示缓存、最后同步时间与待发送。恢复同一信箱，阅览和补发收敛；独立核对三条普通留言各仅一份，B seen_seq=1、A seen_seq=0。后台同步不会代替本人阅览。
4. B 使用既有 CtrlF 文件选择器勾选合成文件并发送。两次复制一致性检查拒绝后，第三次通过同一界面重新选择成功；A 的收件箱显示附件交付及“接收端已校验并保存”，CtrlF 详情显示 67 字节已保存到本机。实际接收字节 SHA-256 与源文件一致：`e32fd84be1cd27f3e42f47e8c8f85307d407ee83a380c9e71d500fcc88584d99`。已保存不等于本人已读，未读仍保留。
5. 所有隔离个人库为 schema 10，信箱为 schema 2，完整性检查均为 ok。

首轮曾出现一次 HTTP 503，原错误未记录到足以断言根因。已将新增写事务改为 BEGIN IMMEDIATE，新增八线程同键并发回归并通过，后续实际终端正常；不能把这一证据写成已证明所有 503 的原因。真实文件读取中还观察到 ctime 随读变化、内容哈希和 mtime 未变，导致既有一致性保护暂停；未放宽保护，未改数据库补造结果，该环境现象的根因尚未确定。

## 真实模型与最终回答

沿用现有 DeepSeek 配置和官方 Codex 登录，但只操作合成测试文件。A 从 TUI 提出只读请求并以 CtrlS 确认，任务 `5123bf23-3d25-522a-8b24-754edfb51ef6` 第 1 版；执行成员 A，目录属于测试 A。第一次冻结因一致性检查暂停，通过同一 TUI CtrlY 恢复后完成 discovery 和 analysis。共 3 次成功 Codex turn（含第一次 discovery），均 turn_completed=1、exit_code=0。

亲自读到终端最终答复：marker 为 **INBOX-6319**，sample_count 为 **37**，来源为工作目录中的 inbox-check.txt，67 字节，并提供真实文件哈希。任务卡“已完成”，下一步“无”。原文件、冻结副本及实际结果一致。本轮真实模型证据发生在第一轮构建；其后的变更为收件箱事务串行化、未读定位和提示文字，最终构建另做了双端终端及附件复查。相关构建哈希分别记录，不能混写成同一二进制。最后仅将 MCP 联系人的 structuredContent 包装为对象以兼容客户端，六工具 HTTP 集成和 Clippy 再次通过，终端逻辑未再变更。

## 10 月 13 日：真实 OAuth 与平台续验

新增证据见 [平台与 OAuth 证据](inbox-platform-oauth-evidence.json)。Apple Silicon 原完整 153 项回归保留；本次 OAuth Basic 编码修复后，相关 HTTP MCP 回归、格式、Clippy 与优化构建通过。Linux 最终源码完整 **153 通过、0 失败、2 项原有忽略**；Intel 目标在 Rosetta 下完整 **153 通过、0 失败、2 项原有忽略**，其后同一 OAuth 修复的定向回归再次通过。三平台优化构建完成，Linux 与 Intel 包分别在独立目录离线安装并运行。Linux 源码逐文件哈希与本机清单一致。Intel 证据来自 Apple Silicon 上的 Rosetta，尚无本轮原生 Intel 机器或最终源码 CI 证据。

Linux 首次使用了错误的最终链接器，造成启动崩溃；恢复项目已验证的 Rust 默认链接器，musl-gcc 只用于 C 依赖。系统 Python 3.6 又不能运行依赖 tomllib 的测试夹具，改用既有 Python 3.12 后通过全部回归。未据此改动产品逻辑。Linux 仍有既有 libc::time_t 弃用警告。

Trader 新建独立 schema 2 信箱与 Keycloak 26.8.0（Temurin 21.0.12.1+1），共用既有 HTTPS 代理的新增限定路径；当时日常信箱 PID 3710124、schema 1 保持不变；随后现用升级见下节。Keycloak 只在 loopback 监听，公开测试 realm 的 OAuth 与登录资源，管理接口不公开。使用单节点 H2 dev-file 保存合成身份和授权，属于隔离验收部署。两个测试服务已启用，配置和凭据留在私有测试目录；没有把真实成员资料或个人设备令牌交给 OpenAI。

亲自在 Chrome 使用合成 inbox-b 登录，完成独立验收客户端的授权码及 PKCE S256。实际令牌刷新与轮换通过，introspection 返回精确 issuer、resource audience、sub 和 client_id；HTTPS MCP 正确解析为测试成员 B。Keycloak 当前要求 introspection 客户端位于受众内，因此将资源客户端 ID 设为精确 MCP URL，保留其 audience 校验。联调发现原适配器直接发送 HTTP Basic 凭据，URL 客户端 ID 的冒号会破坏认证；已按 RFC 6749 对 ID 和 secret 先做表单编码，再构造 Basic，并加入 URL、空格、特殊字符与 Unicode 的回归。

真实令牌的连接撤销、主体映射不匹配与退出团队均返回 403，恢复配置后重新成功。Provider 撤销测试刷新令牌返回 200，随后原 access token 访问 MCP 返回 401，再次刷新返回 400 invalid_grant。该实验使用独立验收客户端，不能记为 OpenAI/dot 的刷新与撤销验收。

跨机真实 PTY：Intel 构建的 A 向 Linux B 发送 DOT-7421 与样本数 37；B 终端看见原话并回复，A 终端最终显示准确的 B 回复及原条目关联。真实 OAuth 验收客户端读取同一线程后，B 未读仍为 1；B 普通回复后未读也保留，两人的 seen_seq 均为 0。两条记录只各保存一份，旧任务运输队列为 0。退出成员实验正确阻断了 B 的设备后台（401/blocked）；恢复成员并显式重启个人后台后，终端重新同步正常。此轮未重复此前真实模型计算，保留上节模型和附件证据及对应旧构建哈希。

## 10 月 13 日：OpenAI 连接修复与个人 dot

用户明确批准完整隔离权限后，在登录和同意页遇到 OpenAI “无法连接账户”。服务日志证明登录已成功，失败发生在授权码换令牌，原因是合成用户 B 缺少 `offline_access` realm 角色。补齐该受邀用户的角色，保留已有 scope 映射、精确 audience、PKCE、同意要求及 fullScopeAllowed=false。重新开始连接后，OpenAI 显示测试账户已连接，Keycloak 记录 CODE_TO_TOKEN 成功及 Offline 刷新令牌。此次修复只涉及隔离 OAuth 配置及运维文档，没有改 Rust 逻辑或重建二进制。

先用真实 ChatGPT Work 查询身份、列表和原两条线程记录，明确指示后发送一条普通回复（seq=3）；再从用户个人 dot 的 `/dots/home` 入口重新查询并发送另一条不同的验收回复（seq=4）。两次发送都在 B 的 Linux 客户端停止时进行，并各使用固定幂等键。A 的 Intel/Rosetta 真实 TUI 看见准确正文、B → A、经 dot 委托发出及原始留言关联。B 恢复后看到 4 条唯一缓存，未读仍为 1；两个人库任务数均为 0，旧任务运输队列仍为 0。随后个人 dot 实时重新调用身份和线程工具，确认 4 条记录、两条回复各一份、B seen_seq=0。不能把普通留言发送成功称为执行任务成功。

个人 dot 最后实时复查时，Keycloak 另记录真实 xxassxx-dot-test 客户端的 Offline REFRESH_TOKEN 成功事件，随后身份与线程查询成功。此前未存储成功刷新事件，现已只对隔离 realm 开启该事件类别；不能用先前缺少记录断言没有刷新。

精简记录见 [真实 OpenAI 与 dot 证据](dot-live-evidence.json)。Work 对话与个人 dot 分别记录，不将测试脚本的 MCP 调用冒充个人 dot。首次本机 PTY 运行于限制联网的工具沙箱，明确显示缓存与暂时不同步；在获准联网的同一隔离环境重启后恢复，无产品逻辑变更。此前独立 OAuth 客户端的 provider 撤销、绑定撤销、错主体及退组证据继续保留，未将这些负向测试混写成 OpenAI UI 操作。

## 10 月 13 日：现用三端升级与用户演示

用户明确要求移走本机编译负担、升级三端并展示一个实际功能。本机实际身份为 owner、原程序 0.3.2-alpha、路径 `~/.local/bin/xxassxx`，与此前 MBA 的部署记录不同；本轮按实测身份操作，没有更换为 mba。Lakota test 和 Trader 原为 0.5.0-alpha。三端停服保存完整数据目录、配置与旧程序，以 SQLite 备份 API 验证数据库完整性及外键；备份保存在各机 `~/.local/share/xxassxx/backups/inbox-060-20261013/`，未实际演练整套回退。

本次升级为 0.6.0-alpha：信箱 1→2，个人端 9→10。原业务表的记录指纹及配置核对保留，身份、白名单、任务、运行与消息历史不丢失。服务器成员心跳是动态数据，重启后时间戳正常变化，不能把它算作业务历史丢失。两台个人后台显示 0.6.0-alpha 且 doctor 通过，Trader 原 systemd 单元 active/running、零重启，实际进程二进制哈希与验收产物一致。

Linux 在 Lakota 完整通过 153 项测试，0 失败、2 项既有忽略，优化构建通过；131 个运行源码文件与本机清单一致。两个 Mac 架构沿用已验证 release 缓存，仅做版本号增量构建，分别运行版本入口确认 0.6.0-alpha。三平台安装包均保存到 Lakota 并校验 SHA-256；随后本机 target 缓存已移除，约释放 21 GiB，可用空间由约 2.6 GiB 升到 25 GiB。今后构建地点偏好已写入 AGENTS.md 和发布说明。未借用旧 CI 结果宣称最终版本原生 Intel 验收。

亲自操作升级后的两端实际 PTY，经日常 HTTPS 信箱完成“收件箱小演示 · 10 月 13 日”：owner 用 `/inbox` 新建普通留言，终端显示已进入对方收件箱；test 的未读列表显示原话，使用 r 在同一条对话中回复；owner 终端最终显示“收到，可以在同一条对话里继续交流。这是 Lakota 发回的普通留言，没有启动计算任务。”原留言与回复各一条，回复关联正确。两端 seen_seq 均保持 0，Mac 留有一个未读线程供用户自己查看；任务数保持 8 / 10，旧任务消息队列保持 118。没有绕过产品写入业务数据库，没有运行新的业务计算。关闭演示 PTY 后两个后台仍运行。

精简证据见 [现用部署与演示](inbox-deployment-evidence.json)，实际终端画面保存在 `smoke-output/inbox-deployment-20261013/received-reply.png`。真实 dot 仍绑定隔离测试团队，不因这次安装升级扩大到日常数据。

## 尚未完成与交付边界

- Intel 目标已编译、安装并在 Rosetta 运行；当时无原生 Intel 硬件或最终源码 CI 证据；用户于 2026-10-13 决定停止 Intel 支持，此项验收待办已撤销。本机 owner、Lakota test 与 Trader 已停服备份并完成跨 schema 升级；未提交或公开发布。
- 主动提醒、MCP Events、dot 任务执行／本地授权及附件正文下载仍不在本期范围。普通发送的永久拒绝项可查看原因，尚无专用重新编辑表单。

迁移恢复步骤见 [成员收件箱](inbox.md)，实际端点与授权要求见 [dot 接入](dot-connection.md)。自动回归、真实 PTY、真实模型、独立 OAuth 客户端与真实 dot 的证据分别记录；当前不能宣称一期完整验收。
