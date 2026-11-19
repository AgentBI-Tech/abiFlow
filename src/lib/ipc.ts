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
};

export type StepRow = {
  id: string;
  runId: string;
  seq: number;
  nodeType: string;
  owner: string;
  toolRef: string | null;
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
};

export type DashboardData = {
  sops: SopRow[];
  runs: RunRow[];
  pendingGates: GateRow[];
  schedules: ScheduleInfo[];
  toolCount: number;
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
  nodeType: string;
  nodeId: string;
  correctionCount: number;
  threshold: number;
  eligible: boolean;
  reason: string;
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
export function runControl(runId: string, action: string): Promise<string> {
  return invoke("run_control", { payload: { runId, action } });
}
export function gateDecide(gateId: string, decision: string, note?: string): Promise<void> {
  return invoke("gate_decide", { payload: { gateId, decision, note: note ?? null } });
}
export function schedulerTick(): Promise<SchedulerTickResult> {
  return invoke("scheduler_tick");
}
export function demoInstall(): Promise<string> {
  return invoke("demo_install");
}
export function sopSetEnabled(id: string, enabled: boolean): Promise<void> {
  return invoke("sop_set_enabled", { id, enabled });
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
export function correctionList(stepId: string | null): Promise<CorrectionRow[]> {
  return invoke("correction_list", { stepId });
}
export function upgradeCandidateList(): Promise<UpgradeCandidate[]> {
  return invoke("upgrade_candidate_list");
}
export function upgradeApprove(nodeId: string, approve: boolean): Promise<void> {
  return invoke("upgrade_approve", { payload: { nodeId, approve } });
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

export function errMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const err = e as AppError;
    return `${err.code}: ${err.message}`;
  }
  return String(e);
}
