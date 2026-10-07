# 本机 owner 与 Lakota test 的第一次体验提示词

下面正文可直接交给这台 Mac 上的新 Codex 对话。Mac 的 owner 与 Lakota 的 test 个人端均已建立并启动；该提示词负责检查现有配置并开展第一次真正的协作。所有身份凭据仍保存在私有文件中。

---

请替我用 xXASSXx 与 Lakota 上的 `test` 管家进行一次真实的脚本阅读协作，复用现有安装和配置，完成任务准备，让我看到问答和最终结果。

本机仓库是 `/Users/shenyaojin/Documents/gitproject/xXASSXx`。先阅读仓库的 `docs/phase4.md`、`smoke-output/mac-owner-ready.json` 和 `smoke-output/lakota-mariner-ready.json`。就绪记录包含两端个人端目录、已经导入的脚本版本与预授权 workflow ID；它不是这次新任务的完成证据。

本机使用 `owner` 身份，显示名称为 `Shenyao "Keith" Jin`。程序已安装到 `/Users/shenyaojin/.local/bin/xxassxx`，数据目录为 `/Users/shenyaojin/.local/share/xxassxx/client`。先用程序的绝对路径检查 `client whoami` 和 `client status`；复用同一身份，不重新 init，不覆盖配置或数据库。确认 Codex CLI 可用并复用我的官方登录。缺少必须由我完成的登录步骤时明确指出。

我授权这次试用消耗我的 DeepSeek 额度。本机 `~/.config/xxassxx/secrets.env` 已配置且权限为 600，复用即可，不输出内容。管家使用 `deepseek-flash`，两次真实工具调用探针已经通过，记录见本机就绪报告；不必重复安装或重复探针，先检查当前连接。个人端若停止则 `client start`。

这次本机没有要开放的业务目录，白名单保持为空即可。可以通过本地知识对象的 `--text` 保存我下面的任务要求，形成发起端的固定版本 `record.txt` 并作为本机任务授权；这不需要扩大文件白名单。

我的问题是：

“请 test 阅读 `scripts/DSS_history_match/109_revised_misfit_func.py`，解释它现在实际完成了哪些数据预处理，漂移校正如何计算，时间和深度窗口分别是什么。它是否已经实现并运行了 misfit 或优化？请区分实际代码、注释中的计划和仍需其他文件才能确认的部分，用中文给出来源。”

通过 xXASSXx 的协作协议完成：检查就绪记录中的远端 workflow 是否仍为 `prepared`；如果已经使用过，可通过 SSH 在 Lakota 使用同一脚本版本和同一 grant，为 peer=`owner` 新建一次 `collaboration prepare`，不要改旧任务、填库或扩大授权。然后在本机用 `collaboration start`，指定 peer=`test`、该远端 workflow ID、本机任务说明的 grant 和上面的问题，让两端 Rust 服务与各自的 Codex 推进任务。有真正的信息缺口时允许澄清往返，不为了展示刻意制造追问。

这次读取能力验证必须由 test 的 Codex 通过它的任务 MCP 读取脚本完成。SSH 只用于检查服务与准备任务；不要直接 SSH 读取脚本后替 test 回答，也不要用安装时的检查答案冒充这次结果。不得运行该科研脚本、模拟器或优化程序，不读取脚本引用的 `data`、环境、凭据等目录；目前 Lakota 唯一白名单是 `/rcp/rcp42/home/shenyaojin/Documents/bakken_mariner/scripts`，且本次任务只授权就绪记录中的那一个脚本版本。

等待双方完成，检查实际 workflow、原始消息、文件读取记录、Codex `turn.completed` 与退出码，再用 `client result` 取最终结果。失败时报告实际状态并定位原因，不手工补答案或伪造成功。不要另外给任何人发邮件或聊天消息。

最后用中文展示简短问答经过和最终回答，附本次双方 workflow ID、准确来源、实际模型/执行轮数及结果位置。测试管家是后续试用的常驻端，完成后保留它运行；本机管家也可保留供我继续试用。说明普通 `client send` 只发消息，不能代替这次需要文件 grant 的正式协作任务。
