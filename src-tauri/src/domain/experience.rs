use std::collections::HashMap;

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
/// 只针对「可放权」步：agent_step / mcp_tool 且当前 owner=human；
/// human_gate / acceptance 是结构性人工检查点，永不作为候选；资金/不可逆工具恒不可升级。
pub fn upgrade_candidates(
    steps: &[StepRow],
    corrections: &[CorrectionRow],
    unsafe_tool_refs: &[String],
    run_sops: &HashMap<String, String>,
    threshold: usize,
) -> Vec<UpgradeCandidate> {
    let th = threshold.max(1);
    let mut out = Vec::new();
    // 只看 human owner 且类型可放权的 step。
    for s in steps
        .iter()
        .filter(|s| s.owner == "human" && matches!(s.node_type.as_str(), "agent_step" | "mcp_tool"))
    {
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
            sop_id: run_sops.get(&s.run_id).cloned().unwrap_or_default(),
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

/// 生成放权后的 SOP 定义（纯逻辑，不落库）：把指定 node 的 config.owner 改为 agent。
/// 结构性人工检查点与资金/不可逆工具在此硬拦，永不放权。返回新的 definition JSON 字符串。
pub fn plan_owner_upgrade(
    definition_json: &str,
    node_id: &str,
    unsafe_tool_refs: &[String],
) -> AppResult<String> {
    let mut def: Value = serde_json::from_str(definition_json)
        .map_err(|e| AppError::InvalidInput(format!("SOP 定义非法：{}", e)))?;
    let nodes = def
        .get_mut("nodes")
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| AppError::InvalidInput("SOP 定义缺 nodes 数组".into()))?;
    let node = nodes
        .iter_mut()
        .find(|n| n.get("id").and_then(|v| v.as_str()) == Some(node_id))
        .ok_or_else(|| AppError::NotFound(format!("节点 {}", node_id)))?;
    let node_type = node.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if !matches!(node_type.as_str(), "agent_step" | "mcp_tool") {
        return Err(AppError::RiskBlocked(
            "human_gate / acceptance 是结构性人工检查点，不可放权".into(),
        ));
    }
    if node_type == "mcp_tool" {
        if let Some(tool) = node
            .get("config")
            .and_then(|c| c.get("tool"))
            .and_then(|v| v.as_str())
        {
            if unsafe_tool_refs.iter().any(|u| u == tool) {
                return Err(AppError::RiskBlocked("资金/不可逆动作永不升级为 agent".into()));
            }
        }
    }
    // config 必须是对象；缺省则补一个空对象，再写入 owner=agent。
    if node.get("config").and_then(|v| v.as_object()).is_none() {
        node["config"] = json!({});
    }
    node["config"]["owner"] = json!("agent");
    serde_json::to_string(&def).map_err(|e| AppError::InvalidInput(e.to_string()))
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
        let mut sops = HashMap::new();
        sops.insert("run1".into(), "sop1".into());
        let few = vec![corr("s1"), corr("s1")];
        let c = upgrade_candidates(&steps, &few, &[], &sops, 3);
        assert!(c.is_empty(), "未达阈值不出候选");
        let many = vec![corr("s1"), corr("s1"), corr("s1")];
        let c = upgrade_candidates(&steps, &many, &[], &sops, 3);
        assert_eq!(c.len(), 1);
        assert!(c[0].eligible);
        assert_eq!(c[0].sop_id, "sop1", "候选须带 sop_id 才能定位放权");
        // 资金/不可逆工具永不升级。
        let c = upgrade_candidates(
            &steps,
            &many,
            &["demo:check_anomalies".to_string()],
            &sops,
            3,
        );
        assert_eq!(c.len(), 1);
        assert!(!c[0].eligible);
        // 结构性人工检查点永不作为候选。
        let gate = vec![step("s3", "human", "done", "human_gate")];
        let c = upgrade_candidates(&gate, &many, &[], &sops, 3);
        assert!(c.is_empty(), "human_gate 不可放权");
    }

    #[test]
    fn plan_owner_upgrade_hard_blocks_structural_and_unsafe() {
        let def = json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"mcp_tool","config":{"tool":"safe:read","owner":"human"}},
                {"id":"n2","type":"human_gate","config":{}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges": []
        }).to_string();
        // 合法放权：写 owner=agent。
        let out = plan_owner_upgrade(&def, "n1", &[]).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["nodes"][1]["config"]["owner"], "agent");
        // 结构性人工点拒绝。
        assert!(plan_owner_upgrade(&def, "n2", &[]).is_err());
        // 资金/不可逆工具拒绝。
        assert!(plan_owner_upgrade(&def, "n1", &["safe:read".into()]).is_err());
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

        // 结构性人工检查点（human_gate）上的修正不产生候选——只有 agent_step/mcp_tool 可放权。
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
        let sops = store::run_sop_map(&c).unwrap();
        let cands = upgrade_candidates(&steps, &corrs, &[], &sops, 3);
        assert!(cands.is_empty(), "human_gate 不可放权");

        // 效率报表不编金额。
        let rep = efficiency_report(1, 1, &steps, 1, 1);
        assert!(!rep.saved_steps_note.contains("元"));
    }

    /// 端到端：human-owned agent_step → 修正累积 → 候选 → 人确认放权 → 后续运行不再停人工门。
    #[test]
    fn owner_upgrade_end_to_end() {
        use crate::models::SopUpsertPayload;
        let c = db::open_in_memory().unwrap();
        executor::demo_install(&c).unwrap(); // 注册 demo 工具 + 授权
        let def = json!({
            "version": 1,
            "nodes": [
                {"id":"n0","type":"start","config":{}},
                {"id":"n1","type":"agent_step","config":{"goal":"汇总","owner":"human"}},
                {"id":"n2","type":"acceptance","config":{"criteria":["steps.n1.mock == true"]}},
                {"id":"n3","type":"end","config":{}}
            ],
            "edges": [
                {"from":"n0","to":"n1"},{"from":"n1","to":"n2"},{"from":"n2","to":"n3"}
            ]
        });
        let sop_id = store::upsert_sop(&c, &SopUpsertPayload {
            id: Some("sop-delegation-test".into()),
            name: "放权测试".into(),
            trigger_json: "{}".into(),
            risk: "low".into(),
            owner_policy: "hybrid".into(),
            definition: def,
            expect_version: None,
        }).unwrap().0;

        // 首跑：human-owned agent_step 必须先过 owner 确认门。
        let run1 = executor::start_run(
            &c, &sop_id, "build", "manual", &json!({}), &DemoTools, &MockProvider,
        ).unwrap();
        assert_eq!(store::get_run(&c, &run1).unwrap().status, "wait_human");
        let gates = store::list_gates(&c, true).unwrap();
        assert_eq!(gates.len(), 1);
        assert_eq!(gates[0].reason, "owner_review");
        let human_step = store::list_steps(&c, &run1).unwrap()
            .into_iter().find(|s| s.owner == "human").unwrap();
        assert_eq!(human_step.node_type, "agent_step");
        store::decide_gate(&c, &gates[0].id, "approve", None).unwrap();
        assert_eq!(
            executor::resume_after_gate(&c, &run1, &DemoTools, &MockProvider).unwrap(),
            "done"
        );

        // 累积 3 条人工修正 → 出候选，且带 sop_id。
        for _ in 0..3 {
            store::add_correction(&c, &human_step.id, &CorrectionInput {
                step_id: human_step.id.clone(),
                original_json: "{}".into(),
                corrected_json: "{\"ok\":true}".into(),
                note: None,
            }).unwrap();
        }
        let sops = store::run_sop_map(&c).unwrap();
        let cands = upgrade_candidates(
            &store::all_steps(&c).unwrap(),
            &store::list_corrections(&c, None).unwrap(),
            &[], &sops, 3,
        );
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].sop_id, sop_id);
        assert!(cands[0].eligible);

        // 应用放权：plan_owner_upgrade 产出新定义，set_node_owner 落库并递增版本，审计留痕。
        let curr = store::get_sop(&c, &sop_id).unwrap();
        let new_def = plan_owner_upgrade(&curr.definition, "n1", &[]).unwrap();
        let v2 = store::set_node_owner(&c, &sop_id, &new_def).unwrap();
        assert_eq!(v2, curr.version + 1);
        store::insert_owner_decision(
            &c, &sop_id, "n1", "agent_step", "human", "agent", "approve", 3, None,
        ).unwrap();
        let after = store::get_sop(&c, &sop_id).unwrap();
        let v: Value = serde_json::from_str(&after.definition).unwrap();
        assert_eq!(v["nodes"][1]["config"]["owner"], "agent");
        assert_eq!(store::list_owner_decisions(&c, Some(&sop_id)).unwrap().len(), 1);

        // 再跑：不再停 owner 门，直接 done；新 step owner 已是 agent。
        let run2 = executor::start_run(
            &c, &sop_id, "build", "manual", &json!({}), &DemoTools, &MockProvider,
        ).unwrap();
        assert_eq!(store::get_run(&c, &run2).unwrap().status, "done");
        let s2 = store::list_steps(&c, &run2).unwrap()
            .into_iter().find(|s| s.node_id.as_deref() == Some("n1")).unwrap();
        assert_eq!(s2.owner, "agent");
    }
}
