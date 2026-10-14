# 真实 DSS 结果图交付验收

日期：2026-10-13（America/Denver）。用户明确要求使用真实 DSS 项目，随后要求停止旧任务并新开任务。本轮不使用测试图片代替业务验收。

## 操作与结果

从 Mac 已安装程序的可交互 PTY 打开原任务 `defc0941-301a-51b9-a2e9-f8713c27939b`。修订 4 暴露启动指令冲突：尽管 get_task 的 files 合同要求本轮选定文件，启动提示仍要求 discover 仅提交路径待冻结。实际模型只核对元数据，准备两张尚未确定的候选图，未批准发送。按用户随后明确要求，通过 CtrlX 填写原因并停止归档；Lakota 收到取消通知。旧 15 份参考资料及两图批次均没有发送。

修复 `src/codex.rs` 启动指令，以 get_task 的执行合同为准，明确 files 需要本轮读取必要依据并选定交付物。模拟执行器回归检查真正传给进程的启动提示，防止这一冲突再次绕过上下文测试。Lakota 最终完整测试 167 通过、0 失败、2 项原有忽略，格式、Clippy（保留原有 deprecated 豁免）与 Linux musl release 构建通过；两地 99 个 Rust、SQL、Python 测试及 Cargo 文件指纹一致。

新任务 `5b210e2b-f35b-5f9b-b643-d86ee540d829` 在新对话中创建、确认，要求现有 DSS 最新结果图和一句说明，不重算，并提供此前已知分支作为查找线索。第 1 版实际读取状态，但将“最新”理解成写盘最晚的参数扰动评估图。核对后未批准它，随后通过 CtrlE／CtrlS 明确要最新完成优化最终采用的结果。第 2 版实际执行 `009d3112-b4af-5e32-9fa4-3f24fe7786e7`，run `fea4b8bf-4964-4e89-9469-68e987a10dfe`，官方 Codex 会话 `01a137ba-75be-7d93-8935-9a0a0ef34e3c`，一次提交、turn.completed、退出码 0。两次定向只读命令检查完成状态、优化结果及其指向目录，没有再查全项目或运行计算。

`continuous_geometry_bg18/fixed_geometry_inversion/status.json` 和 `optimizer_result.json` 均记录 2026-10-07 06:54 UTC 完成、成功收敛，并以 `final_forward_folder` 指向 `permeability_evaluations/k_024a5f80458d6667`。采用其 `qc_bg_18_only.png`；该图与 `fixed_geometry_inversion/qc_final.png` 字节完全一致。晚写入的 `k_0640aa308e803988` 属于参数扰动试算，不能代替最终采用点。结论来自此分支现有记录，不代表再次完成科研验证或重新计算。

在 Lakota 真实界面打开附件清单，确认仅一张 252.2 KiB PNG，按 a 批准确切批次 `a10f23a9-cc4f-5060-9dfb-0b9afb6db072`。原始 PNG 经产品的 Trader HTTPS 文件交付路径发送，Mac 自动接收，双方任务完成；Mac 终端显示“1 个文件已送达 @owner”，文件页显示已保存到本机，Enter 请求系统打开，图片实际解码、目视核对成功。再使用客户端 s 另存为 `/Users/shenyaojin/Downloads/DSS-最新完成结果-2026-10-07.png`。

原图、发件固定副本、收件副本、下载副本都是 258255 字节，SHA-256 `792071a95e00935ba04a1fe739a55593acd832b67cb3ec395bc0d19a3e237c9f`。核对的 8 个源记录／图片前后哈希未变。SSH 只用于构建、安装、检查及保存验收证据，没有用 SCP 传科研图片绕过产品。Mac 本任务零文件执行，Lakota 最终修订零重试、无未答问题。

## 安装与边界

只将 Lakota 更新为 0.6.5-alpha（程序 SHA-256 `54637d20787a4cb66ad52e8175f9208910b6bbc13e390d6c4d85b55c725d454b`），安装前确认无在途执行，备份在 `~/.local/share/xxassxx/backups/dss-065-20261013/`，停服安装前后全部数据库表及配置指纹一致。Mac 保留 0.6.4-alpha，Trader 保留 0.6.0-alpha，无本机 Rust 编译。Linux 包在本机 `dist/v0.6.5-alpha/` 与 Lakota 校验保存；未提交、推送、公开发布或实测更新器。

使用真实 PTY，并保存 ANSI、文本和单元格画面；由画面记录绘制 PNG 供目视检查，不声称控制了用户原有 Terminal 窗口。此为人工核对并澄清“最新”的真实业务交付，不能宣称完全自动选图已解决。完成页目前只显示送达与文件名，简短选图解释没有保留在最终主文案中；原说明与依据保留在任务结果详情。旧版未批准批次还会出现在发送端附件历史，不影响当前交付，但仍有界面整理空间。

验收用 PTY 已退出，日常后台继续运行；恢复了为打开旧会话而临时选择的 Lakota 工作目录偏好，未修改任务状态数据库。约 5.6 MiB 的本机临时 PTY 依赖已清理。详细记录位于 Git 忽略目录 `smoke-output/dss-real-delivery-20261013/`，精简证据见 [JSON](dss-real-delivery-evidence.json)。
