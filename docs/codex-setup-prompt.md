# 交给 Codex 的个人端安装提示词

将下面正文交给朋友自己 Mac 上的 Codex，同时提供解压后的安装包目录、pengchao.json 的本地路径，以及想允许读取的文件夹。当前离线包是 Apple 芯片版；Intel 不能安装此包。邀请和 DeepSeek key 不要粘贴进聊天。

---

请帮我在这台 Mac 安装并配置 xXASSXx 个人端。请实际执行安装与检查，最后给我简短的使用方法。

我会给你这三项本地路径：
- 解压后的 xXASSXx 安装包目录。
- 我的私人邀请 pengchao.json。
- 我允许 Codex 读取的业务文件夹（可暂时留空，稍后在界面按 Ctrl+R 添加）。

先确认系统和 CPU 架构。当前提供的包支持 macOS Apple 芯片（arm64），不要在 Intel 上强行安装。阅读包里的安装说明和命令帮助，使用随包 install.sh --from 安装包目录，安装到 ~/.local/bin，并校验 SHA-256；不需要 Rust 或 sudo。使用已提供的离线包，不假设 GitHub 上已经有包含最新功能的 Release。将 ~/.local/bin 加入当前会话 PATH；如果登录 shell 的 PATH 缺失，幂等地加入对应 shell 配置，保留已有内容。

先检查是否已有个人端（默认 ~/.local/share/xxassxx/client）。已有个人端就检查身份并保留其数据、白名单和配置，不重新 init、不替换身份、不删除数据库；旧版升级前先确认无运行任务，停止后台并退出旧界面；保留数据和身份，不重新初始化。0.3.1-alpha 的数据库版本为 9，旧二进制不能直接读取升级后的数据库。

邀请已绑定团队 xxassxx、用户名 pengchao、显示名称 pengchao，服务器地址为 https://xxassxx.shenyaojin.com。用户名不能通过修改邀请自行更换。当前邀请中的联系人应为 owner（Shenyao "Keith" Jin）、test（Lakota）和 mba（Keith 的 MacBook Air）；不得添加 a、b。已有个人端若使用旧邀请，更新联系人时保留其身份、模型和执行配置，不重新初始化。不要输出邀请正文或其中的凭据，不把邀请打进公共安装包。

local agent 先用 DeepSeek，模型 deepseek-flash。密钥文件为 ~/.config/xxassxx/secrets.env，字段 DEEPSEEK_API_KEY，权限 600、目录权限 700。缺少密钥时，请让我在本机终端以隐藏输入的方式填入，不在聊天中询问明文，不回显、不写进命令历史，也不覆盖已有密钥文件。确认配置采用 thinking=false（这是当前默认值）。

检查官方 Codex CLI 是否可用，以及我的现有登录状态。保留我的 ChatGPT/Codex 登录，不要求 OPENAI_API_KEY。需要登录或安装 CLI 时给出明确操作，完成前不要宣称已配置好。请将实际找到的 Codex 可执行文件绝对路径传给 client init 的 --codex，避免后台 PATH 找不到它。

只有新个人端才执行：
xxassxx client init --invite 邀请文件绝对路径 --provider deepseek --model deepseek-flash --model-secrets "$HOME/.config/xxassxx/secrets.env" --codex Codex可执行文件绝对路径
如果我指定了白名单文件夹，为每个文件夹追加一个 --allow-dir 参数。已有个人端用 client roots add 添加我明确选定的目录。不要自动把整个 HOME、当前目录或父目录加入白名单。

xXASSXx/local agent 收集用户意图、上下文、澄清并联系其他成员，大部分实质任务交给本机 Codex。Codex 使用原生工具在白名单内只读查找、查看和解释，按本地或团队请求返回答案与相对文件清单。添加白名单不自动上传整个目录，但之后的团队查询可以返回相关信息。当前不支持通过此入口修改文件或运行科研程序；不要把旧版“必须先导入对象、再指定 object ID”的流程当作普通找文件的前提。

运行 xxassxx client doctor --probe-model，检查信箱身份、Codex 登录和 DeepSeek 工具调用闭环。我授权这项小型模型检查。检查失败就报告真实原因，不把普通聊天成功当作工具调用成功。然后运行 xxassxx client start，并确认 client status 中 alive=true。不要替我发送团队消息或执行科研任务。

最后告诉我身份、白名单、连接与模型检查结果、后台是否运行，以及这几种用法：
- 在项目目录运行 xxassxx，或 xxassxx open /项目路径，打开 xXASSXx 自己的终端界面。第一次启动会询问是否将启动目录加入白名单。
- 普通输入与自己的 local agent 对话；@owner 联系 Keith，@test 联系 Lakota 的 local agent。
- Ctrl+R 管理白名单，Ctrl+O 查看会话，Tab 查看成员状态。
- Ctrl+Q 只退出界面，后台继续工作；xxassxx client stop 停止后台。
- xxassxx client start 重新启动后台。当前尚未配置开机自动启动，也没有启动更新检查。
