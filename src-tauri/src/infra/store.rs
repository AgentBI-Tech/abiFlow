use std::collections::{HashMap, HashSet};

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
            "SELECT id,run_id,seq,node_type,owner,tool_ref,node_id,status,input_json,output_json,started_at,finished_at \
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
                node_id: r.get(6)?,
                status: r.get(7)?,
                input_json: r.get(8)?,
                output_json: r.get(9)?,
                started_at: r.get(10)?,
                finished_at: r.get(11)?,
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
                "SELECT sc.sop_id, s.name, sc.next_run_at, sc.last_run_at, sc.status, sc.interval_secs \
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
                    interval_secs: r.get(5)?,
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

// ---- v0.3 工具授权（gateway 用）----

use crate::domain::gateway::{Decision, Mode, ToolGrant, ToolSpec};

/// 从 tools 表构造 ToolSpec；plane/requires_approval/irreversible 从 scope_json 解析，
/// 缺省 plane=public、其余 false。无需改 schema。
pub fn tool_spec(conn: &Connection, tool_id: &str) -> AppResult<ToolSpec> {
    let (risk, scope): (String, String) = conn
        .query_row(
            "SELECT risk, scope_json FROM tools WHERE id=?1",
            params![tool_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::Db(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("工具 {}", tool_id)))?;
    let scope_v: serde_json::Value =
        serde_json::from_str(&scope).unwrap_or_else(|_| serde_json::json!({}));
    let get_str = |k: &str, d: &str| {
        scope_v.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string()
    };
    let get_bool = |k: &str| scope_v.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    Ok(ToolSpec {
        id: tool_id.to_string(),
        risk,
        plane: get_str("plane", "public"),
        requires_approval: get_bool("requires_approval"),
        irreversible: get_bool("irreversible"),
    })
}

pub fn tool_grant(conn: &Connection, tool_id: &str, mode: Mode) -> AppResult<Option<ToolGrant>> {
    conn.query_row(
        "SELECT tool_id,mode,allowed,planes_json FROM tool_grants WHERE tool_id=?1 AND mode=?2",
        params![tool_id, mode.as_str()],
        |r| {
            let planes: String = r.get(3)?;
            let planes: Vec<String> =
                serde_json::from_str(&planes).unwrap_or_default();
            Ok(ToolGrant {
                tool_id: r.get(0)?,
                mode: Mode::parse(&r.get::<_, String>(1)?).unwrap_or(Mode::Build),
                allowed: r.get::<_, i64>(2)? != 0,
                planes,
            })
        },
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))
}

pub fn list_tool_specs(conn: &Connection) -> AppResult<Vec<ToolSpec>> {
    let mut stmt = conn
        .prepare("SELECT id FROM tools ORDER BY id")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();
    ids.iter().map(|id| tool_spec(conn, id)).collect()
}

pub fn upsert_tool_grant(
    conn: &Connection,
    tool_id: &str,
    mode: Mode,
    allowed: bool,
    planes: &[String],
) -> AppResult<()> {
    let planes_json = serde_json::to_string(planes)
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    conn.execute(
        "INSERT INTO tool_grants(tool_id,mode,allowed,planes_json) VALUES(?1,?2,?3,?4) \
         ON CONFLICT(tool_id,mode) DO UPDATE SET allowed=?3, planes_json=?4",
        params![tool_id, mode.as_str(), allowed as i64, planes_json],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

/// 构建授权上下文：actor 由外部给定，mode 与 grant 从库解析。
pub fn call_ctx(
    conn: &Connection,
    run_id: &str,
    actor: crate::domain::gateway::Actor,
    mode: Mode,
    tool_id: &str,
) -> AppResult<crate::domain::gateway::CallCtx> {
    Ok(crate::domain::gateway::CallCtx {
        run_id: run_id.to_string(),
        actor,
        mode,
        grant: tool_grant(conn, tool_id, mode)?,
    })
}

/// 判定某工具对某角色/模式是否可见（供 tool_list 过滤）。
pub fn tool_visible(
    conn: &Connection,
    tool_id: &str,
    role: &str,
    mode: Mode,
) -> AppResult<bool> {
    let spec = tool_spec(conn, tool_id)?;
    let ctx = call_ctx(conn, "", crate::domain::gateway::Actor::human("", role), mode, tool_id)?;
    Ok(matches!(
        crate::domain::gateway::authorize(&spec, &ctx),
        Decision::Allow | Decision::NeedGate
    ))
}

/// 工具完整元数据（供 tool_list 展示真实 title/server）。
pub fn tool_meta(conn: &Connection, tool_id: &str) -> AppResult<(String, String)> {
    conn.query_row(
        "SELECT title,server FROM tools WHERE id=?1",
        params![tool_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| AppError::Db(e.to_string()))?
    .ok_or_else(|| AppError::NotFound(format!("工具 {}", tool_id)))
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    use crate::domain::gateway::Mode;
    use crate::infra::db;

    #[test]
    fn grant_change_affects_visibility() {
        let c = db::open_in_memory().unwrap();
        crate::domain::executor::demo_install(&c).unwrap();
        // 初始：agent/build 授权 public+content → 可见。
        assert!(tool_visible(&c, "demo:check_anomalies", "agent", Mode::Build).unwrap());
        // 关掉 build 授权 → 不可见。
        upsert_tool_grant(&c, "demo:check_anomalies", Mode::Build, false, &[]).unwrap();
        assert!(!tool_visible(&c, "demo:check_anomalies", "agent", Mode::Build).unwrap());
        // plan 模式授权还在 → plan 可见（三模式独立）。
        assert!(tool_visible(&c, "demo:check_anomalies", "agent", Mode::Plan).unwrap());
    }

    #[test]
    fn content_role_cannot_see_cost_plane_tool() {
        let c = db::open_in_memory().unwrap();
        // 造一个 cost 面工具 + 全模式全平面授权。
        c.execute(
            "INSERT INTO tools(id,server,command,title,risk,scope_json,enabled) \
             VALUES('ads:spend','builtin','x','广告花费','low','{\"plane\":\"cost\"}',1)",
            [],
        ).unwrap();
        for m in [Mode::Plan, Mode::Build, Mode::Review] {
            upsert_tool_grant(&c, "ads:spend", m, true, &["cost".into()]).unwrap();
        }
        // content 角色被角色面策略拦下（看不到进价/花费）。
        assert!(!tool_visible(&c, "ads:spend", "content", Mode::Build).unwrap());
        // ops 角色可见。
        assert!(tool_visible(&c, "ads:spend", "ops", Mode::Build).unwrap());
    }
}

// ---- v0.3 credentials 元数据（只存引用，不存值）----

use crate::models::CredentialRef;

pub fn list_credentials(conn: &Connection) -> AppResult<Vec<CredentialRef>> {
    let mut stmt = conn
        .prepare("SELECT ref_id,storage_key,provider,label FROM credentials ORDER BY ref_id")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(CredentialRef {
                ref_id: r.get(0)?,
                storage_key: r.get(1)?,
                provider: r.get(2)?,
                label: r.get(3)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn upsert_credential_ref(
    conn: &Connection,
    ref_id: &str,
    provider: &str,
    label: Option<&str>,
) -> AppResult<()> {
    // storage_key 用 ref_id 自身（OS 密钥链里的条目键）；值不进库。
    conn.execute(
        "INSERT INTO credentials(ref_id,storage_key,provider,label,created_at,updated_at) \
         VALUES(?1,?1,?2,?3,?4,?4) \
         ON CONFLICT(ref_id) DO UPDATE SET provider=?2, label=?3, updated_at=?4",
        params![ref_id, provider, label, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn remove_credential_ref(conn: &Connection, ref_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM credentials WHERE ref_id=?1", params![ref_id])
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use crate::infra::db;

    #[test]
    fn credential_meta_never_stores_value() {
        let c = db::open_in_memory().unwrap();
        upsert_credential_ref(&c, "ga4-key", "ga4", Some("GA4 演示")).unwrap();
        // 库里只有引用列，没有 value 列。
        let mut stmt = c.prepare("PRAGMA table_info(credentials)").unwrap();
        let cols: Vec<String> = stmt.query_map([], |r| r.get(1)).unwrap()
            .filter_map(|r| r.ok()).collect();
        assert!(!cols.iter().any(|x| x == "value" || x == "secret"));
        let list = list_credentials(&c).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ref_id, "ga4-key");
        assert_eq!(list[0].storage_key, "ga4-key"); // storage_key = ref_id，不含密钥值
    }
}

// ---- v0.4 经验沉淀：templates / corrections ----

use crate::models::{CorrectionInput, CorrectionRow, TemplateBundle, TemplateRow};

pub fn list_templates(conn: &Connection) -> AppResult<Vec<TemplateRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id,name,sop_snapshot,trajectory_json,corrections_json,source_shop,validated,created_at \
             FROM templates ORDER BY created_at DESC",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(TemplateRow {
                id: r.get(0)?,
                name: r.get(1)?,
                sop_snapshot: r.get(2)?,
                trajectory_json: r.get(3)?,
                corrections_json: r.get(4)?,
                source_shop: r.get(5)?,
                validated: r.get::<_, i64>(6)? != 0,
                created_at: r.get(7)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

pub fn save_template(conn: &Connection, bundle: &TemplateBundle) -> AppResult<String> {
    let id = new_id("tpl");
    conn.execute(
        "INSERT INTO templates(id,name,sop_snapshot,trajectory_json,corrections_json,source_shop,validated,created_at) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            id, bundle.name, bundle.sop_snapshot, bundle.trajectory_json, bundle.corrections_json,
            bundle.source_shop, bundle.validated as i64, now_secs()
        ],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(id)
}

pub fn list_corrections(conn: &Connection, step_id: Option<&str>) -> AppResult<Vec<CorrectionRow>> {
    let (sql, param): (&str, Option<&str>) = if let Some(s) = step_id {
        (
            "SELECT id,step_id,run_id,node_type,original_json,corrected_json,note,author,created_at \
             FROM corrections WHERE step_id=?1 ORDER BY created_at",
            Some(s),
        )
    } else {
        (
            "SELECT id,step_id,run_id,node_type,original_json,corrected_json,note,author,created_at \
             FROM corrections ORDER BY created_at DESC",
            None,
        )
    };
    let mut stmt = conn.prepare(sql).map_err(|e| AppError::Db(e.to_string()))?;
    let rows = if let Some(s) = param {
        stmt.query_map(params![s], map_correction)
    } else {
        stmt.query_map([], map_correction)
    }
    .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

fn map_correction(r: &rusqlite::Row) -> Result<CorrectionRow, rusqlite::Error> {
    Ok(CorrectionRow {
        id: r.get(0)?,
        step_id: r.get(1)?,
        run_id: r.get(2)?,
        node_type: r.get(3)?,
        original_json: r.get(4)?,
        corrected_json: r.get(5)?,
        note: r.get(6)?,
        author: r.get(7)?,
        created_at: r.get(8)?,
    })
}

pub fn add_correction(conn: &Connection, step_id: &str, input: &CorrectionInput) -> AppResult<i64> {
    // 从 step 反查 run_id / node_type，保证审计字段真实。
    let (run_id, node_type): (String, String) = conn
        .query_row(
            "SELECT run_id,node_type FROM steps WHERE id=?1",
            params![step_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| AppError::Db(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("步骤 {}", step_id)))?;
    conn.execute(
        "INSERT INTO corrections(step_id,run_id,node_type,original_json,corrected_json,note,author,created_at) \
         VALUES(?1,?2,?3,?4,?5,?6,'human',?7)",
        params![
            step_id, run_id, node_type, input.original_json, input.corrected_json,
            input.note, now_secs()
        ],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

pub fn count_completed_runs(conn: &Connection) -> AppResult<usize> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM runs WHERE status='done'", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(n as usize)
}

pub fn count_acceptance_checks(conn: &Connection) -> AppResult<usize> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM acceptance", [], |r| r.get(0))
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(n as usize)
}

pub fn all_steps(conn: &Connection) -> AppResult<Vec<StepRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id,run_id,seq,node_type,owner,tool_ref,node_id,status,input_json,output_json,started_at,finished_at \
             FROM steps ORDER BY run_id,seq",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(StepRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                seq: r.get(2)?,
                node_type: r.get(3)?,
                owner: r.get(4)?,
                tool_ref: r.get(5)?,
                node_id: r.get(6)?,
                status: r.get(7)?,
                input_json: r.get(8)?,
                output_json: r.get(9)?,
                started_at: r.get(10)?,
                finished_at: r.get(11)?,
            })
        })
        .map_err(|e| AppError::Db(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::Db(e.to_string()))
}

/// 返回工具的完整 scope_json（配置驱动执行的依据）。
pub fn tool_config(conn: &Connection, tool_id: &str) -> AppResult<serde_json::Value> {
    let scope: String = conn
        .query_row("SELECT scope_json FROM tools WHERE id=?1", params![tool_id], |r| r.get(0))
        .optional()
        .map_err(|e| AppError::Db(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("工具 {}", tool_id)))?;
    serde_json::from_str(&scope).map_err(|e| AppError::InvalidInput(format!("scope_json 非法：{}", e)))
}

/// 新建或更新一个工具定义（工具即数据）。
pub fn upsert_tool(
    conn: &Connection,
    id: &str,
    server: &str,
    command: &str,
    title: &str,
    risk: &str,
    scope_json: &str,
    enabled: bool,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO tools(id,server,command,title,risk,scope_json,enabled) \
         VALUES(?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(id) DO UPDATE SET server=?2,command=?3,title=?4,risk=?5,scope_json=?6,enabled=?7",
        params![id, server, command, title, risk, scope_json, enabled as i64],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

pub fn remove_tool(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM tools WHERE id=?1", params![id])
        .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(())
}

// ---- v0.5 实际放权 ----

/// run_id → sop_id 映射，供 upgrade_candidates 给候选标注所属 SOP。
pub fn run_sop_map(conn: &Connection) -> AppResult<HashMap<String, String>> {
    let mut stmt = conn
        .prepare("SELECT id, sop_id FROM runs")
        .map_err(|e| AppError::Db(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| AppError::Db(e.to_string()))?;
    let mut map = HashMap::new();
    for row in rows {
        let (id, sop) = row.map_err(|e| AppError::Db(e.to_string()))?;
        map.insert(id, sop);
    }
    Ok(map)
}

/// 落库放权后的 SOP 定义（由 domain::plan_owner_upgrade 产出），版本递增。返回新版本号。
pub fn set_node_owner(
    conn: &Connection,
    sop_id: &str,
    definition_json: &str,
) -> AppResult<i32> {
    let current: i32 = conn
        .query_row("SELECT version FROM sops WHERE id=?1", params![sop_id], |r| r.get(0))
        .optional()
        .map_err(|e| AppError::Db(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("SOP {}", sop_id)))?;
    let new_version = current + 1;
    conn.execute(
        "UPDATE sops SET version=?2, definition=?3, updated_at=?4 WHERE id=?1",
        params![sop_id, new_version, definition_json, now_secs()],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(new_version)
}

/// 记录一次 owner 决策审计（approve/reject 都留痕）。
#[allow(clippy::too_many_arguments)]
pub fn insert_owner_decision(
    conn: &Connection,
    sop_id: &str,
    node_id: &str,
    node_type: &str,
    from_owner: &str,
    to_owner: &str,
    decision: &str,
    correction_count: i64,
    note: Option<&str>,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO owner_decisions(sop_id,node_id,node_type,from_owner,to_owner,decision,correction_count,note,decided_by,created_at) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'human',?9)",
        params![
            sop_id, node_id, node_type, from_owner, to_owner, decision,
            correction_count, note, now_secs()
        ],
    )
    .map_err(|e| AppError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

/// 列出 owner 决策审计；sop_id 为 Some 时按 SOP 过滤。
pub fn list_owner_decisions(
    conn: &Connection,
    sop_id: Option<&str>,
) -> AppResult<Vec<crate::models::OwnerDecisionRow>> {
    if let Some(sid) = sop_id {
        let mut stmt = conn.prepare(
            "SELECT id,sop_id,node_id,node_type,from_owner,to_owner,decision,correction_count,note,decided_by,created_at \
             FROM owner_decisions WHERE sop_id=?1 ORDER BY created_at DESC",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
        let rows = stmt
            .query_map(params![sid], map_owner_decision)
            .map_err(|e| AppError::Db(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Db(e.to_string()))
    } else {
        let mut stmt = conn.prepare(
            "SELECT id,sop_id,node_id,node_type,from_owner,to_owner,decision,correction_count,note,decided_by,created_at \
             FROM owner_decisions ORDER BY created_at DESC",
        )
        .map_err(|e| AppError::Db(e.to_string()))?;
        let rows = stmt
            .query_map([], map_owner_decision)
            .map_err(|e| AppError::Db(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::Db(e.to_string()))
    }
}

fn map_owner_decision(r: &rusqlite::Row) -> Result<crate::models::OwnerDecisionRow, rusqlite::Error> {
    Ok(crate::models::OwnerDecisionRow {
        id: r.get(0)?,
        sop_id: r.get(1)?,
        node_id: r.get(2)?,
        node_type: r.get(3)?,
        from_owner: r.get(4)?,
        to_owner: r.get(5)?,
        decision: r.get(6)?,
        correction_count: r.get(7)?,
        note: r.get(8)?,
        decided_by: r.get(9)?,
        created_at: r.get(10)?,
    })
}
