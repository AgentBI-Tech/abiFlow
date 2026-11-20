import { useEffect, useMemo, useRef, useState } from "react";

import type { SopRow } from "../lib/ipc";

// v0.1「空画布」：SOP AST 只读流程图。节点框、文字、连线全部是同一个 <svg> 的元素——
// 物理上只有一层，不会在 WebKitGTK 下出现「线在上、块在下」的错位。
// 不用 ReactFlow：其 ResizeObserver/fitView 在 Tauri WebKitGTK 下不工作。

type AstNode = { id: string; type: string; config?: Record<string, unknown> };
type AstEdge = { from: string; to: string; when?: string };
type AstDef = { version?: number; nodes: AstNode[]; edges: AstEdge[] };

const TYPE_LABEL: Record<string, string> = {
  start: "开始", agent_step: "Agent 步骤", mcp_tool: "工具调用",
  human_gate: "人工门", condition: "条件分支", acceptance: "验收门", end: "结束",
};
const TYPE_COLOR: Record<string, string> = {
  start: "#0284c7", agent_step: "#7c3aed", mcp_tool: "#2563eb",
  human_gate: "#d97706", condition: "#0d9488", acceptance: "#059669", end: "#475569",
};

const NODE_W = 188;
const NODE_H = 74;
const GAP_X = 56;
const GAP_Y = 46;
const PAD = 24;
const COLS = 3;

function nodeDetail(n: AstNode): string {
  const c = n.config ?? {};
  switch (n.type) {
    case "mcp_tool": return typeof c.tool === "string" ? c.tool : "未指定工具";
    case "agent_step": return typeof c.goal === "string" ? c.goal : "未设定目标";
    case "human_gate": return typeof c.reason === "string" ? c.reason : "请人工确认";
    case "condition": return typeof c.expr === "string" ? c.expr : "未设条件";
    case "acceptance": {
      const k = Array.isArray(c.criteria) ? c.criteria.length : 0;
      return `${k} 条判定标准`;
    }
    default: return "";
  }
}
function clip(s: string, n: number) { return s.length > n ? s.slice(0, n - 1) + "…" : s; }

/** 紧凑蛇形网格：按 BFS 定序，每行 COLS 个，奇数行反向，让连线尽量顺。 */
function gridLayout(nodes: AstNode[], edges: AstEdge[]) {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const out = new Map<string, AstEdge[]>();
  for (const e of edges) { const a = out.get(e.from) ?? []; a.push(e); out.set(e.from, a); }
  const depth = new Map<string, number>();
  const start = nodes.find((n) => n.type === "start");
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
  for (const n of nodes) if (!depth.has(n.id)) depth.set(n.id, maxD + 1);
  const order = [...nodes].sort((a, b) => (depth.get(a.id)! - depth.get(b.id)!));

  const pos = new Map<string, { x: number; y: number; cx: number; cy: number }>();
  order.forEach((n, i) => {
    const row = Math.floor(i / COLS);
    const col = i % COLS;
    const cc = row % 2 === 0 ? col : (COLS - 1 - col); // 蛇形
    const x = PAD + cc * (NODE_W + GAP_X);
    const y = PAD + row * (NODE_H + GAP_Y);
    pos.set(n.id, { x, y, cx: x + NODE_W / 2, cy: y + NODE_H / 2 });
  });
  const rows = Math.ceil(order.length / COLS);
  const width = PAD * 2 + COLS * NODE_W + (COLS - 1) * GAP_X;
  const height = PAD * 2 + rows * NODE_H + (rows - 1) * GAP_Y;
  return { pos, width, height };
}

export default function SopCanvas({ sop }: { sop: SopRow }) {
  const [zoom, setZoom] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const viewportRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{ sx: number; sy: number; ox: number; oy: number } | null>(null);

  const graph = useMemo(() => {
    let def: AstDef;
    try { def = JSON.parse(sop.definition) as AstDef; } catch { return null; }
    if (!Array.isArray(def.nodes) || !Array.isArray(def.edges)) return null;
    const { pos, width, height } = gridLayout(def.nodes, def.edges);
    const nodes = def.nodes.map((n) => {
      const p = pos.get(n.id)!;
      return {
        ...n, ...p,
        color: TYPE_COLOR[n.type] ?? "#475569",
        label: TYPE_LABEL[n.type] ?? n.type,
        detail: clip(nodeDetail(n), 22),
      };
    });
    const edges = def.edges
      .filter((e) => pos.has(e.from) && pos.has(e.to))
      .map((e) => ({ ...e, a: pos.get(e.from)!, b: pos.get(e.to)! }));
    return { nodes, edges, width, height };
  }, [sop.definition]);

  function fit() {
    if (!graph || !viewportRef.current) return;
    const el = viewportRef.current;
    const z = Math.max(0.4, Math.min(1.1, (el.clientWidth - 24) / graph.width));
    setZoom(z);
    setOffset({ x: 10, y: 10 });
  }
  useEffect(() => { fit(); /* eslint-disable-next-line */ }, [sop.definition]);

  function onMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    drag.current = { sx: e.clientX, sy: e.clientY, ox: offset.x, oy: offset.y };
  }
  function onMouseMove(e: React.MouseEvent) {
    if (!drag.current) return;
    setOffset({
      x: drag.current.ox + (e.clientX - drag.current.sx),
      y: drag.current.oy + (e.clientY - drag.current.sy),
    });
  }
  const onMouseUp = () => { drag.current = null; };

  return (
    <section className="card" data-testid="sop-canvas">
      <div className="row">
        <h2>SOP 画布 · {sop.name}</h2>
        <span className="ver">v{sop.version} · {sop.id}</span>
      </div>
      <p className="sub-row">只读流程图：从「开始」按箭头走向，色条为节点类型，细字为 when 条件。默认全览，按住可拖动。</p>
      {!graph || graph.nodes.length === 0 ? (
        <p className="empty">SOP 定义无法解析为画布（definition 非法或为空）。</p>
      ) : (
        <>
          <div className="sop-canvas-tools">
            <button onClick={fit}>全览</button>
            <label>缩放
              <input type="range" min="0.4" max="1.6" step="0.05" value={zoom}
                onChange={(e) => setZoom(Number(e.target.value))} />
            </label>
            <span className="ver">{Math.round(zoom * 100)}%</span>
            <span className="ver">节点 {graph.nodes.length} · 连线 {graph.edges.length} · 按住拖动</span>
          </div>
          <div className="sop-canvas-viewport" ref={viewportRef}
            onMouseDown={onMouseDown} onMouseMove={onMouseMove} onMouseUp={onMouseUp} onMouseLeave={onMouseUp}>
            <div className="sop-canvas-stage" data-node-count={graph.nodes.length} data-edge-count={graph.edges.length}
              style={{ width: graph.width, height: graph.height,
                transform: `translate(${offset.x}px, ${offset.y}px) scale(${zoom})` }}>
              <svg width={graph.width} height={graph.height} style={{ display: "block", overflow: "visible" }}>
                <defs>
                  <marker id="sop-arrow" viewBox="0 0 10 10" refX="9" refY="5"
                    markerWidth={6} markerHeight={6} markerUnits="userSpaceOnUse"
                    orient="auto-start-reverse">
                    <path d="M 0 0 L 10 5 L 0 10 z" fill="#64748b" />
                  </marker>
                </defs>
                {graph.edges.map((e, i) => {
                  let x1: number, y1: number, x2: number, y2: number, d: string;
                  if (e.a.cy === e.b.cy) {
                    // 同一行：从右边缘到左边缘（或反向蛇形）
                    const right = e.a.cx < e.b.cx;
                    x1 = right ? e.a.x + NODE_W : e.a.x;
                    x2 = right ? e.b.x : e.b.x + NODE_W;
                    y1 = e.a.cy; y2 = e.b.cy;
                    d = `M ${x1} ${y1} L ${x2} ${y2}`;
                  } else {
                    // 跨行：从下边缘到上边缘，竖向贝塞尔
                    x1 = e.a.cx; y1 = e.a.y + NODE_H;
                    x2 = e.b.cx; y2 = e.b.y;
                    const mid = (y1 + y2) / 2;
                    d = `M ${x1} ${y1} C ${x1} ${mid}, ${x2} ${mid}, ${x2} ${y2}`;
                  }
                  return (
                    <g key={`${e.from}-${e.to}-${i}`}>
                      <path d={d} stroke="#64748b" strokeWidth={2.5} fill="none"
                        markerEnd="url(#sop-arrow)" />
                      {e.when && (
                        <text x={(x1 + x2) / 2} y={(y1 + y2) / 2 - 6} textAnchor="middle"
                          fontSize={11} fill="#94a3b8" fontFamily="ui-monospace, monospace">
                          {e.when}
                        </text>
                      )}
                    </g>
                  );
                })}
                {graph.nodes.map((n) => (
                  <g key={n.id} data-node-id={n.id}>
                    <rect x={n.x} y={n.y} width={NODE_W} height={NODE_H} rx="8"
                      fill="#fff" stroke={n.color} strokeWidth={2} />
                    {/* 色条：rect 从 y 到 y+22，顶部两角随方框圆角；不用 stroke 中心线（会偏下） */}
                    <path d={`M ${n.x} ${n.y + 22} L ${n.x} ${n.y + 8} Q ${n.x} ${n.y} ${n.x + 8} ${n.y} L ${n.x + NODE_W - 8} ${n.y} Q ${n.x + NODE_W} ${n.y} ${n.x + NODE_W} ${n.y + 8} L ${n.x + NODE_W} ${n.y + 22} Z`}
                      fill={n.color} />
                    <text x={n.x + 8} y={n.y + 16} fontSize={12}
                      fill="#fff" fontWeight="600">{n.label}</text>
                    <text x={n.x + 8} y={n.y + 40} fontSize={12}
                      fill="#1e293b" fontFamily="ui-monospace, monospace" fontWeight="600">{n.id}</text>
                    {n.detail && (
                      <text x={n.x + 8} y={n.y + 60} fontSize={11}
                        fill="#64748b" fontFamily="ui-monospace, monospace">{n.detail}</text>
                    )}
                  </g>
                ))}
              </svg>
            </div>
          </div>
        </>
      )}
    </section>
  );
}
