//! Recovery is a choice of a next action, not an unconditional retry button.
use super::{draw_lines, presentation};
use ratatui::layout::{Constraint, Layout, Rect};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Supplement,
    Retry,
    Answer,
    Approve,
    Handoff,
    Revise,
    Detail,
    Stop,
    Back,
}

impl Action {
    pub(super) fn label(self, task: &Value, owner: &str) -> &'static str {
        match self {
            Self::Supplement if task["initiator"] != owner => "s 补充本机约束或纠正",
            Self::Supplement => "s 补充事实或纠正",
            Self::Retry if task["blocker"]["step_kind"] == "job" => {
                "r 重试失败的判断或检查 · 复用已有执行结果"
            }
            Self::Retry if task["blocker"]["reason"] == "execution_uncertain" => {
                "r 允许先核对已有成果与副作用，再有限续接"
            }
            Self::Retry if task["blocker"]["reason"] == "local_permission" => {
                "r 修复本机权限后重新检查并续接"
            }
            Self::Retry => "r 从失败步骤有限续接 · 先核对已有成果",
            Self::Answer => "a 回答当前待答问题",
            Self::Approve => "a 查看本机授权范围",
            Self::Handoff => "h 核对旧执行已停止及副作用",
            Self::Revise => "e 修改目标或范围 · 生成新版本后确认",
            Self::Detail => "d 查看完整任务、成果及运行记录",
            Self::Stop => "x 停止任务 · 先填写原因，不会立即停止",
            Self::Back => "Esc 返回原任务 · 保留当前状态",
        }
    }
    pub(super) fn shortcut(self) -> char {
        match self {
            Self::Supplement => 's',
            Self::Retry => 'r',
            Self::Answer | Self::Approve => 'a',
            Self::Handoff => 'h',
            Self::Revise => 'e',
            Self::Detail => 'd',
            Self::Stop => 'x',
            Self::Back => '\u{1b}',
        }
    }
}

fn participates(task: &Value, owner: &str) -> bool {
    task["initiator"] == owner
        || task["executors"]
            .as_array()
            .is_some_and(|members| members.iter().any(|member| member == owner))
}

pub(super) fn can_supplement(task: &Value, owner: &str) -> bool {
    participates(task, owner)
        && task["confirmed"] == task["revision"]
        && task["revision"].is_number()
        && !matches!(
            task["state"].as_str(),
            Some("completed" | "cancelled" | "stopped")
        )
        && !stopped_assignment(task)
}

fn stopped_assignment(task: &Value) -> bool {
    presentation::stop_status(task).is_some()
}

fn member_withdrew(task: &Value, owner: &str) -> bool {
    task["history"].as_array().is_some_and(|events| {
        events.iter().any(|event| {
            event["revision"] == task["revision"]
                && event["sender"] != owner
                && event["kind"] == "progress"
                && event["payload"]["stage"] == "member_stopped"
        })
    })
}

pub(super) fn can_retry(task: &Value, owner: &str) -> bool {
    !stopped_assignment(task)
        && !member_withdrew(task, owner)
        && (task["state"] == "needs_attention"
            || (task["state"] == "waiting"
                && task["initiator"] == owner
                && task["blocker"]["id"].is_string()))
        && task["blocker"]["reason"] != "user_input"
        && task["execution_permission"]["can_allow"] != true
        && !task["questions"].as_array().is_some_and(|questions| {
            questions
                .iter()
                .any(|q| q["revision"] == task["revision"] && q["state"] == "user")
        })
        && (task["initiator"] == owner
            || task["blocker"]["owner"] == owner
            || (task["blocker"].is_null() && participates(task, owner)))
}

pub(super) fn actions(task: &Value, owner: &str) -> Vec<Action> {
    let terminal = matches!(
        task["state"].as_str(),
        Some("completed" | "cancelled" | "stopped")
    );
    let mut actions = vec![];
    if !terminal {
        if task["assignment_stops"].as_array().is_some_and(|stops| {
            stops
                .iter()
                .any(|s| s["executor"] == owner && s["state"] != "stopped")
        }) {
            actions.push(Action::Handoff);
        } else if task["execution_permission"]["can_allow"] == true {
            actions.push(Action::Approve);
        } else if task["initiator"] == owner
            && task["questions"].as_array().is_some_and(|questions| {
                questions
                    .iter()
                    .any(|q| q["revision"] == task["revision"] && q["state"] == "user")
            })
        {
            actions.push(Action::Answer);
        }
        if can_supplement(task, owner) && !stopped_assignment(task) {
            actions.push(Action::Supplement);
        }
        if can_retry(task, owner) {
            actions.push(Action::Retry);
        }
        if task["initiator"] == owner {
            actions.push(Action::Revise);
        }
    }
    actions.push(Action::Detail);
    if !terminal && participates(task, owner) {
        actions.push(Action::Stop);
    }
    actions.push(Action::Back);
    actions
}

pub(super) fn lines(task: &Value, owner: &str) -> Vec<String> {
    let b = &task["blocker"];
    let mut lines = vec![format!(
        "{} · 第 {} 版 · {}",
        task["short_id"].as_str().unwrap_or("当前任务"),
        task["revision"],
        task["goal"].as_str().unwrap_or("")
    )];
    if let Some(reason) = b["reason"].as_str() {
        // Do not let a broad category hide the actionable error below it.
        let label = serde_json::from_value::<crate::task_recovery::Reason>(b["reason"].clone())
            .ok()
            .map(|r| r.label())
            .unwrap_or(reason);
        lines.push(format!("状态：{label}"));
    } else {
        lines.push(format!("状态：{}", presentation::task_status(task, owner)));
    }
    if let Some(detail) = b["detail"]
        .as_str()
        .or(task["error"].as_str())
        .filter(|s| !s.trim().is_empty())
    {
        lines.push(format!("具体原因：{detail}"));
    } else {
        lines.push("具体原因：当前记录未提供错误细节，可查看完整记录。".into());
    }
    let execution = task["executions"].as_array().and_then(|items| {
        items
            .iter()
            .find(|e| e["id"] == b["step_id"] && !b["step_id"].is_null())
    });
    let step = match execution.and_then(|e| e["phase"].as_str()) {
        Some("discover") => "查找和选择材料",
        Some("analyze") => "分析已选材料",
        Some("execute") => "本机写入或运行",
        _ => match b["step_kind"].as_str() {
            Some("job") => "模型判断或结果检查（保留已有执行结果）",
            Some("execution") => "执行处理（先核对已有运行与产物）",
            _ => "根据当前记录定位未完成步骤",
        },
    };
    lines.push(format!("停在：{step}"));
    let next = b["owner"].as_str().or(task["next_owner"].as_str());
    if let Some(next) = next {
        lines.push(format!(
            "下一步：@{next}{}",
            if next == owner {
                "（本机）"
            } else {
                "（对方电脑）"
            }
        ));
    }
    let runs = task["runs"].as_array().map_or(0, |runs| {
        runs.iter()
            .filter(|r| r["revision"] == task["revision"])
            .count()
    });
    let materials = task["materials"].as_array().map_or(0, |items| {
        items
            .iter()
            .filter(|m| m["revision"] == task["revision"])
            .count()
    });
    lines.push(format!(
        "已保留：本机记录有 {runs} 次运行、{materials} 份材料快照；已有文件和对话不清除。"
    ));
    if let Some(body) = task["result"]["body"].as_str() {
        lines.push(format!("已记录结果：{body}"));
    }
    if let Some(work) = task["saved_work"].as_array().and_then(|work| work.first()) {
        if let Some(body) = work["body"].as_str() {
            lines.push(format!("已保存的阶段发现（待最终核对）：{body}"));
        }
    }
    match b["reason"].as_str() {
        Some("execution_uncertain") => lines.push(
            "先核对已有成果和外部操作是否完成；无法确认时继续询问，不能盲目重跑。".into(),
        ),
        Some("material_input") => lines.push(
            "可补充应保留或跳过哪些材料；续接时调整材料并复用已有发现。".into(),
        ),
        Some("model_configuration") => lines.push(
            "由上面的责任成员检查模型凭据、额度或配置；配置修复后选择续接。".into(),
        ),
        Some("user_input") => lines.push("回答具体问题后续接；普通补充不会代替答案。".into()),
        Some("local_permission") if task["execution_permission"]["can_allow"] == true =>
            lines.push("在执行电脑查看并批准任务；补充不会代替授权。".into()),
        Some("local_permission") => lines.push(
            "由执行成员检查具体路径的可读权限和允许的目录（/roots）；修复后可重新检查并续接，程序会再次验证权限。".into()),
        _ => lines.push("可先补充纠正，再继续未完成步骤；已有成功结果优先复用。".into()),
    }
    if let Some(stop) = presentation::stop_status(task) {
        lines.push(stop);
    }
    if member_withdrew(task, owner) {
        lines.push("参与方已停止本次任务；发起人可修改要求并重新确认，或停止并保留记录。".into());
    }
    lines
}

pub(super) fn render(
    f: &mut ratatui::Frame,
    area: Rect,
    task: &Value,
    owner: &str,
    selected: usize,
    scroll: usize,
) {
    let actions = actions(task, owner);
    let menu_height = (actions.len() as u16 + 2).min(area.height.saturating_sub(5));
    let split = Layout::vertical([Constraint::Min(3), Constraint::Length(menu_height)]).split(area);
    draw_lines(
        f,
        split[0],
        "处理当前任务 · PgUp/PgDn 查看原因",
        &lines(task, owner),
        scroll,
        false,
    );
    let menu = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            format!(
                "{} {}",
                if i == selected { "▶" } else { " " },
                a.label(task, owner)
            )
        })
        .collect::<Vec<_>>();
    draw_lines(
        f,
        split[1],
        "↑↓ 选择 · Enter 打开 · 字母直达",
        &menu,
        selected.saturating_sub(split[1].height.saturating_sub(3) as usize),
        false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn blocked() -> Value {
        json!({"state":"needs_attention","revision":2,"confirmed":2,"initiator":"a","executors":["b"],
            "blocker":{"id":"blocked","owner":"b","step_kind":"execution","step_id":"discovery","reason":"execution_error","detail":"material changed during snapshot: report.txt"},
            "executions":[{"id":"discovery","phase":"discover"}],
            "runs":[{"revision":1},{"revision":2}],"materials":[{"revision":2}]})
    }

    #[test]
    fn concrete_error_and_saved_evidence_remain_visible() {
        let text = lines(&blocked(), "b").join("\n");
        assert!(text.contains("material changed during snapshot: report.txt"));
        assert!(text.contains("查找和选择材料"));
        assert!(text.contains("@b（本机）"));
        assert!(text.contains("1 次运行、1 份材料快照"));
    }

    #[test]
    fn executor_has_correction_and_resume_without_goal_or_permission_changes() {
        let task = blocked();
        let offered = actions(&task, "b");
        assert_eq!(offered[0], Action::Supplement);
        assert!(offered.contains(&Action::Retry));
        assert!(offered.contains(&Action::Stop));
        assert!(!offered.contains(&Action::Revise));
        assert!(!offered.contains(&Action::Approve));
        assert_eq!(actions(&task, "c"), vec![Action::Detail, Action::Back]);
    }

    #[test]
    fn questions_authorization_and_stop_checks_never_offer_blind_retry() {
        let mut task = blocked();
        task["blocker"]["reason"] = json!("local_permission");
        task["execution_permission"] = json!({"can_allow":true});
        assert_eq!(actions(&task, "b")[0], Action::Approve);
        assert!(!actions(&task, "b").contains(&Action::Retry));
        task["execution_permission"] = Value::Null;
        task["blocker"]["reason"] = json!("user_input");
        task["questions"] = json!([{"revision":2,"state":"user"}]);
        assert_eq!(actions(&task, "a")[0], Action::Answer);
        assert!(!actions(&task, "a").contains(&Action::Retry));
        task["blocker"]["reason"] = json!("execution_uncertain");
        task["assignment_stops"] = json!([{"executor":"b","state":"needs_check"}]);
        assert_eq!(actions(&task, "b")[0], Action::Handoff);
        assert!(!actions(&task, "b").contains(&Action::Retry));
        assert!(!actions(&task, "b").contains(&Action::Supplement));
    }

    #[test]
    fn repaired_filesystem_permission_has_a_recheck_without_new_approval() {
        let mut task = blocked();
        task["blocker"]["reason"] = json!("local_permission");
        task["execution_permission"] = json!({"can_allow":false});
        assert!(actions(&task, "b").contains(&Action::Retry));
        assert!(!actions(&task, "b").contains(&Action::Approve));
        assert!(Action::Retry.label(&task, "b").contains("重新检查"));
        task["confirmed"] = Value::Null;
        assert!(!actions(&task, "a").contains(&Action::Supplement));
        assert!(actions(&task, "a").contains(&Action::Revise));
        assert!(actions(&task, "a").contains(&Action::Retry));
        task["history"] = json!([{"revision":2,"sender":"b","kind":"progress","payload":{"stage":"member_stopped"}}]);
        assert!(!actions(&task, "a").contains(&Action::Retry));
        assert!(actions(&task, "a").contains(&Action::Revise));
    }

    #[test]
    fn long_errors_do_not_hide_controls_and_uncertain_execution_is_explicit() {
        let mut task = blocked();
        task["blocker"]["reason"] = json!("execution_uncertain");
        task["blocker"]["detail"] = json!("a long error message ".repeat(100));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|f| render(f, f.area(), &task, "b", 1, 0))
            .unwrap();
        let screen = format!("{:?}", terminal.backend().buffer());
        assert!(screen.contains("先核对已有成果与副作用"));
        assert!(screen.contains("补充本机约束或纠正"));
        assert!(screen.contains("返回原任务"));
        task["state"] = json!("completed");
        assert_eq!(actions(&task, "a"), vec![Action::Detail, Action::Back]);
    }
}
