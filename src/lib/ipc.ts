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

export function errMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    const err = e as AppError;
    return `${err.code}: ${err.message}`;
  }
  return String(e);
}
