use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 前端展示用的应用信息（app_info 命令返回）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub db_path: String,
    pub table_count: usize,
}

// ---- 补齐 IPC（§3.1）：sop_get / config / mcp_server ----

/// SOP 版本历史项：从 run 的定义快照取（sops 表原地升版本、无历史行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SopVersion {
    pub version: i32,
    pub definition: serde_json::Value,
    pub ran_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SopDetail {
    pub sop: SopRow,
    pub versions: Vec<SopVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetPayload {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerPayload {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub note: String,
}

// ---- v0.2 执行闭环的数据传输对象（对齐实现设计 §2 / §3.1） ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SopRow {
    pub id: String,
    pub name: String,
    pub version: i32,
    pub trigger_json: String,
    pub risk: String,
    pub owner_policy: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub id: String,
    pub sop_id: String,
    pub sop_version: i32,
    pub status: String,
    pub mode: String,
    pub trigger_by: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub verdict: Option<String>,
    pub note: Option<String>,
    pub definition_snapshot: Option<String>,
    pub current_node: Option<String>,
    pub outputs_json: String,
    pub branch_skipped: String,
    /// 验收打回次数：0 未打回；1 已打回一次（可重试）；>=2 转 review 人工仲裁。
    pub reject_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRow {
    pub id: String,
    pub run_id: String,
    pub seq: i32,
    pub node_type: String,
    pub owner: String,
    pub tool_ref: Option<String>,
    pub node_id: Option<String>,
    pub status: String,
    pub input_json: Option<String>,
    pub output_json: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateRow {
    pub id: String,
    pub run_id: String,
    pub step_id: String,
    pub reason: String,
    pub prompt: String,
    pub payload_json: Option<String>,
    pub requested_at: i64,
    pub decided_at: Option<i64>,
    pub decision: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceRow {
    pub id: i64,
    pub run_id: String,
    pub criterion: String,
    pub passed: bool,
    pub checked_by: String,
    pub note: Option<String>,
    pub checked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub run: RunRow,
    pub steps: Vec<StepRow>,
    pub gates: Vec<GateRow>,
    pub acceptance: Vec<AcceptanceRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleInfo {
    pub sop_id: String,
    pub sop_name: String,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub status: String,
    pub interval_secs: i64,
    /// 标准 5 段 cron 表达式；None = 旧的 interval 间隔模式。
    pub cron_expr: Option<String>,
}

// ---- v0.2 基础看板：日/周 + 按 SOP / 分类聚合（纯本地事实，不跨店比绝对值）----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardPeriodStats {
    pub label: String,
    pub total: usize,
    pub done: usize,
    pub running: usize,
    pub wait_human: usize,
    pub rejected: usize,
    pub failed: usize,
    pub cancelled: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupStat {
    pub key: String,
    pub label: String,
    pub total: usize,
    pub done: usize,
    pub wait_human: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardSummary {
    pub day: BoardPeriodStats,
    pub week: BoardPeriodStats,
    /// 按 SOP 聚合（key=sop_id，label=SOP 名）。
    pub by_sop: Vec<GroupStat>,
    /// 按分类聚合（现有 risk 维度：low/medium/high，不新增分类字段）。
    pub by_risk: Vec<GroupStat>,
    pub scope_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardData {
    pub sops: Vec<SopRow>,
    pub runs: Vec<RunRow>,
    pub pending_gates: Vec<GateRow>,
    pub schedules: Vec<ScheduleInfo>,
    pub tool_count: usize,
    /// 日/周与分类聚合（v0.2 基础看板）。
    pub board: BoardSummary,
}

// ---- 命令入参 ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SopUpsertPayload {
    pub id: Option<String>,
    pub name: String,
    pub trigger_json: String,
    pub risk: String,
    pub owner_policy: String,
    pub definition: Value,
    pub expect_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartPayload {
    pub sop_id: String,
    pub mode: Option<String>,
    pub input: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunControlPayload {
    pub run_id: String,
    pub action: String,
    /// change_path 的目标节点 id；其它动作忽略。
    pub value: Option<String>,
    /// 人工干预留痕说明。
    pub note: Option<String>,
}

/// 人工仲裁（验收打回两轮后 review 状态）：resume 放行重跑 / fail 判失败。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunArbitratePayload {
    pub run_id: String,
    pub decision: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDecidePayload {
    pub gate_id: String,
    pub decision: String,
    pub note: Option<String>,
    pub payload: Option<Value>,
}

/// 到期调度项：cron 模式 cron_expr 为 Some（按表达式算下次），否则用 interval_secs。
#[derive(Debug, Clone)]
pub struct DueSchedule {
    pub sop_id: String,
    pub interval_secs: i64,
    pub cron_expr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleSetPayload {
    pub sop_id: String,
    pub enabled: bool,
    pub interval_secs: Option<i64>,
    /// 设了 cron 表达式则走 cron 模式（与 interval_secs 二选一，cron 优先）。
    pub cron_expr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolGrantSetPayload {
    pub tool_id: String,
    pub mode: String,
    pub allowed: bool,
    pub planes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRow {
    pub id: String,
    pub title: String,
    pub server: String,
    pub risk: String,
    pub plane: String,
    pub requires_approval: bool,
    pub irreversible: bool,
    /// 当前角色/模式下的可见性（tool_list 按 role+mode 过滤后填充）。
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolGrantRow {
    pub tool_id: String,
    pub mode: String,
    pub allowed: bool,
    pub planes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRef {
    pub ref_id: String,
    pub storage_key: String,
    pub provider: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSetPayload {
    pub ref_id: String,
    pub provider: String,
    pub label: Option<String>,
    /// 密钥值：只在内存传给 keystore，绝不落库、不进日志、不回显。
    pub value: String,
}

// ---- v0.4 经验沉淀 ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateRow {
    pub id: String,
    pub name: String,
    pub sop_snapshot: String,
    pub trajectory_json: String,
    pub corrections_json: String,
    pub source_shop: Option<String>,
    pub validated: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionRow {
    pub id: i64,
    pub step_id: String,
    pub run_id: String,
    pub node_type: String,
    pub original_json: String,
    pub corrected_json: String,
    pub note: Option<String>,
    pub author: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionInput {
    pub step_id: String,
    pub original_json: String,
    pub corrected_json: String,
    pub note: Option<String>,
}

/// 模板导出包（§6 zip 内的 manifest/sop/trajectory/corrections/README 的内存表示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateBundle {
    pub manifest_schema: i32,
    pub name: String,
    pub validated: bool,
    pub sop_snapshot: String,
    pub trajectory_json: String,
    pub corrections_json: String,
    pub source_shop: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeCandidate {
    /// 候选所在的 SOP（放权时必须定位到具体 SOP + 节点）。
    pub sop_id: String,
    pub node_type: String,
    pub node_id: String,
    pub correction_count: usize,
    pub threshold: usize,
    /// 是否允许升级：资金/不可逆工具恒为 false。
    pub eligible: bool,
    pub reason: String,
}

/// owner 决策审计记录（人确认/驳回一次升级）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerDecisionRow {
    pub id: i64,
    pub sop_id: String,
    pub node_id: String,
    pub node_type: String,
    pub from_owner: String,
    pub to_owner: String,
    pub decision: String,
    pub correction_count: i64,
    pub note: Option<String>,
    pub decided_by: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EfficiencyReport {
    pub total_runs: usize,
    pub completed_runs: usize,
    pub total_steps: usize,
    pub agent_done_steps: usize,
    pub automation_rate: f64,
    pub human_interventions: usize,
    pub avg_acceptance_rounds: f64,
    /// 只按步数/次数陈述，不编金额。
    pub saved_steps_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateCreatePayload {
    pub run_id: String,
    pub name: String,
    pub source_shop: Option<String>,
}

/// 模板换参复用（v0.4）：从 validated 模板 + 参数生成一条新 SOP。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateReusePayload {
    pub template_id: String,
    pub name: String,
    /// 参数名 → 值（字符串/数字/布尔）。必须覆盖模板里的全部 {{param:*}}。
    pub params: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeApprovePayload {
    /// 定位到具体 SOP + 节点，避免同名 node_id 跨 SOP 误操作。
    pub sop_id: String,
    pub node_id: String,
    pub approve: bool,
}

/// 自定义工具定义（工具即数据，用户可改）。kind=builtin|http。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefPayload {
    pub id: String,
    pub title: String,
    pub risk: String,
    pub server: String,
    pub command: String,
    /// 完整工具配置 JSON：{kind, method, url, headers?, query?, body?, timeout_s?, plane?, read_only?}
    pub scope: Value,
    pub enabled: bool,
}

// ---- 命令出参 ----

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SopUpsertResult {
    pub id: String,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStartResult {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerTickResult {
    pub started: Vec<String>,
    pub errors: Vec<String>,
}

/// 统一错误：前端按 code 决定提示与跳转（对齐实现设计 §3.1）。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("版本冲突：SOP 已被他人改动")]
    VersionConflict,
    #[error("需要人工确认后才能继续")]
    GateRequired,
    #[error("工具未授权：{0}")]
    ToolDenied(String),
    #[error("高风险动作被拦截：{0}")]
    RiskBlocked(String),
    #[error("MCP 服务不可用：{0}")]
    McpUnavailable(String),
    #[error("LLM 调用失败：{0}")]
    LlmError(String),
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("输入非法：{0}")]
    InvalidInput(String),
    #[error("数据库错误：{0}")]
    Db(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let (code, msg) = match self {
            AppError::VersionConflict => ("VersionConflict", self.to_string()),
            AppError::GateRequired => ("GateRequired", self.to_string()),
            AppError::ToolDenied(m) => ("ToolDenied", format!("工具未授权：{}", m)),
            AppError::RiskBlocked(m) => ("RiskBlocked", format!("高风险动作被拦截：{}", m)),
            AppError::McpUnavailable(m) => ("McpUnavailable", format!("MCP 服务不可用：{}", m)),
            AppError::LlmError(m) => ("LlmError", format!("LLM 调用失败：{}", m)),
            AppError::NotFound(m) => ("NotFound", format!("未找到：{}", m)),
            AppError::InvalidInput(m) => ("InvalidInput", format!("输入非法：{}", m)),
            AppError::Db(m) => ("Db", format!("数据库错误：{}", m)),
        };
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("code", code)?;
        map.serialize_entry("message", &msg)?;
        map.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// 生成带前缀的本地 id（时间戳纳秒 + 原子计数，足以单机去重）。
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{:x}-{:x}", prefix, ts, n)
}

pub fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
