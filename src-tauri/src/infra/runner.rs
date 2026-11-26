//! 后台推进引擎（2026-11-26）：把 advance 从「命令同步跑完」改为「先落库返回、后台线程推进」。
//!
//! 设计口径：
//! - 每个 run 一个独立 std::thread，线程内**自己开独立 DB 连接**（rusqlite::Connection 不跨线程），
//!   用克隆的 keystore / LLM Arc 重建 ToolEngine；两个 Arc 都是 Send+Sync，trait 不变。
//! - 每步状态变化仍走 store::update_step → events::emit，前端按 runId 刷详情、刷看板，色带随事件逐条出现。
//! - 应用退出时线程随进程结束（允许并发、退出即停，不优雅等待）；DB 里停在 running 的 run 不自动续跑。
//! - 线程内推进失败只记日志不抛（前端已能感知终态），不把后台异常回传 UI。

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use crate::domain::executor;
use crate::domain::keystore::KeyStore;
use crate::domain::llm::LlmProvider;
use crate::infra::db;
use crate::infra::tool_engine::ToolEngine;

fn spawn(
    db_path: PathBuf,
    ks: Arc<dyn KeyStore>,
    llm: Arc<dyn LlmProvider>,
    work: impl FnOnce(&rusqlite::Connection, ToolEngine, Arc<dyn LlmProvider>) + Send + 'static,
) {
    thread::spawn(move || {
        match db::open(&db_path) {
            Ok(conn) => {
                let engine = ToolEngine::new(ks);
                work(&conn, engine, llm);
            }
            Err(e) => eprintln!("[runner] 打开数据库失败，后台推进跳过：{}", e),
        }
    });
}

/// 新建 run 后调用：后台从起点推进直到完成 / 失败 / 等人工门。
pub fn spawn_run(
    db_path: PathBuf,
    run_id: String,
    ks: Arc<dyn KeyStore>,
    llm: Arc<dyn LlmProvider>,
) {
    spawn(db_path, ks, llm, move |conn, engine, llm| {
        if let Err(e) = executor::advance(conn, &run_id, &engine, llm.as_ref()) {
            eprintln!("[runner] run {} 后台推进失败：{}", run_id, e);
        }
    });
}

/// 人工门裁决放行后调用：先恢复状态再后台推进（resume_after_gate 内含状态检查）。
pub fn spawn_resume(
    db_path: PathBuf,
    run_id: String,
    ks: Arc<dyn KeyStore>,
    llm: Arc<dyn LlmProvider>,
) {
    spawn(db_path, ks, llm, move |conn, engine, llm| {
        if let Err(e) = executor::resume_after_gate(conn, &run_id, &engine, llm.as_ref()) {
            eprintln!("[runner] run {} 门后恢复失败：{}", run_id, e);
        }
    });
}

/// 打回第 1 轮重试：后台清空验收留痕并从头重跑。
pub fn spawn_retry(
    db_path: PathBuf,
    run_id: String,
    ks: Arc<dyn KeyStore>,
    llm: Arc<dyn LlmProvider>,
) {
    spawn(db_path, ks, llm, move |conn, engine, llm| {
        if let Err(e) = executor::retry_rejected(conn, &run_id, &engine, llm.as_ref()) {
            eprintln!("[runner] run {} 重试推进失败：{}", run_id, e);
        }
    });
}

/// 打回两轮后的仲裁放行：后台重置计数并带修正说明重跑。
pub fn spawn_arbitrate(
    db_path: PathBuf,
    run_id: String,
    decision: String,
    note: Option<String>,
    ks: Arc<dyn KeyStore>,
    llm: Arc<dyn LlmProvider>,
) {
    spawn(db_path, ks, llm, move |conn, engine, llm| {
        if let Err(e) = executor::arbitrate(conn, &run_id, &decision, note.as_deref(), &engine, llm.as_ref()) {
            eprintln!("[runner] run {} 仲裁推进失败：{}", run_id, e);
        }
    });
}
