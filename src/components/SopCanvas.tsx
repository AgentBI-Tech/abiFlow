import { useMemo } from "react";
import {
  Background,
  Controls,
  MarkerType,
  ReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import type { SopRow } from "../lib/ipc";

// v0.1「空画布」：把 SOP 定义的 AST 只读渲染成节点图（不做拖拽编辑，那是未定期数的完整编辑器）。
// 7 类节点 → 分色展示；边带 when 条件标签；布局按从 start 的 BFS 层级自动排布。

type AstNode = { id: string; type: string; config?: Record<string, unknown> };
type AstEdge = { from: string; to: string; when?: string };
type AstDef = { version?: number; nodes: AstNode[]; edges: AstEdge[] };

const TYPE_LABEL: Record<string, string> = {
  start: "开始",
  agent_step: "Agent 步骤",
  mcp_tool: "工具调用",
  human_gate: "人工门",
  condition: "条件分支",
  acceptance: "验收门",
  end: "结束",
};

const TYPE_COLOR: Record<string, string> = {
  start: "#0ea5e9",
  agent_step: "#8b5cf6",
  mcp_tool: "#2563eb",
  human_gate: "#f59e0b",
  condition: "#14b8a6",
  acceptance: "#10b981",
  end: "#64748b",
};

function nodeSummary(n: AstNode): string {
  const c = n.config ?? {};
  switch (n.type) {
    case "mcp_tool":
      return typeof c.tool === "string" ? c.tool : "未指定工具";
    case "agent_step":
      return typeof c.goal === "string" ? c.goal : "未设定目标";
    case "human_gate":
      return typeof c.reason === "string" ? c.reason : "请人工确认";
    case "condition":
      return typeof c.expr === "string" ? c.expr : "未设条件";
    case "acceptance": {
      const criteria = Array.isArray(c.criteria) ? c.criteria.length : 0;
      return `${criteria} 条判定标准`;
    }
    default:
      return "";
  }
}

type FlowData = { astId: string; label: string; summary: string; color: string };
type SopFlowNode = Node<FlowData, "sop">;

function SopNodeView({ data }: NodeProps<SopFlowNode>) {
  return (
    <div
      className="sop-node"
      data-node-id={data.astId}
      style={{ borderColor: data.color }}
    >
      <div className="sop-node-label" style={{ background: data.color }}>
        {data.label}
      </div>
      <div className="sop-node-id">{data.astId}</div>
      {data.summary && <div className="sop-node-summary">{data.summary}</div>}
    </div>
  );
}

const nodeTypes = { sop: SopNodeView };

/** 从 start 做 BFS 得到每个节点的层级；不可达节点放最后一列。 */
function layout(nodes: AstNode[], edges: AstEdge[]): Map<string, { depth: number; index: number }> {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const outgoing = new Map<string, AstEdge[]>();
  for (const e of edges) {
    const arr = outgoing.get(e.from) ?? [];
    arr.push(e);
    outgoing.set(e.from, arr);
  }
  const depth = new Map<string, number>();
  const start = nodes.find((n) => n.type === "start");
  if (start) {
    const queue = [start.id];
    depth.set(start.id, 0);
    while (queue.length) {
      const cur = queue.shift()!;
      for (const e of outgoing.get(cur) ?? []) {
        if (!byId.has(e.to) || depth.has(e.to)) continue;
        depth.set(e.to, depth.get(cur)! + 1);
        queue.push(e.to);
      }
    }
  }
  const maxDepth = Math.max(0, ...depth.values());
  for (const n of nodes) if (!depth.has(n.id)) depth.set(n.id, maxDepth + 1);

  const perDepth = new Map<number, number>();
  const pos = new Map<string, { depth: number; index: number }>();
  for (const n of nodes) {
    const d = depth.get(n.id)!;
    const i = perDepth.get(d) ?? 0;
    perDepth.set(d, i + 1);
    pos.set(n.id, { depth: d, index: i });
  }
  return pos;
}

export default function SopCanvas({ sop }: { sop: SopRow }) {
  const { flowNodes, flowEdges } = useMemo(() => {
    let def: AstDef;
    try {
      def = JSON.parse(sop.definition) as AstDef;
    } catch {
      return { flowNodes: [] as SopFlowNode[], flowEdges: [] as Edge[] };
    }
    if (!Array.isArray(def.nodes) || !Array.isArray(def.edges)) {
      return { flowNodes: [] as SopFlowNode[], flowEdges: [] as Edge[] };
    }
    const pos = layout(def.nodes, def.edges);
    const fn: SopFlowNode[] = def.nodes.map((n) => {
      const p = pos.get(n.id)!;
      return {
        id: n.id,
        type: "sop",
        position: { x: p.depth * 260, y: p.index * 118 },
        data: {
          astId: n.id,
          label: TYPE_LABEL[n.type] ?? n.type,
          summary: nodeSummary(n),
          color: TYPE_COLOR[n.type] ?? "#64748b",
        },
        draggable: false,
        selectable: false,
        connectable: false,
      };
    });
    const fe: Edge[] = def.edges.map((e) => ({
      id: `${e.from}->${e.to}`,
      source: e.from,
      target: e.to,
      type: "smoothstep",
      label: e.when,
      markerEnd: { type: MarkerType.ArrowClosed },
    }));
    return { flowNodes: fn, flowEdges: fe };
  }, [sop.definition]);

  return (
    <section className="card" data-testid="sop-canvas">
      <div className="row">
        <h2>SOP 画布 · {sop.name}</h2>
        <span className="ver">v{sop.version} · {sop.id}</span>
      </div>
      <p className="sub-row">只读预览：按 AST 自动排布，7 类节点分色，边标注 when 条件。编排编辑尚未实现。</p>
      {flowNodes.length === 0 ? (
        <p className="empty">SOP 定义无法解析为画布（definition 非法或为空）。</p>
      ) : (
        <div className="sop-canvas-wrap" data-node-count={flowNodes.length} data-edge-count={flowEdges.length}>
          <ReactFlow
            nodes={flowNodes}
            edges={flowEdges}
            nodeTypes={nodeTypes}
            fitView
            fitViewOptions={{ padding: 0.2 }}
            nodesDraggable={false}
            nodesConnectable={false}
            elementsSelectable={false}
            panOnDrag
            zoomOnScroll
          >
            <Background />
            <Controls showInteractive={false} />
          </ReactFlow>
        </div>
      )}
    </section>
  );
}
