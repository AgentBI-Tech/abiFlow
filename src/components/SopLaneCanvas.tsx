import { useMemo, useState } from "react";

import type { SopRow, StepRow } from "../lib/ipc";

// 泳道图（参考 ShopPilot 原型前三项之一，2026-11-26）：谁做什么、在哪一步交接给谁。
// 纯 DOM/CSS，不用图表库；与 SopCanvas 并列 tab，原只读流程图保留。

type AstNode = { id: string; type: string; config?: Record<string, unknown> };
type AstEdge = { from: string; to: string; when?: string };
type AstDef = { version?: number; nodes: AstNode[]; edges: AstEdge[] };

const TYPE_LABEL: Record<string, string> = {
  start: "开始", agent_step: "Agent 步骤", mcp_tool: "工具调用",
  human_gate: "人工门", condition: "条件分支", acceptance: "验收门", end: "结束",
};

const LANES = [
  { key: "human", label: "运营者", en: "HUMAN", cls: "human", types: ["human_gate", "acceptance"] },
  { key: "agent", label: "AI Agent", en: "REASONING", cls: "agent", types: ["agent_step"] },
  { key: "tool", label: "工具与调度", en: "MCP · SCHEDULER", cls: "tool", types: ["start", "mcp_tool", "condition", "end"] },
  { key: "ext", label: "外部系统", en: "EXTERNAL", cls: "ext", types: [] },
] as const;

const STEP_COLOR: Record<string, string> = {
  done: "var(--ok)",
  completed: "var(--ok)",
  running: "var(--brand)",
  wait_human: "#d97706",
  failed: "var(--danger)",
  rejected: "#d97706",
};

function laneOf(n: AstNode): string {
  const hit = LANES.find((l) => (l.types as readonly string[]).includes(n.type));
  return hit ? hit.key : "tool";
}
function nodeDetail(n: AstNode): string {
  const c = n.config ?? {};
  switch (n.type) {
    case "mcp_tool": return typeof c.tool === "string" ? c.tool : "未指定工具";
    case "agent_step": return typeof c.goal === "string" ? c.goal : "未设定目标";
    case "human_gate": return typeof c.reason === "string" ? c.reason : "请人工确认";
    case "condition": return typeof c.expr === "string" ? c.expr : "未设条件";
    case "acceptance": return `${Array.isArray(c.criteria) ? c.criteria.length : 0} 条判定标准`;
    default: return "";
  }
}
function clip(s: string, n: number) { return s.length > n ? s.slice(0, n - 1) + "…" : s; }

function layout(def: AstDef) {
  const byId = new Map(def.nodes.map((n) => [n.id, n]));
  const out = new Map<string, AstEdge[]>();
  for (const e of def.edges) { const a = out.get(e.from) ?? []; a.push(e); out.set(e.from, a); }
  const depth = new Map<string, number>();
  const start = def.nodes.find((n) => n.type === "start");
  if (start) {
    const q = [start.id]; depth.set(start.id, 0);
    while (q.length) {
      const cur = q.shift()!;
      for (const e of out.get(cur) ?? []) {
        if (byId.has(e.to) && !depth.has(e.to)) { depth.set(e.to, depth.get(cur)! + 1); q.push(e.to); }
      }
    }
  }
  const maxD = Math.max(0, ...depth.values());
  for (const n of def.nodes) if (!depth.has(n.id)) depth.set(n.id, maxD + 1);
  const cols = Math.max(1, Math.max(0, ...depth.values()) + 1);
  return { depth, cols, out };
}

export default function SopLaneCanvas({
  sop,
  steps = [],
}: {
  sop: SopRow;
  steps?: StepRow[];
}) {
  const [selected, setSelected] = useState<string | null>(null);

  const graph = useMemo(() => {
    let def: AstDef;
    try { def = JSON.parse(sop.definition) as AstDef; } catch { return null; }
    if (!Array.isArray(def.nodes) || !Array.isArray(def.edges)) return null;
    const { depth, cols, out } = layout(def);
    return { def, depth, cols, out };
  }, [sop.definition]);

  if (!graph || graph.def.nodes.length === 0) {
    return <section className="card"><p className="empty">SOP 定义无法解析为泳道图。</p></section>;
  }

  const { def, depth, cols, out } = graph;
  const stepByNode = new Map<string, StepRow>();
  for (const s of steps) if (s.nodeId && !stepByNode.has(s.nodeId)) stepByNode.set(s.nodeId, s);

  const gridStyle = { gridTemplateColumns: `112px repeat(${cols}, minmax(150px, 1fr))` };
  const selNode = selected ? def.nodes.find((n) => n.id === selected) : null;
  const selStep = selected ? stepByNode.get(selected) : null;

  return (
    <section className="card lane-canvas" data-testid="sop-lane-canvas">
      <div className="row">
        <h2>泳道图 · {sop.name}</h2>
        <span className="ver">v{sop.version} · {sop.id}</span>
      </div>
      <p className="sub-row">四条泳道对应运营者 / Agent / 工具调度 / 外部系统；横向按流程顺序展开。点节点看详情与交接。</p>
      <div className="lane-grid-scroll">
        <div className="lane-grid" style={gridStyle}>
          <div className="lane-corner">泳道 / 步骤</div>
          {Array.from({ length: cols }, (_, i) => (
            <div key={i} className="lane-col-head">步骤 {i + 1}</div>
          ))}
          {LANES.map((lane) => (
            <div key={lane.key} className="lane-row-wrap" style={{ display: "contents" }}>
              <div className={`lane-tag ${lane.cls}`}>
                <strong>{lane.label}</strong><span>{lane.en}</span>
              </div>
              {Array.from({ length: cols }, (_, col) => {
                const nodes = def.nodes.filter((n) => depth.get(n.id) === col && laneOf(n) === lane.key);
                const extNodes = lane.key === "ext"
                  ? def.nodes.filter((n) => depth.get(n.id) === col && n.type === "mcp_tool" && typeof n.config?.tool === "string")
                  : [];
                if (!nodes.length && !extNodes.length) return <div key={col} className="lane-cell" />;
                return (
                  <div key={col} className="lane-cell">
                    {nodes.map((n) => {
                      const st = stepByNode.get(n.id);
                      const next = out.get(n.id)?.[0];
                      const nextLane = next ? LANES.find((l) => l.key === laneOf(def.nodes.find((x) => x.id === next.to)!)) : null;
                      const border = st && STEP_COLOR[st.status] ? STEP_COLOR[st.status] : undefined;
                      return (
                        <button key={n.id} type="button"
                          className={`lane-node ${lane.cls} ${selected === n.id ? "sel" : ""}`}
                          data-node-id={n.id}
                          style={border ? { borderColor: border } : undefined}
                          onClick={() => setSelected(n.id)}>
                          <span className="lane-node-type">{TYPE_LABEL[n.type] ?? n.type}</span>
                          <strong className="lane-node-id">{n.id}</strong>
                          <span className="lane-node-detail">{clip(nodeDetail(n), 18)}</span>
                          {st && <span className="lane-node-step">{st.status}</span>}
                          {next && (
                            <span className="lane-node-next">
                              → {next.to}{nextLane && nextLane.key !== lane.key ? ` (${nextLane.label})` : ""}
                            </span>
                          )}
                        </button>
                      );
                    })}
                    {extNodes.map((n) => (
                      <div key={`ext-${n.id}`} className="lane-ext-node" data-ext-for={n.id}>
                        <span>被调用</span>
                        <strong>{clip(String(n.config?.tool), 18)}</strong>
                      </div>
                    ))}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </div>

      {selNode && (
        <div className="lane-detail" data-selected-node={selNode.id}>
          <div className="row">
            <strong>{TYPE_LABEL[selNode.type] ?? selNode.type} · {selNode.id}</strong>
            <button className="close" onClick={() => setSelected(null)}>关闭</button>
          </div>
          <p className="sub-row">{nodeDetail(selNode)}</p>
          {selStep ? (
            <dl className="detail-dl">
              <div><dt>步骤状态</dt><dd>{selStep.status}</dd></div>
              <div><dt>执行主体</dt><dd>{selStep.owner}</dd></div>
              <div><dt>工具</dt><dd>{selStep.toolRef ?? "—"}</dd></div>
              <div><dt>开始</dt><dd>{selStep.startedAt ? new Date(selStep.startedAt * 1000).toLocaleString("zh-CN", { hour12: false }) : "—"}</dd></div>
            </dl>
          ) : <p className="sub-row">本次 run 尚未走到该节点。</p>}
        </div>
      )}
    </section>
  );
}
