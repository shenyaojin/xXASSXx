# 第三阶段验证记录（已验收）

日期：2026-10-06（America/Denver）。用户已选定“实验数据质控”，首次真实双机运行、控制端独立检查及清理复核均通过，第三阶段可以验收完成。通信仍依赖 Mac/controller SSH 隧道；公网常驻部署属于第四阶段。

| 类别 | 当前结果 |
| --- | --- |
| 原有默认回归 | 47 项保留并通过；会话不匹配断言改为专门诊断 |
| 新增模拟协作/权限/服务测试 | 12 项通过 |
| macOS clippy（警告视为错误） | 通过 |
| Linux 完整回归 | 59 项通过 |
| 两端真实 Codex / DeepSeek / 文件读取 | 两端各一次真实检查通过 |
| 双机自动澄清及会话恢复 | 真实技术联调及最终质控案例均通过 |
| 用户选定最终任务 | 实验数据质控：8 个合成样本，方案/测量分属两端 |
| 最终双机验收 | 首次真实运行和独立检查通过；4 个通过、4 个需复测 |

默认回归与 clippy 沿用本文件记录的已有通过结果；本次未改动 Rust 源码、测试、运行脚本、材料正文或验收程序。已核对 36 个 Cargo/src/tests 文件与既有验证构建归档逐字一致，无需重新编译。

## 预检与连接

重新检查官方 CLI：Trader 为 `codex-cli 0.159.3`，Lakota 为 `codex-cli 0.159.2`，两端 `login status` 均确认 ChatGPT 登录有效。仅调用官方状态命令，未读取登录凭据。Lakota 使用已有 NVM Node/Codex 目录，为子进程补全 PATH，不修改全局 shell。

两端已有 `~/.config/xxassxx/secrets.env`，权限 600，程序按数据读取 DEEPSEEK_API_KEY。预检不打印密钥。

直连尝试：trader 无法解析 `lakota.mines.edu`；Lakota 到 Trader 的 SSH 握手未通过现有严格主机信任检查。未修改用户 SSH 配置，也未放宽校验。本轮复用经过 Mac 的加密 SSH 隧道，Mac 断开则失去跨机器投递能力；各端数据库保留待办。

原始预检位于 `smoke-output/phase3-preflight/`。真实结果必须以随后写入的原始证据为准，预检成功不等于模型参与或完整流程成功。


## 源码与 Linux 构建

本轮隔离目录：`~/xxassxx-tests/phase3-build-7fe212bd-cd5d-445c-abf6-789722bd4aa3`。
复用 Lakota 已有 `~/xxassxx-tests/cross-machine-dfb595f0-cfad-4d4e-be20-303a584efe4a/tooling/{cargo,rustup}`，限制 2 个编译任务和测试线程。仅允许列表中的 Cargo 清单/锁文件、src、tests、examples、README 进入源码归档，未包含 `.env` 或现有业务数据。

已验证源码归档：`source-verified.tar.gz`；SHA-256：`c644dc794d94f181ef8561d1bcf2fbadfb4436972149d36fc75fe23528d37a03`。日志：`smoke-output/phase3-build-7fe212bd-cd5d-445c-abf6-789722bd4aa3/linux-tests-verified.log`。59 项测试通过，2 个原有真实模型测试保持 ignored。真实验证使用第三阶段独立入口，不混入默认测试。

首次 Linux 回归发现旧断言仍期待通用 `protocol_error`，而新版专门报告 `session_mismatch`；更新该断言并复跑全套后通过。没有放宽会话一致性检查。


## 两端单端真实检查：通过

运行 ID：`phase3-real-1bc36b35-fcdb-4c2b-8980-f2b78db64f3a`。
完整证据目录：`smoke-output/phase3-real-single-1/`。两端都是实际官方 Codex 和实际 DeepSeek，没有模拟模型参与。

| 检查 | Trader / A | Lakota / B |
| --- | --- | --- |
| Codex 版本 | 0.159.3 | 0.159.2 |
| DeepSeek 请求模型 | deepseek-flash | deepseek-flash |
| 实际协调调用 | 3 | 3 |
| inspect_workflow / dispatch_codex | 两工具成功且调用 ID 已保存 | 两工具成功且调用 ID 已保存 |
| 授权文件正文读取 | 48 字节，随机 probe 正确 | 48 字节，不同随机 probe 正确 |
| 结果 | values 求和 31 | values 求和 31 |
| Codex 会话 | 01a11389-4773-7753-be36-56a638d72bba | 01a11389-4bdd-7de1-8fd9-1053c520bfc7 |
| 验收 | MCP 初始化/读取/提交、turn.completed、退出码 0、持久结果均通过 | 同左 |

每端结果引用具体对象、版本、路径和哈希。`a/b-execution.json` 保存原始结构化事件，`a/b-workflow.json` 保存读取凭据、协调记录和执行关联，`a/b-result.json` 保存最终产物。再次查询和空闲轮询未增加模型或 Codex 调用。两端服务、信箱和隧道已正常停止。

这些是单端技术检查，不是最终协作案例，也不单独证明会话续接。


## 双机真实技术往返：通过

运行 ID：`phase3-real-b2e0f60e-2881-465d-b07d-ef1aecf43c9f`。
完整证据：`smoke-output/phase3-real-dual-technical-1/`；可随仓库保存的 [精简证据](phase3-evidence.json) 同时包含单端与双端记录。

测试使用 `examples/phase3-technical.json` 中的合成校准材料，仅验证协议，不冒充与用户共同选择的最终案例。A 的规则与 B 的测量存于各自机器，数据库和材料没有互相复制。脚本只准备初始任务并启动两个 Rust 服务，没有逐轮手动执行 Codex、代填澄清回复或修改业务数据库。

实际链路：A 请求批次校准总量 → B 读到 p7、4、5，询问系数 → A 读本地规则答复系数 3 → B 返回 27 → A 结合本地 offset 2 保存 final_total 29。独立检查重新解析原始 CSV/JSON 并计算 `(4+5)*3+2`，同时核对两端材料 SHA-256 与最终来源引用，均通过。

| 证据 | A / Trader | B / Lakota |
| --- | --- | --- |
| Codex 执行 | 3 次全部 succeeded | 2 次全部 succeeded |
| 保存会话 | 01a1138a-61f1-7bd0-8b09-b0ddf180dce8 | 01a1138a-bbab-77e3-97b5-d6c4a0964c3d |
| 后续运行 resumed_session_id | 两次均等于保存 ID | 一次等于保存 ID |
| DeepSeek 实际调用 | 首轮 3 次；后续没有调用 | 首轮 3 次；后续没有调用 |
| 正文读取 | 累计 171 字节 | 累计 62 字节 |
| 固定版本 | 0fddfdd2-fd17-4bff-bbbc-ef618f3a4370 | a2d0b7a2-48a0-4ca2-bd67-adb14c1c5691 |

原始消息顺序为 request/clarification/answer/result，4 个消息 ID、reply_to 和事件均持久保存。每轮通过真实 MCP、完成事件和进程状态验收。只读状态查询和空闲等待未增加调用；结果 JSON 与来源引用保存于 A 本地数据库及结果文件。两端服务、信箱和隧道确认停止。

同一 Linux 二进制 SHA-256：`72ae3b82ab0e57a33978e329cd004e7aa76ee0fbe6c9f567b5314f1346736424`。

### 重跑技术检查

以下为已验证的现存隔离二进制路径，不修改用户原有服务：

```sh
python3 scripts/ssh_workflow.py \
  --binary-a /home/ubuntu/xxassxx-tests/phase3-build-7fe212bd-cd5d-445c-abf6-789722bd4aa3/bin/xxassxx \
  --binary-b /rcp/rcp42/home/shenyaojin/xxassxx-tests/phase3-build-7fe212bd-cd5d-445c-abf6-789722bd4aa3/bin/xxassxx \
  --technical --case examples/phase3-technical.json \
  --output smoke-output/my-new-phase3-technical-run
python3 scripts/check_phase3_technical.py smoke-output/my-new-phase3-technical-run
```

省略 `--technical --case ...` 可重跑两端独立真实检查。输出目录必须不存在，每次创建新的远端实例；成员信箱凭据仅在测试环境中传递，不持久保存，因此旧实例用于查看证据，重跑请用新目录。模型密钥由远端安全读取原有文件，Codex 使用原有官方登录。

## 最终案例：实验数据质控，验收通过

运行 ID：`phase3-real-e03420f2-0617-47ec-b2e0-3caadd05db34`。使用 [原始材料清单](../examples/phase3-qc/case.json) 与上文同一隔离二进制，未使用 `--technical`，未修改默认预算。首次真实运行即通过，无案例失败重试、无源码修复、无重新编译。

原始输出：[`smoke-output/phase3-qc-final-1/`](../smoke-output/phase3-qc-final-1/)。[真实运行报告](../smoke-output/phase3-qc-final-1/report.json)、[独立验收报告](../smoke-output/phase3-qc-final-1/independent-check.json)、[逐轮原始事件核对](../smoke-output/phase3-qc-final-1/raw-event-check.json)、[清理复核](../smoke-output/phase3-qc-final-1/cleanup-check.json) 均通过。可随仓库保存的 [精简证据](phase3-evidence.json) 保留历史记录，并新增 `two_host_qc`。

### 预检、授权与自动推进

本次预检保存在 [`phase3-qc-preflight-20261006-174915-27516fb4`](../smoke-output/phase3-qc-preflight-20261006-174915-27516fb4/)。两端官方登录有效，密钥文件为当前用户所有的普通文件、权限 600；只检查文件属性，不打印密钥或登录凭据。初次连接受控制端网络沙箱限制，放行已授权的 SSH 后成功；Lakota 系统 Python 3.6 不支持预检使用的参数，预检改用已有隔离 Python 3.12.14 后成功。这些均发生在真实案例启动前，未安装工具或改变远端全局配置。

仅 Trader 获得 1025 字节方案，Lakota 获得 162 字节 CSV；每端恰好一个固定版本 `record.txt` 授权。标准答案、独立验收程序和完整 case manifest 未传给两端任务。脚本仅发布各自材料、准备任务并启动服务；五轮 Codex 由 Rust 服务自动调度，没有手动代答或修改数据库填入结果。

workflow 默认限额保持不变：最多 6 次唤醒、8 次管家模型调用、6 条发信、1200 秒、累计 256 KiB 正文读取、每轮 64 KiB、每块及单次结果 8192 字节。执行器每轮 240 秒；DeepSeek 每次 60 秒、最多 4 次调用/3 个工具回合、1024 输出 token。实际每端管家调用均为 3 次，后续事件直接续接 Codex；空闲查询未增加调用。

### 实际往返摘录

以下时间为 America/Denver，原文与消息关联保存在两端 `*-workflow.json`。

1. 17:52:04，Trader 发起： “请先读取实际材料，确认其中全部实际批次，并提出完成质控所缺少的校准参数、计算、判定及报告规则问题”。
2. 17:52:28，Lakota 追问： “已读取授权测量 CSV，共 8 个样本，实际批次为 p7（S01–S04）和 q4（S05–S08）”，并查询参数、公式、阈值、等号边界、失败原因顺序与舍入规则。
3. 17:52:58，Trader 续接回答： “p7：blank=5、factor=2”， “q4：blank=2、factor=1”， “全部满足 20<=mean<=50 且 spread<=6 才通过，三个边界均包含等号”。完整答复包含方案标识、全部原因、复测及报告规则，未发送无关 r9 参数。方案未规定单位，答复明确不推定单位。
4. 17:53:21，Lakota 续接提交逐样本 JSON： “4个通过、4个未通过，S02、S03、S06和S08需复测”。它还按 Trader 的复核请求附上来自本地 CSV 的 `raw_samples`。
5. Trader 再次续接，重读方案并依据返回的原始读数/计算结果复核，保存最终 JSON： “依据 FL-QC-02 复核全部8个样本，4个通过、4个未通过，S02、S03、S06和S08需复测。”

原始消息序列严格为 `request → clarification → answer → result`，四条消息的 `message_id`、`reply_to`、会话关联与双方来源全部持久保存。

### 独立核算的样本结果

校正公式为 `(raw - blank) × factor`；均值须在 `[20,50]`、极差须 `≤6`，全部边界含等号。两端保存结果均由原始 JSON/CSV 独立重算核对。

| 样本 | 批次 | 三次校正值 | 均值 | 极差 | 判定及复测原因 |
| --- | --- | --- | --- | --- | --- |
| S01 | p7 | 20, 22, 24 | 22 | 4 | 通过 |
| S02 | p7 | 14, 16, 18 | 16 | 4 | 复测：均值低于 20 |
| S03 | p7 | 20, 26, 32 | 26 | 12 | 复测：极差大于 6 |
| S04 | p7 | 50, 50, 50 | 50 | 0 | 通过，均值等于上限 |
| S05 | q4 | 20, 20, 20 | 20 | 0 | 通过，均值等于下限 |
| S06 | q4 | 52, 53, 54 | 53 | 2 | 复测：均值高于 50 |
| S07 | q4 | 21, 24, 27 | 24 | 6 | 通过，极差等于上限 |
| S08 | q4 | 8, 18, 28 | 18 | 20 | 复测：均值低于 20 且极差大于 6 |

### 真实读取及原会话续接

| 证据 | Trader / A | Lakota / B |
| --- | --- | --- |
| Codex 版本 | 0.159.3 | 0.159.2 |
| Codex 成功轮数 | 3：ask / reply / complete | 2：ask / complete |
| 原会话 ID | `01a113a1-6353-7432-bc5c-cd320ae1c6b2` | `01a113a1-c4a3-7302-8f68-d234e9390300` |
| 后续 resumed_session_id | 两次均等于原 ID | 一次等于原 ID |
| 完整正文读取 | 每轮 1025 字节，共 3075 | 每轮 162 字节，共 324 |
| 材料对象 ID | `44877f68-7639-4c8e-bdc2-cf7e1a10832d` | `4e4eb3dc-7c1a-4164-90c1-f058ee7cc854` |
| 固定版本 ID | `7cc26068-83eb-445d-832a-c8577e1a98a6` | `4e8a104b-28dc-43eb-9eb9-59e65a79119d` |
| DeepSeek 模型与调用 | deepseek-flash，3 次 | deepseek-flash，3 次 |

A 正文 SHA-256：`ac918cf259b5ac079f6b99a88fab50830638e83e5451f63f9ea8ebcd5586aaad`。B 正文 SHA-256：`0ec36f40f45232aaf2f30bbd6e4c1a3acaa7eb6697420fe2ee1791c3673752b1`。最终 sources 的 member/object_id/version_id/path/sha256 与两端授权和真实读取全部一致。

[`a-execution.json`](../smoke-output/phase3-qc-final-1/a-execution.json) 与 [`b-execution.json`](../smoke-output/phase3-qc-final-1/b-execution.json) 保存官方原始 JSON 事件。额外逐轮核对确认：五轮均有 `get_task`、成功的 `read_task_file`、一次 `submit_collaboration_turn`、匹配原 ID 的 `thread.started` 和 `turn.completed`，退出码均为 0；MCP 实际返回的全文与该端材料逐字一致。实际工具调用仅为任务 MCP，未执行 shell、网页搜索或其他 Agent 调用。

最终产物为 [Trader 汇总](../smoke-output/phase3-qc-final-1/a-result.json) 与 [Lakota 分析](../smoke-output/phase3-qc-final-1/b-result.json)，来源引用与数据库快照中的结果一致。控制端运行 `python3 scripts/check_phase3_qc.py smoke-output/phase3-qc-final-1` 返回 `passed: true`；验收程序和标准答案未作修改。

### 清理与工作区保留

两端本次管家均为 `alive=false/state=stopped`；信箱退出码为 0。额外只读检查确认本次三个已记录服务 PID 已消失，两端无匹配本次实例的剩余进程，Mac 的本次 SSH 控制目录已移除且无匹配隧道进程。远端实例目录、数据库、材料及日志保留，未触碰其他服务。工作区未 reset、stash、clean 或提交；仅在真实运行和独立检查通过后更新阶段文档与精简证据。

## 已知限制与后续阶段

- [双机实验数据质控案例](../examples/phase3-qc/README.md) 已完成第三阶段最终验收；此结论限定于本案例、既有授权和预算边界。
- 连接仍依赖 Mac/controller SSH 隧道，未验证脱离 Mac 的服务器直连部署。
- 授权按确切文件和版本，B 必须先由本地主人 prepare；暂不提供动态远程扩权。
- 文本工具支持 UTF-8/CSV/JSON，单文件哈希校验上限 64 MiB；未实现 PDF/Office 解析和代码执行。
- 一端服务依次处理本地工作流；服务停止等待当前有界 tick 收尾，不承诺立即取消模型网络请求。
- 进程/会话锁基于单机本地数据库实例；断连导致的执行不确定需人工查看并 retry，不能视为分布式 exactly-once。
- 应用工具权限不是完整 OS 隔离；同一用户可管理本机数据。获准正文会进入 Codex 云上下文；本轮只使用合成数据。
