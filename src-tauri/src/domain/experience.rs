use serde_json::{json, Value};

use crate::models::{
    AppError, AppResult, CorrectionRow, EfficiencyReport, StepRow, TemplateBundle,
    UpgradeCandidate,
};

/// 模板清单 schema 版本（§6 manifest.json）。
pub const MANIFEST_SCHEMA: i32 = 1;
/// owner 升级候选的修正累积阈值（框架常量，后续可挪配置）。
pub const UPGRADE_THRESHOLD: usize = 3;

/// 从一条「实测通过」的 run 捕获经验模板（§5.5）。
/// validated 只在 run 状态 done 且 verdict=pass 时为真——未实测通过不得上岗。
pub fn capture_template(
    run_status: &str,
    run_verdict: Option<&str>,
    name: &str,
    sop_snapshot: &str,
    steps: &[StepRow],
    _acceptance: &[Value],
    corrections: &[CorrectionRow],
    source_shop: Option<&str>,
) -> AppResult<TemplateBundle> {
    if name.trim().is_empty() {
        return Err(AppError::InvalidInput("模板名不得为空".into()));
    }
    let validated = run_status == "done" && run_verdict == Some("pass");
    let trajectory: Vec<Value> = steps
        .iter()
        .map(|s| {
            json!({
                "nodeType": s.node_type,
                "owner": s.owner,
                "toolRef": s.tool_ref,
                "status": s.status,
                "output": s.output_json,
            })
        })
        .collect();
    Ok(TemplateBundle {
        manifest_schema: MANIFEST_SCHEMA,
        name: name.to_string(),
        validated,
        sop_snapshot: sop_snapshot.to_string(),
        trajectory_json: serde_json::to_string(&trajectory).unwrap_or_else(|_| "[]".into()),
        corrections_json: serde_json::to_string(corrections).unwrap_or_else(|_| "[]".into()),
        source_shop: source_shop.map(|s| s.to_string()),
    })
}

/// 导入校验（§6）：schema 版本匹配 + validated=1，否则拒绝（未实测通过的模板不得上岗）。
pub fn validate_import(bundle: &TemplateBundle) -> AppResult<()> {
    if bundle.manifest_schema != MANIFEST_SCHEMA {
        return Err(AppError::InvalidInput(format!(
            "模板 schema 不兼容：{} != {}",
            bundle.manifest_schema, MANIFEST_SCHEMA
        )));
    }
    if !bundle.validated {
        return Err(AppError::InvalidInput("模板未实测通过（validated=0），拒绝导入".into()));
    }
    // definition 必须是合法 JSON。
    serde_json::from_str::<Value>(&bundle.sop_snapshot)
        .map_err(|e| AppError::InvalidInput(format!("SOP 快照非法：{}", e)))?;
    Ok(())
}

/// owner 升级候选（§5.5）：某 node 的人工修正累积到阈值 → 候选，需人确认，不自动放权。
/// 资金/不可逆工具恒不可升级。
pub fn upgrade_candidates(
    steps: &[StepRow],
    corrections: &[CorrectionRow],
    unsafe_tool_refs: &[String],
    threshold: usize,
) -> Vec<UpgradeCandidate> {
    let th = threshold.max(1);
    let mut out = Vec::new();
    // 只看 human owner 的 step。
    for s in steps.iter().filter(|s| s.owner == "human") {
        let n = corrections.iter().filter(|c| c.step_id == s.id).count();
        if n < th {
            continue;
        }
        let unsafe_tool = s
            .tool_ref
            .as_ref()
            .map(|t| unsafe_tool_refs.iter().any(|u| u == t))
            .unwrap_or(false);
        out.push(UpgradeCandidate {
            node_type: s.node_type.clone(),
            node_id: s.node_id.clone().unwrap_or_else(|| s.id.clone()),
            correction_count: n,
            threshold: th,
            eligible: !unsafe_tool,
            reason: if unsafe_tool {
                "资金/不可逆动作永不升级为 agent".into()
            } else {
                "人工修正累积达阈值，仅候选，需人确认".into()
            },
        });
    }
    out
}

/// 效率报表（§5.4/§5.5）：自动化率、人工介入、验收轮数；节省按步数陈述，不编金额。
pub fn efficiency_report(
    runs_total: usize,
    runs_completed: usize,
    steps: &[StepRow],
    human_interventions: usize,
    acceptance_checks: usize,
) -> EfficiencyReport {
    let total = steps.len();
    let agent_done = steps
        .iter()
        .filter(|s| s.owner == "agent" && s.status == "done")
        .count();
    let rate = if total == 0 { 0.0 } else { agent_done as f64 / total as f64 };
    let rounds = if runs_completed == 0 {
        0.0
    } else {
        acceptance_checks as f64 / runs_completed as f64
    };
    // 不编金额：只陈述步数/次数这一本地事实。
    let saved_steps_note = format!(
        "Agent 独立完成 {} / {} 步（{:.0}%）；人工介入 {} 次",
        agent_done, total, rate * 100.0, human_interventions
    );
    EfficiencyReport {
        total_runs: runs_total,
        completed_runs: runs_completed,
        total_steps: total,
        agent_done_steps: agent_done,
        automation_rate: rate,
        human_interventions,
        avg_acceptance_rounds: rounds,
        saved_steps_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, owner: &str, status: &str, node_type: &str) -> StepRow {
        StepRow {
            id: id.into(),
            run_id: "run1".into(),
            seq: 1,
            node_type: node_type.into(),
            owner: owner.into(),
            tool_ref: Some("demo:check_anomalies".into()),
            node_id: None,
            status: status.into(),
            input_json: None,
            output_json: None,
            started_at: None,
            finished_at: None,
        }
    }
    fn corr(step_id: &str) -> CorrectionRow {
        CorrectionRow {
            id: 1,
            step_id: step_id.into(),
            run_id: "run1".into(),
            node_type: "agent_step".into(),
            original_json: "{}".into(),
            corrected_json: "{}".into(),
            note: None,
            author: "human".into(),
            created_at: 0,
        }
    }

    #[test]
    fn capture_only_validates_passed_run() {
        let s = vec![step("s1", "agent", "done", "mcp_tool")];
        let ok = capture_template("done", Some("pass"), "T", "{}", &s, &[], &[], None).unwrap();
        assert!(ok.validated);
        let partial = capture_template("done", Some("partial"), "T", "{}", &s, &[], &[], None).unwrap();
        assert!(!partial.validated, "未全过不得 validated");
        let running = capture_template("running", None, "T", "{}", &s, &[], &[], None).unwrap();
        assert!(!running.validated);
    }

    #[test]
    fn import_rejects_bad_schema_and_unvalidated() {
        let b = TemplateBundle {
            manifest_schema: 99,
            name: "x".into(),
            validated: true,
            sop_snapshot: "{}".into(),
            trajectory_json: "[]".into(),
            corrections_json: "[]".into(),
            source_shop: None,
        };
        assert!(validate_import(&b).is_err());
        let b2 = TemplateBundle { manifest_schema: MANIFEST_SCHEMA, validated: false, ..b };
        assert!(validate_import(&b2).is_err());
        let b3 = TemplateBundle { validated: true, sop_snapshot: "not-json".into(), ..b2 };
        assert!(validate_import(&b3).is_err());
        let b4 = TemplateBundle {
            validated: true,
            manifest_schema: MANIFEST_SCHEMA,
            sop_snapshot: "{}".into(),
            ..b3
        };
        assert!(validate_import(&b4).is_ok());
    }

    #[test]
    fn upgrade_candidate_needs_threshold_and_never_for_irreversible() {
        let steps = vec![
            step("s1", "human", "done", "mcp_tool"),
            step("s2", "human", "done", "mcp_tool"),
        ];
        let few = vec![corr("s1"), corr("s1")];
        let c = upgrade_candidates(&steps, &few, &[], 3);
        assert!(c.is_empty(), "未达阈值不出候选");
        let many = vec![corr("s1"), corr("s1"), corr("s1")];
        let c = upgrade_candidates(&steps, &many, &[], 3);
        assert_eq!(c.len(), 1);
        assert!(c[0].eligible);
        // 资金/不可逆工具永不升级。
        let c = upgrade_candidates(
            &steps,
            &many,
            &["demo:check_anomalies".to_string()],
            3,
        );
        assert_eq!(c.len(), 1);
        assert!(!c[0].eligible);
    }

    #[test]
    fn efficiency_report_states_facts_not_money() {
        let steps = vec![
            step("s1", "agent", "done", "mcp_tool"),
            step("s2", "agent", "done", "mcp_tool"),
            step("s3", "human", "done", "human_gate"),
        ];
        let r = efficiency_report(4, 4, &steps, 2, 4);
        assert_eq!(r.total_steps, 3);
        assert_eq!(r.agent_done_steps, 2);
        assert!((r.automation_rate - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(r.human_interventions, 2);
        assert!(!r.saved_steps_note.contains("元"), "不得编金额");
        assert!(!r.saved_steps_note.contains("¥"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::domain::executor::{self, DemoTools};
    use crate::domain::llm::MockProvider;
    use crate::infra::{db, store, template_zip};
    use crate::models::CorrectionInput;

    #[test]
    fn full_experience_cycle() {
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap();
        // 跑通一条到 done（含人工门批准）。
        let run_id = executor::start_run(
            &c, "sop-demo-daily-anomaly", "build", "manual", &json!({}), &DemoTools, &MockProvider,
        ).unwrap();
        let gate = store::list_gates(&c, true).unwrap()[0].clone();
        store::decide_gate(&c, &gate.id, "approve", None).unwrap();
        assert_eq!(executor::resume_after_gate(&c, &run_id, &DemoTools, &MockProvider).unwrap(), "done");

        // 捕获模板：validated=1（实测通过）。
        let detail = store::run_detail(&c, &run_id).unwrap();
        let bundle = capture_template(
            &detail.run.status, detail.run.verdict.as_deref(), "巡检经验",
            detail.run.definition_snapshot.as_deref().unwrap_or("{}"),
            &detail.steps, &[], &[], Some("shop-1"),
        ).unwrap();
        assert!(bundle.validated);
        validate_import(&bundle).unwrap();

        // zip 导出导入 roundtrip。
        let bytes = template_zip::export_zip(&bundle).unwrap();
        let back = template_zip::import_zip(&bytes).unwrap();
        validate_import(&back).unwrap();
        let tid = store::save_template(&c, &back).unwrap();
        assert_eq!(store::list_templates(&c).unwrap().len(), 1);
        assert!(!tid.is_empty());

        // 人工修正累积 → 升级候选（资金/不可逆恒不可升级）。
        let step = detail.steps.iter().find(|s| s.owner == "human").unwrap();
        for _ in 0..3 {
            store::add_correction(&c, &step.id, &CorrectionInput {
                step_id: step.id.clone(),
                original_json: "{\"v\":0}".into(),
                corrected_json: "{\"v\":1}".into(),
                note: None,
            }).unwrap();
        }
        let steps = store::all_steps(&c).unwrap();
        let corrs = store::list_corrections(&c, None).unwrap();
        let cands = upgrade_candidates(&steps, &corrs, &[], 3);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].correction_count, 3);
        assert!(cands[0].eligible);

        // 效率报表不编金额。
        let rep = efficiency_report(1, 1, &steps, 1, 1);
        assert!(!rep.saved_steps_note.contains("元"));
    }
}
