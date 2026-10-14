---
name: xxassxx-task-intake
description: 接续 xXASSXx 已确认的通用任务；在 Codex 每次启动或恢复时核对本轮任务、范围和已有答案，识别必要澄清。用于绑定任务的执行端，不用于需求草拟、成员调度或普通信箱留言。
metadata:
  version: "0.1.3"
---

# 接任务与澄清

接住已经分配给本机 Codex 的一轮工作。需求确认、跨成员协调和最终完成判断由 xXASSXx 的 local agent 与内核负责；本技能不创建任务或授予权限。

## 何时使用

收到 xXASSXx 启动事件中的 `task_id`、`run_id`，且本轮是有 `business_task` 的通用任务执行时使用。首次启动、回答问题后恢复、显式重试和切换执行阶段都重新使用。若只有旧版 `submit_collaboration_turn` 协议，沿用该协议，不套用本技能的提交方式。

## 接续本轮

1. 首先调用本轮绑定的 `get_task(task_id)`。返回的 `input` 是 JSON 字符串，解析后读取 `business_task`、`execution_policy`、`result_contract` 和授权范围。恢复会话也必须重新读取。
2. 工具返回的顶层 `task_id` 是本次执行编号，`business_task.task_id` 是业务任务编号；提交使用前者和工具返回的当前 `run_id`。不能拿历史 run 或业务编号替代。
3. 对照 `original` 与 `confirmed_intent`，理解本轮目标、交付物、约束、`mode` 和 `phase`。原话保留意图，当前确认版本决定范围；需要改变范围时交回协调，不自行把只读任务改为运行任务。
4. 以 `working_directory`、`authorized_roots` 及存在时的 `execution_access` 确定实际访问范围。`logical_working_directory` 是业务位置线索，不能据此离开当前物理范围。其他成员的路径不当作本机路径使用。
5. 阅读相关的 `questions_and_answers`、`owner_supplements`、`previous_execution`、`previous_task_findings`、`review_instructions` 和 `recovery_reason`。核对版本和问题状态：旧结果只是线索；`investigating` 问题的 `answer` 是调查安排，不是已经证实的答案。只读本次任务提供的上下文，不查私人数据库或其他对话。

## 判断是否需要澄清

- 已有答案足够、或能在当前授权范围内通过必要检查查清时，继续工作。普通概览不因输出样式等可选偏好阻塞；文件定位由执行方负责，不让发起人猜对方机器的路径。
- 只有影响本轮交付的事实、用户选择或必需材料确实缺失时，提出一个合并后的具体问题，说明缺什么、为什么影响结果、已经知道什么。不要重复已回答问题，或把进展查询扩大为完整科研验证。
- 快照缺文件只说明本轮材料不足，不证明原目录没有该文件或原目录未授权。说明需要补查的材料，由 local agent 决定是否安排原执行方重新发现材料；本轮不自行跳出快照。
- 通过 `submit_task_question` 提交 `body`、`reason`、`known`，使用当前执行 `task_id`、`run_id`，`idempotency_key` 等于当前 `run_id`。工具接受后结束本轮，由协调方处理；不直接联系其他成员，不在进程里等待用户答复。
- 如果 `get_task` 或工具明确报告任务停止、版本失效或授权撤销，结束本轮，不用旧上下文继续。其他读取错误如实交回启动器，不伪造问题或成功结果。

## 本技能的产出

信息足够时，继续完成当前阶段；不足时，交回一项可回答的结构化问题。不要额外生成接单报告或给用户复述内部编排过程。
