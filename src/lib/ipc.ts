import { invoke } from "@tauri-apps/api/core";

// 前端唯一 IPC 入口（实现设计 §3.1）。所有命令走这里，错误以 {code,message} 抛出。

export type SopRow = {
  id: string;
  name: string;
  version: number;
  triggerJson: string;
  risk: string;
  ownerPolicy: string;
  enabled: boolean;
  createdAt: number;
  updatedAt: number;
  definition: string;
};

export type RunRow = {
  id: string;
  sopId: string;
  sopVersion: number;
  status: string;
  mode: string;
  triggerBy: string;
  startedAt: number | null;
  finishedAt: number | null;
  verdict: string | null;
  note: string | null;
  definitionSnapshot: string | null;
  currentNode: string | null;
  outputsJson: string;
  branchSkipped: string;
  rejectCount: number;
};

export type StepRow = {
  id: string;
  runId: string;
  seq: number;
  nodeType: string;
  owner: string;
  toolRef: string | null;
  nodeId: string | null;
  status: string;
  inputJson: string | null;
  outputJson: string | null;
  startedAt: number | null;
  finishedAt: number | null;
};

export type GateRow = {
  id: string;
  runId: string;
  stepId: string;
  reason: string;
  prompt: string;
  payloadJson: string | null;
  requestedAt: number;
  decidedAt: number | null;
  decision: string | null;
  note: string | null;
};

export type AcceptanceRow = {
  id: number;
  runId: string;
  criterion: string;
  passed: boolean;
  checkedBy: string;
  note: string | null;
  checkedAt: number | null;
};

export type RunDetail = {
  run: RunRow;
  steps: StepRow[];
  gates: GateRow[];
  acceptance: AcceptanceRow[];
};

export type ScheduleInfo = {
  sopId: string;
  sopName: string;
  nextRunAt: number | null;
  lastRunAt: number | null;
  status: string;
  intervalSecs: number;
  cronExpr: string | null;
};

export type BoardPeriodStats = {
  label: string;
  total: number;
  done: number;
  running: number;
  waitHuman: number;
  rejected: number;
  failed: number;
  cancelled: number;
};

export type GroupStat = {
  key: string;
  label: string;
  total: number;
  done: number;
  waitHuman: number;
  failed: number;
};

export type BoardSummary = {
  day: BoardPeriodStats;
  week: BoardPeriodStats;
  bySop: GroupStat[];
  byRisk: GroupStat[];
  scopeNote: string;
};

export type DashboardData = {
  sops: SopRow[];
  runs: RunRow[];
  pendingGates: GateRow[];
  schedules: ScheduleInfo[];
  toolCount: number;
  board: BoardSummary;
};

export type SchedulerTickResult = {
  started: string[];
  errors: string[];
};

export type ToolRow = {
  id: string;
  title: string;
  server: string;
  risk: string;
  plane: string;
  requiresApproval: boolean;
  irreversible: boolean;
  visible: boolean;
};

export type ToolGrantRow = {
  toolId: string;
  mode: string;
  allowed: boolean;
  planes: string[];
};

export type CredentialRef = {
  refId: string;
  storageKey: string;
  provider: string;
  label: string | null;
};

export type TemplateRow = {
  id: string;
  name: string;
  sopSnapshot: string;
  trajectoryJson: string;
  correctionsJson: string;
  sourceShop: string | null;
  validated: boolean;
  createdAt: number;
};

export type CorrectionRow = {
  id: number;
  stepId: string;
  runId: string;
  nodeType: string;
  originalJson: string;
  correctedJson: string;
  note: string | null;
  author: string;
  createdAt: number;
};

export type UpgradeCandidate = {
  sopId: string;
  nodeType: string;
  nodeId: string;
  correctionCount: number;
  threshold: number;
  eligible: boolean;
  reason: string;
};

export type OwnerDecisionRow = {
  id: number;
  sopId: string;
  nodeId: string;
  nodeType: string;
  fromOwner: string;
  toOwner: string;
  decision: string;
  correctionCount: number;
  note: string | null;
  decidedBy: string;
  createdAt: number;
};

export type EfficiencyReport = {
  totalRuns: number;
  completedRuns: number;
  totalSteps: number;
  agentDoneSteps: number;
  automationRate: number;
  humanInterventions: number;
  avgAcceptanceRounds: number;
  savedStepsNote: string;
};

export type AppError = { code: string; message: string };

export function dashboardGet(): Promise<DashboardData> {
  return invoke("dashboard_get");
}
export function runGet(runId: string): Promise<RunDetail> {
  return invoke("run_get", { runId });
}
export function runStart(sopId: string, input?: unknown): Promise<{ runId: string }> {
  return invoke("run_start", { payload: { sopId, mode: "build", input: input ?? {} } });
}
export function runList(sopId?: string, status?: string, limit?: number): Promise<RunRow[]> {
  return invoke("run_list", { sopId: sopId ?? null, status: status ?? null, limit: limit ?? 30 });
}
/** 人工干预：pause/resume/takeover/cancel/change_path。changePath 时 value=目标节点 id。 */
export function runControl(runId: string, action: string, value?: string, note?: string): Promise<string> {
  return invoke("run_control", { payload: { runId, action, value: value ?? null, note: note ?? null } });
}
/** 第 1 轮打回后重试。 */
export function runRetry(runId: string): Promise<string> {
  return invoke("run_retry", { payload: { runId, action: "retry" } });
}
/** 人工仲裁（review 状态）：decision = resume | fail。 */
export function runArbitrate(runId: string, decision: string, note?: string): Promise<string> {
  return invoke("run_arbitrate", { payload: { runId, decision, note: note ?? null } });
}
export function gateDecide(gateId: string, decision: string, note?: string): Promise<void> {
  return invoke("gate_decide", { payload: { gateId, decision, note: note ?? null } });
}
export function schedulerTick(): Promise<SchedulerTickResult> {
  return invoke("scheduler_tick");
}
/// 注入假事件（事件订阅触发的演示入口）；下一次 scheduler tick 消费。
export function emitEvent(eventType: string, payloadJson?: string): Promise<void> {
  return invoke("emit_event", { eventType, payloadJson: payloadJson ?? null });
}
export function scheduleSet(sopId: string, enabled: boolean, intervalSecs: number, cronExpr?: string | null): Promise<void> {
  return invoke("schedule_set", { payload: { sopId, enabled, intervalSecs, cronExpr: cronExpr ?? null } });
}
export function runAsAgent(sopId: string): Promise<{ runId: string }> {
  return invoke("run_as_agent", { sopId });
}
export function demoInstall(): Promise<string> {
  return invoke("demo_install");
}
export function sopSetEnabled(id: string, enabled: boolean): Promise<void> {
  return invoke("sop_set_enabled", { id, enabled });
}
export function sopUpsert(p: {
  id?: string;
  name: string;
  triggerJson: string;
  risk: string;
  ownerPolicy: string;
  definition: Record<string, unknown>;
  expectVersion?: number;
}): Promise<{ id: string; version: number }> {
  return invoke("sop_upsert", { payload: p });
}
export function toolList(role: string, mode: string): Promise<ToolRow[]> {
  return invoke("tool_list", { role, mode });
}
export function toolGrantList(): Promise<ToolGrantRow[]> {
  return invoke("tool_grant_list");
}
export function toolGrantSet(toolId: string, mode: string, allowed: boolean, planes: string[]): Promise<void> {
  return invoke("tool_grant_set", { payload: { toolId, mode, allowed, planes } });
}
export function credentialList(): Promise<CredentialRef[]> {
  return invoke("credential_list");
}
export function credentialSet(refId: string, provider: string, label: string | null, value: string): Promise<void> {
  return invoke("credential_set", { payload: { refId, provider, label, value } });
}
export function credentialRemove(refId: string): Promise<void> {
  return invoke("credential_remove", { refId });
}

export function templateCreate(runId: string, name: string, sourceShop: string | null): Promise<string> {
  return invoke("template_create", { payload: { runId, name, sourceShop } });
}
export function templateList(): Promise<TemplateRow[]> {
  return invoke("template_list");
}
export function templateExport(id: string): Promise<number[]> {
  return invoke("template_export", { id });
}
export function templateImport(bytes: number[]): Promise<string> {
  return invoke("template_import", { bytes });
}
/** 取模板定义里声明的可换参数 {{param:*}}。 */
export function templateParams(templateId: string): Promise<string[]> {
  return invoke("template_params", { templateId });
}
/** 换参复用：从 validated 模板 + 参数生成一条新 SOP。 */
export function templateReuse(templateId: string, name: string, params: Record<string, string | number | boolean>): Promise<{ id: string; version: number }> {
  return invoke("template_reuse", { payload: { templateId, name, params } });
}
export function correctionList(stepId: string | null): Promise<CorrectionRow[]> {
  return invoke("correction_list", { stepId });
}
export function correctionAdd(stepId: string, originalJson: string, correctedJson: string, note: string | null): Promise<number> {
  return invoke("correction_add", { input: { stepId, originalJson, correctedJson, note } });
}
export function upgradeCandidateList(): Promise<UpgradeCandidate[]> {
  return invoke("upgrade_candidate_list");
}
export function upgradeApprove(sopId: string, nodeId: string, approve: boolean): Promise<void> {
  return invoke("upgrade_approve", { payload: { sopId, nodeId, approve } });
}
export function ownerDecisionList(sopId?: string): Promise<OwnerDecisionRow[]> {
  return invoke("owner_decision_list", { sopId: sopId ?? null });
}
export function reportGet(): Promise<EfficiencyReport> {
  return invoke("report_get");
}

export type ToolDefPayload = {
  id: string;
  title: string;
  risk: string;
  server: string;
  command: string;
  scope: Record<string, unknown>;
  enabled: boolean;
};
export function toolUpsert(p: ToolDefPayload): Promise<void> {
  return invoke("tool_upsert", { payload: p });
}
export function toolRemove(id: string): Promise<void> {
  return invoke("tool_remove", { id });
}
export function exampleToolsInstall(): Promise<string[]> {
  return invoke("example_tools_install");
}
export function maintenanceSopInstall(): Promise<string> {
  return invoke("maintenance_sop_install");
}
export function delegationDemoInstall(): Promise<string> {
  return invoke("delegation_demo_install");
}
export function llmStatus(): Promise<{ name: string; mock: boolean }> {
  return invoke("llm_status");
}

// ---- 补齐 IPC（§3.1）：sop_get / config / mcp_server ----
export type SopVersionInfo = { version: number; definition: Record<string, unknown>; ranAt: number | null };
export type SopDetail = { sop: SopRow; versions: SopVersionInfo[] };
export function sopGet(id: string): Promise<SopDetail> {
  return invoke("sop_get", { id });
}
export function configGet(): Promise<Record<string, string>> {
  return invoke("config_get");
}
export function configSet(key: string, value: string): Promise<void> {
  return invoke("config_set", { payload: { key, value } });
}
export type McpServerInfo = { id: string; command: string; args: string[]; note: string; transport: string; url: string };
export function mcpServerList(): Promise<McpServerInfo[]> {
  return invoke("mcp_server_list");
}
export function mcpServerAdd(s: McpServerInfo): Promise<McpServerInfo[]> {
  return invoke("mcp_server_add", { payload: s });
}
export function mcpServerRemove(id: string): Promise<McpServerInfo[]> {
  return invoke("mcp_server_remove", { id });
}
export function sopParseText(text: string): Promise<Record<string, unknown>> {
  return invoke("sop_parse_text", { text });
}

// ---- 用户管理与权限（2026-11-24）----
export type UserRow = {
  id: string;
  username: string;
  displayName: string;
  role: string;
  createdAt: number;
};
export type CurrentUser = {
  id: string;
  username: string;
  displayName: string;
  role: string;
};
export function userList(): Promise<UserRow[]> {
  return invoke("user_list");
}
export function userCurrent(): Promise<CurrentUser | null> {
  return invoke("user_current");
}
export function userAdd(username: string, displayName: string, role: string, password: string): Promise<UserRow> {
  return invoke("user_add", { payload: { username, displayName, role, password } });
}
export function userLogin(username: string, password: string): Promise<CurrentUser> {
  return invoke("user_login", { payload: { username, password } });
}
export function userLogout(): Promise<void> {
  return invoke("user_logout");
}
export function userSetRole(userId: string, role: string): Promise<UserRow> {
  return invoke("user_set_role", { payload: { userId, role } });
}
export function userRemove(userId: string): Promise<void> {
  return invoke("user_remove", { userId });
}

export function errMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const err = e as AppError;
    return `${err.code}: ${err.message}`;
  }
  return String(e);
}
