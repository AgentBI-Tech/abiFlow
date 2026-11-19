use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{
    now_secs, new_id, AcceptanceRow, AppError, AppResult, DashboardData, GateRow, RunDetail,
    RunRow, ScheduleInfo, SopRow, SopUpsertPayload, StepRow,
};

// ---- SOP ----

pub fn get_sop(conn: &Connection, id: &str) -> AppResult<SopRow> {
    conn.query_row(
        "SELECT id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition \
         FROM sops WHERE id=?1",
        params![id],
        |r| Ok(SopRow {
            id: r.get(0)?,
            name: r.get(1)?,
            version: r.get(2)?,
            trigger_json: r.get(3)?,
            risk: r.get(4)?,
            owner_policy: r.get(5)?,
            enabled: r.get::<_, i64>(6)? != 0,
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
            definition: r.get(9)?,
        }),
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))?
    .ok_or_else(|| AppError::NotFound(format!("SOP {}", id)))
}

pub fn list_sops(conn: &Connection, enabled_only: bool) -> AppResult<Vec<SopRow>> {
    let sql = if enabled_only {
        "SELECT id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition \
         FROM sops WHERE enabled=1 ORDER BY updated_at DESC"
    } else {
        "SELECT id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition \
         FROM sops ORDER BY updated_at DESC"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SopRow {
                id: r.get(0)?,
                name: r.get(1)?,
                version: r.get(2)?,
                trigger_json: r.get(3)?,
                risk: r.get(4)?,
                owner_policy: r.get(5)?,
                enabled: r.get::<_, i64>(6)? != 0,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
                definition: r.get(9)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

/// 新建或改版；带乐观锁（expect_version）。返回 (id, version)。
pub fn upsert_sop(conn: &Connection, p: &SopUpsertPayload) -> AppResult<(String, i32)> {
    let def_json = serde_json::to_string(&p.definition)
        .map_err(|e| AppError::InvalidInput(format!("definition 无法序列化：{}", e)))?;
    let now = now_secs();

    if let Some(id) = &p.id {
        let current: Option<i32> = conn
            .query_row("SELECT version FROM sops WHERE id=?1", params![id], |r| r.get(0))
            .optional()
            .map_err(|e| AppError::Db(e.to_string()))?;
        match current {
            Some(v) => {
                if let Some(ev) = p.expect_version {
                    if v != ev {
                        return Err(AppError::VersionConflict);
                    }
                }
                let new_version = v + 1;
                conn.execute(
                    "UPDATE sops SET name=?2, version=?3, trigger_json=?4, risk=?5, \
                     owner_policy=?6, definition=?7, updated_at=?8 WHERE id=?1",
                    params![
                        id, p.name, new_version, p.trigger_json, p.risk, p.owner_policy,
                        def_json, now
                    ],
                )
                .map_err(|e| AppError::Db(e.to_string()))?;
                Ok((id.clone(), new_version))
            }
            None => {
                // 指定 id 但不存在：当作新建，版本从 1 起。
                let version = 1;
                conn.execute(
                    "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
                     VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7,?8)",
                    params![
                        id, p.name, version, p.trigger_json, p.risk, p.owner_policy, now, def_json
                    ],
                )
                .map_err(|e| AppError::Db(e.to_string()))?;
                Ok((id.clone(), version))
            }
        }
    } else {
        let id = new_id("sop");
        let version = 1;
        conn.execute(
            "INSERT INTO sops(id,name,version,trigger_json,risk,owner_policy,enabled,created_at,updated_at,definition) \
             VALUES(?1,?2,?3,?4,?5,?6,1,?7,?7,?8)",
            params![
                id, p.name, version, p.trigger_json, p.risk, p.owner_policy, now, def_json
            ],
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
        Ok((id, version))
    }
}

pub fn set_sop_enabled(conn: &Connection, id: &str, enabled: bool) -> AppResult<()> {
    let changed = conn
        .execute(
            "UPDATE sops SET enabled=?2, updated_at=?3 WHERE id=?1",
            params![id, enabled as i64, now_secs()],
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    if changed == 0 {
        return Err(AppError::NotFound(format!("SOP {}", id)));
    }
    Ok(())
}

/// 当前启用的工具 id 集合，供 AST 校验（§4）。
pub fn known_tools(conn: &Connection) -> AppResult<HashSet<String>> {
    let mut stmt = conn
        .prepare("SELECT id FROM tools WHERE enabled=1")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn tool_risk(conn: &Connection, tool: &str) -> AppResult<(String, bool)> {
    conn.query_row(
        "SELECT risk, scope_json FROM tools WHERE id=?1",
        params![tool],
        |r| {
            let risk: String = r.get(0)?;
            let scope: String = r.get(1)?;
            let irreversible = scope.contains("irreversible");
            Ok((risk, irreversible))
        },
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))?
    .ok_or_else(|| AppError::NotFound(format!("工具 {}", tool)))
}

// ---- Run / Step / Gate / Acceptance ----

#[allow(clippy::too_many_arguments)]
pub fn insert_run(
    conn: &Connection,
    sop_id: &str,
    sop_version: i32,
    mode: &str,
    trigger_by: &str,
    definition_snapshot: &str,
) -> AppResult<String> {
    let id = new_id("run");
    conn.execute(
        "INSERT INTO runs(id,sop_id,sop_version,status,mode,trigger_by,started_at,definition_snapshot,current_node,outputs_json,branch_skipped) \
         VALUES(?1,?2,?3,'pending',?4,?5,?6,?7,NULL,'{}','[]')",
        params![id, sop_id, sop_version, mode, trigger_by, now_secs(), definition_snapshot],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(id)
}

pub fn get_run(conn: &Connection, id: &str) -> AppResult<RunRow> {
    conn.query_row(
        "SELECT id,sop_id,sop_version,status,mode,trigger_by,started_at,finished_at,verdict,note, \
         definition_snapshot,current_node,outputs_json,branch_skipped FROM runs WHERE id=?1",
        params![id],
        |r| Ok(RunRow {
            id: r.get(0)?,
            sop_id: r.get(1)?,
            sop_version: r.get(2)?,
            status: r.get(3)?,
            mode: r.get(4)?,
            trigger_by: r.get(5)?,
            started_at: r.get(6)?,
            finished_at: r.get(7)?,
            verdict: r.get(8)?,
            note: r.get(9)?,
            definition_snapshot: r.get(10)?,
            current_node: r.get(11)?,
            outputs_json: r.get(12)?,
            branch_skipped: r.get(13)?,
        }),
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))?
    .ok_or_else(|| AppError::NotFound(format!("运行 {}", id)))
}

pub fn list_runs(
    conn: &Connection,
    sop_id: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> AppResult<Vec<RunRow>> {
    let mut sql = String::from(
        "SELECT id,sop_id,sop_version,status,mode,trigger_by,started_at,finished_at,verdict,note, \
         definition_snapshot,current_node,outputs_json,branch_skipped FROM runs WHERE 1=1",
    );
    let sop_owned = sop_id.map(|s| s.to_string());
    let status_owned = status.map(|s| s.to_string());
    let mut values: Vec<&dyn rusqlite::ToSql> = Vec::new();
    if let Some(ref s) = sop_owned {
        sql.push_str(" AND sop_id=?");
        values.push(s);
    }
    if let Some(ref s) = status_owned {
        sql.push_str(" AND status=?");
        values.push(s);
    }
    sql.push_str(" ORDER BY COALESCE(started_at,0) DESC LIMIT ?");
    values.push(&limit);

    let mut stmt = conn.prepare(&sql).map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(values.as_slice(), |r| {
            Ok(RunRow {
                id: r.get(0)?,
                sop_id: r.get(1)?,
                sop_version: r.get(2)?,
                status: r.get(3)?,
                mode: r.get(4)?,
                trigger_by: r.get(5)?,
                started_at: r.get(6)?,
                finished_at: r.get(7)?,
                verdict: r.get(8)?,
                note: r.get(9)?,
                definition_snapshot: r.get(10)?,
                current_node: r.get(11)?,
                outputs_json: r.get(12)?,
                branch_skipped: r.get(13)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn update_run_status(
    conn: &Connection,
    id: &str,
    status: &str,
    verdict: Option<&str>,
    note: Option<&str>,
) -> AppResult<()> {
    let finished = match status {
        "done" | "failed" | "cancelled" => Some(now_secs()),
        _ => None,
    };
    conn.execute(
        "UPDATE runs SET status=?2, verdict=COALESCE(?3,verdict), note=COALESCE(?4,note), \
         finished_at=COALESCE(?5,finished_at) WHERE id=?1",
        params![id, status, verdict, note, finished],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn update_run_cursor(
    conn: &Connection,
    id: &str,
    current_node: Option<&str>,
    outputs_json: &str,
    branch_skipped: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE runs SET current_node=?2, outputs_json=?3, branch_skipped=?4 WHERE id=?1",
        params![id, current_node, outputs_json, branch_skipped],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn insert_step(
    conn: &Connection,
    id: &str,
    run_id: &str,
    seq: i32,
    node_type: &str,
    owner: &str,
    node_id: Option<&str>,
    tool_ref: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO steps(id,run_id,seq,node_type,owner,tool_ref,node_id,status) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,'pending')",
        params![id, run_id, seq, node_type, owner, tool_ref, node_id],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn update_step(
    conn: &Connection,
    id: &str,
    status: &str,
    input_json: Option<&str>,
    output_json: Option<&str>,
) -> AppResult<()> {
    let started = if status == "running" || status == "done" || status == "failed" {
        Some(now_secs())
    } else {
        None
    };
    let finished = if status == "done" || status == "failed" || status == "skipped" {
        Some(now_secs())
    } else {
        None
    };
    conn.execute(
        "UPDATE steps SET status=?2, input_json=COALESCE(?3,input_json), \
         output_json=COALESCE(?4,output_json), started_at=COALESCE(?5,started_at), \
         finished_at=COALESCE(?6,finished_at) WHERE id=?1",
        params![id, status, input_json, output_json, started, finished],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn list_steps(conn: &Connection, run_id: &str) -> AppResult<Vec<StepRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id,run_id,seq,node_type,owner,tool_ref,status,input_json,output_json,started_at,finished_at \
             FROM steps WHERE run_id=?1 ORDER BY seq",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![run_id], |r| {
            Ok(StepRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                seq: r.get(2)?,
                node_type: r.get(3)?,
                owner: r.get(4)?,
                tool_ref: r.get(5)?,
                status: r.get(6)?,
                input_json: r.get(7)?,
                output_json: r.get(8)?,
                started_at: r.get(9)?,
                finished_at: r.get(10)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn step_id_for_node(conn: &Connection, run_id: &str, node_id: &str) -> AppResult<Option<String>> {
    conn.query_row(
        "SELECT id FROM steps WHERE run_id=?1 AND node_id=?2 ORDER BY seq LIMIT 1",
        params![run_id, node_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))
}

pub fn insert_log(
    conn: &Connection,
    step_id: &str,
    actor: &str,
    tool_command: Option<&str>,
    input_redact: Option<&str>,
    output_redact: Option<&str>,
    verdict: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO step_logs(step_id,actor,tool_command,input_redact,output_redact,verdict,ts) \
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            step_id, actor, tool_command, input_redact, output_redact, verdict, now_secs()
        ],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn insert_gate(
    conn: &Connection,
    run_id: &str,
    step_id: &str,
    reason: &str,
    prompt: &str,
    payload_json: Option<&str>,
) -> AppResult<String> {
    let id = new_id("gate");
    conn.execute(
        "INSERT INTO human_gates(id,run_id,step_id,reason,prompt,payload_json,requested_at) \
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![id, run_id, step_id, reason, prompt, payload_json, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(id)
}

pub fn get_gate(conn: &Connection, id: &str) -> AppResult<GateRow> {
    conn.query_row(
        "SELECT id,run_id,step_id,reason,prompt,payload_json,requested_at,decided_at,decision,note \
         FROM human_gates WHERE id=?1",
        params![id],
        |r| Ok(GateRow {
            id: r.get(0)?,
            run_id: r.get(1)?,
            step_id: r.get(2)?,
            reason: r.get(3)?,
            prompt: r.get(4)?,
            payload_json: r.get(5)?,
            requested_at: r.get(6)?,
            decided_at: r.get(7)?,
            decision: r.get(8)?,
            note: r.get(9)?,
        }),
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))?
    .ok_or_else(|| AppError::NotFound(format!("人工门 {}", id)))
}

pub fn list_gates(conn: &Connection, pending_only: bool) -> AppResult<Vec<GateRow>> {
    let sql = if pending_only {
        "SELECT id,run_id,step_id,reason,prompt,payload_json,requested_at,decided_at,decision,note \
         FROM human_gates WHERE decided_at IS NULL ORDER BY requested_at"
    } else {
        "SELECT id,run_id,step_id,reason,prompt,payload_json,requested_at,decided_at,decision,note \
         FROM human_gates ORDER BY requested_at DESC"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(GateRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                step_id: r.get(2)?,
                reason: r.get(3)?,
                prompt: r.get(4)?,
                payload_json: r.get(5)?,
                requested_at: r.get(6)?,
                decided_at: r.get(7)?,
                decision: r.get(8)?,
                note: r.get(9)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn decide_gate(
    conn: &Connection,
    id: &str,
    decision: &str,
    note: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE human_gates SET decided_at=?2, decision=?3, note=COALESCE(?4,note) \
         WHERE id=?1 AND decided_at IS NULL",
        params![id, now_secs(), decision, note],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn insert_acceptance(
    conn: &Connection,
    run_id: &str,
    criterion: &str,
    passed: bool,
    checked_by: &str,
    note: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO acceptance(run_id,criterion,passed,checked_by,note,checked_at) \
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![run_id, criterion, passed as i64, checked_by, note, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn list_acceptance(conn: &Connection, run_id: &str) -> AppResult<Vec<AcceptanceRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id,run_id,criterion,passed,checked_by,note,checked_at \
             FROM acceptance WHERE run_id=?1 ORDER BY id",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![run_id], |r| {
            Ok(AcceptanceRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                criterion: r.get(2)?,
                passed: r.get::<_, i64>(3)? != 0,
                checked_by: r.get(4)?,
                note: r.get(5)?,
                checked_at: r.get(6)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn run_detail(conn: &Connection, run_id: &str) -> AppResult<RunDetail> {
    Ok(RunDetail {
        run: get_run(conn, run_id)?,
        steps: list_steps(conn, run_id)?,
        gates: list_gates_for_run(conn, run_id)?,
        acceptance: list_acceptance(conn, run_id)?,
    })
}

fn list_gates_for_run(conn: &Connection, run_id: &str) -> AppResult<Vec<GateRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id,run_id,step_id,reason,prompt,payload_json,requested_at,decided_at,decision,note \
             FROM human_gates WHERE run_id=?1 ORDER BY requested_at",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![run_id], |r| {
            Ok(GateRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                step_id: r.get(2)?,
                reason: r.get(3)?,
                prompt: r.get(4)?,
                payload_json: r.get(5)?,
                requested_at: r.get(6)?,
                decided_at: r.get(7)?,
                decision: r.get(8)?,
                note: r.get(9)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

// ---- Schedule / Dashboard ----

pub fn upsert_schedule(conn: &Connection, sop_id: &str, interval_secs: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO schedules(sop_id,next_run_at,last_run_at,status,updated_at,interval_secs) \
         VALUES(?1,?2,NULL,'idle',?2,?3) \
         ON CONFLICT(sop_id) DO UPDATE SET next_run_at=?2, interval_secs=?3, updated_at=?2, status='idle'",
        params![sop_id, now_secs() + interval_secs, interval_secs],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn remove_schedule(conn: &Connection, sop_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM schedules WHERE sop_id=?1", params![sop_id])
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn list_due_schedules(conn: &Connection, now: i64) -> AppResult<Vec<(String, i64)>> {
    // 返回 (sop_id, interval_secs)；next_run_at <= now 且未在跑。
    let mut stmt = conn
        .prepare(
            "SELECT sop_id, interval_secs FROM schedules \
             WHERE next_run_at IS NOT NULL AND next_run_at<=?1 AND status!='running' \
             ORDER BY next_run_at",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map(params![now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn mark_schedule(
    conn: &Connection,
    sop_id: &str,
    next_run_at: i64,
    last_run_at: i64,
    status: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE schedules SET next_run_at=?2,last_run_at=?3,status=?4,updated_at=?5 WHERE sop_id=?1",
        params![sop_id, next_run_at, last_run_at, status, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn dashboard(conn: &Connection) -> AppResult<DashboardData> {
    let schedules = {
        let mut stmt = conn
            .prepare(
                "SELECT sc.sop_id, s.name, sc.next_run_at, sc.last_run_at, sc.status \
                 FROM schedules sc JOIN sops s ON s.id=sc.sop_id ORDER BY sc.next_run_at",
            )
            .map_err(|e| AppError::Db(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ScheduleInfo {
                    sop_id: r.get(0)?,
                    sop_name: r.get(1)?,
                    next_run_at: r.get(2)?,
                    last_run_at: r.get(3)?,
                    status: r.get(4)?,
                })
            })
            .map_err(|e| AppError::Db(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Db(e.to_string()))?
    };
    let tool_count = {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tools", [], |r| r.get(0))
            .map_err(|e| AppError::Db(e.to_string()))?;
        n as usize
    };
    Ok(DashboardData {
        sops: list_sops(conn, false)?,
        runs: list_runs(conn, None, None, 30)?,
        pending_gates: list_gates(conn, true)?,
        schedules,
        tool_count,
    })
}

/// 某个 step 上人工门的状态，供 executor 判定是否可推进。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    None,
    Pending,
    Approved,
    Rejected,
}

pub fn gate_state_for_step(conn: &Connection, step_id: &str) -> AppResult<GateState> {
    let decisions: Vec<Option<String>> = {
        let mut stmt = conn
            .prepare("SELECT decision FROM human_gates WHERE step_id=?1 ORDER BY requested_at DESC")
            .map_err(|e| AppError::Db(e.to_string()))?;
        let rows = stmt
            .query_map(params![step_id], |r| r.get::<_, Option<String>>(0))
            .map_err(|e| AppError::Db(e.to_string()))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    if decisions.is_empty() {
        return Ok(GateState::None);
    }
    match &decisions[0] {
        None => Ok(GateState::Pending),
        Some(d) if matches!(d.as_str(), "approve" | "input" | "takeover") => Ok(GateState::Approved),
        Some(_) => Ok(GateState::Rejected),
    }
}
