// 完整 SOP 编辑器（ReactFlow）：把 AST（definition）渲染成可编辑画布，
// 支持增删节点、连线、按类型改 config、编辑条件分支，保存回 AST 走 sop_upsert（含后端校验）。
// AST 本身不含坐标，布局用 BFS 蛇形自动算；节点带显式 style 宽高——实测 WebKitGTK 下
// 缺尺寸会让 fitView 把节点缩到不可见（11-20 白屏），显式尺寸后 13.0.2 渲染正常。

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  addEdge,
  Handle,
  Position,
  useNodesState,
  useEdgesState,
  type Connection,
  type Node,
  type Edge,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import { sopGet, sopUpsert, type SopRow, type SopVersionInfo } from "../lib/ipc";

type AstNode = { id: string; type: string; config?: Record<string, unknown> };
type AstEdge = { from: string; to: string; when?: string };
type AstDef = { version?: number; nodes: AstNode[]; edges: AstEdge[] };

const TYPES = ["start", "agent_step", "mcp_tool", "human_gate", "condition", "acceptance", "end"] as const;
const TYPE_LABEL: Record<string, string> = {
  start: "开始", agent_step: "Agent 步骤", mcp_tool: "工具调用",
  human_gate: "人工门", condition: "条件分支", acceptance: "验收门", end: "结束",
};
const TYPE_COLOR: Record<string, string> = {
  start: "#0284c7", agent_step: "#7c3aed", mcp_tool: "#2563eb",
  human_gate: "#d97706", condition: "#0d9488", acceptance: "#059669", end: "#475569",
};
const NODE_W = 176;
const NODE_H = 58;
const GAP_X = 48;
const GAP_Y = 36;
const PAD = 20;
const COLS = 3;

function nodeDetail(n: AstNode): string {
  const c = n.config ?? {};
  switch (n.type) {
    case "mcp_tool": return typeof c.tool === "string" ? c.tool : "未指定工具";
    case "agent_step": return typeof c.goal === "string" ? c.goal : "未设目标";
    case "human_gate": return typeof c.reason === "string" ? c.reason : "请人工确认";
    case "condition": return typeof c.expr === "string" ? c.expr : "未设条件";
    case "acceptance": return `${Array.isArray(c.criteria) ? c.criteria.length : 0} 条判定`;
    default: return "";
  }
}
function clip(s: string, n: number) { return s.length > n ? s.slice(0, n - 1) + "…" : s; }

/** AST → ReactFlow nodes/edges。BFS 定序 + 蛇形网格；节点显式尺寸。 */
function astToFlow(def: AstDef): { nodes: Node[]; edges: Edge[] } {
  const byId = new Map(def.nodes.map((n) => [n.id, n]));
  const out = new Map<string, AstEdge[]>();
  for (const e of def.edges) {
    const a = out.get(e.from) ?? [];
    a.push(e); out.set(e.from, a);
  }
  const depth = new Map<string, number>();
  const start = def.nodes.find((n) => n.type === "start");
  if (start) {
    const q = [start.id]; depth.set(start.id, 0);
    while (q.length) {
      const cur = q.shift()!;
      for (const e of out.get(cur) ?? []) {
        if (byId.has(e.to) && !depth.has(e.to)) {
          depth.set(e.to, depth.get(cur)! + 1); q.push(e.to);
        }
      }
    }
  }
  const maxD = Math.max(0, ...depth.values());
  for (const n of def.nodes) if (!depth.has(n.id)) depth.set(n.id, maxD + 1);
  const order = [...def.nodes].sort((a, b) => depth.get(a.id)! - depth.get(b.id)!);
  const pos = new Map<string, { x: number; y: number }>();
  order.forEach((n, i) => {
    const row = Math.floor(i / COLS);
    const col = i % COLS;
    const cc = row % 2 === 0 ? col : (COLS - 1 - col);
    pos.set(n.id, { x: PAD + cc * (NODE_W + GAP_X), y: PAD + row * (NODE_H + GAP_Y) });
  });
  const nodes: Node[] = def.nodes.map((n) => ({
    id: n.id,
    position: pos.get(n.id) ?? { x: PAD, y: PAD },
    data: {
      label: TYPE_LABEL[n.type] ?? n.type,
      detail: clip(nodeDetail(n), 18),
      color: TYPE_COLOR[n.type] ?? "#475569",
      nodeType: n.type,
    },
    style: { width: NODE_W, minHeight: NODE_H },
  }));
  const edges: Edge[] = def.edges
    .filter((e) => pos.has(e.from) && pos.has(e.to))
    .map((e, i) => ({
      id: `e-${e.from}-${e.to}-${i}`,
      source: e.from,
      target: e.to,
      label: e.when ? clip(e.when, 14) : undefined,
      data: { when: e.when ?? "" },
    }));
  return { nodes, edges };
}

/** ReactFlow nodes/edges → AST。config 从节点 data.cfg 取（编辑面板写入）。 */
function flowToAst(nodes: Node[], edges: Edge[], version: number): AstDef {
  return {
    version,
    nodes: nodes.map((n) => {
      const cfg = (((n.data as Record<string, unknown>).cfg as Record<string, unknown>) ?? {});
      // args 在编辑面板是文本，运行时必须是 JSON 对象（工具按 key 取参）；这里回转。
      let fixed = { ...cfg };
      if (typeof fixed.args === "string") {
        try { fixed.args = JSON.parse(fixed.args as string); }
        catch { fixed.args = {}; }
      }
      return {
        id: n.id,
        type: String((n.data as Record<string, unknown>).nodeType ?? "agent_step"),
        config: fixed,
      };
    }),
    edges: edges.map((e) => {
      const when = (e.data as Record<string, unknown> | undefined)?.when;
      const base: AstEdge = { from: e.source, to: e.target };
      if (typeof when === "string" && when.trim()) base.when = when.trim();
      return base;
    }),
  };
}

// 自定义节点：色条 + 类型名 + 明细，显式尺寸，稳定渲染。
const SCNode = ({ data, selected }: { data: Record<string, unknown>; selected?: boolean }) => {
  const color = String(data.color ?? "#475569");
  return (
    <div
      data-sop-node={String(data.nodeType)}
      style={{
        width: "100%", minHeight: NODE_H, boxSizing: "border-box",
        border: `2px solid ${selected ? "#0ea5e9" : color}`, borderRadius: 8,
        background: "#fff", overflow: "hidden", fontSize: 12,
        boxShadow: selected ? "0 0 0 3px rgba(14,165,233,.25)" : undefined,
      }}
    >
      <div style={{ background: color, color: "#fff", padding: "3px 8px", fontWeight: 600 }}>
        {String(data.label ?? "")}
      </div>
      <div style={{ padding: "4px 8px", color: "#475569", fontFamily: "ui-monospace, monospace" }}>
        {String(data.detail ?? "") || "—"}
      </div>
      <Handle type="target" position={Position.Top} style={{ background: "#94a3b8", width: 8, height: 8 }} />
      <Handle type="source" position={Position.Bottom} style={{ background: "#94a3b8", width: 8, height: 8 }} />
    </div>
  );
};
const nodeTypes: NodeTypes = { sop: SCNode };

export default function SopEditor({ sop, onSaved }: { sop: SopRow; onSaved?: () => void }) {
  const [name, setName] = useState(sop.name);
  const [err, setErr] = useState("");
  const [saving, setSaving] = useState(false);
  const [selNode, setSelNode] = useState<string | null>(null);
  const [selEdge, setSelEdge] = useState<string | null>(null);
  const [versions, setVersions] = useState<SopVersionInfo[]>([]);

  const initial = useMemo(() => {
    let def: AstDef;
    try { def = JSON.parse(sop.definition) as AstDef; } catch { def = { version: 1, nodes: [], edges: [] }; }
    if (!Array.isArray(def.nodes)) def.nodes = [];
    if (!Array.isArray(def.edges)) def.edges = [];
    return def;
  }, [sop.definition]);

  // 每个节点的 config 存在 node.data.cfg，供属性面板编辑。
  const initialGraph = useMemo(() => {
    const { nodes: ns, edges: es } = astToFlow(initial);
    return {
      nodes: ns.map((n) => {
        const orig = initial.nodes.find((x) => x.id === n.id);
        return { ...n, type: "sop", data: { ...(n.data as object), cfg: orig?.config ?? {} } };
      }),
      edges: es,
    };
  }, [initial]);
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>(initialGraph.nodes);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>(initialGraph.edges);

  // 切换 SOP 时重载
  useEffect(() => {
    const { nodes: ns, edges: es } = astToFlow(initial);
    setNodes(ns.map((n) => {
      const orig = initial.nodes.find((x) => x.id === n.id);
      return { ...n, type: "sop", data: { ...(n.data as object), cfg: orig?.config ?? {} } };
    }));
    setEdges(es);
    setSelNode(null); setSelEdge(null);
    setName(sop.name); setErr("");
    // 版本历史（sop_get）：只读展示，可瞻仰"那次是哪版流程跑的"。
    void sopGet(sop.id).then((d) => setVersions(d.versions)).catch(() => setVersions([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sop.id, sop.definition]);

  const onConnect = useCallback(
    (c: Connection) => setEdges((eds) => addEdge({ ...c, id: `e-${c.source}-${c.target}-${Date.now()}` }, eds)),
    [setEdges]
  );

  function addNode(type: string) {
    const id = `n${Date.now().toString(36).slice(-5)}`;
    const cfg: Record<string, unknown> =
      type === "mcp_tool" ? { tool: "" } :
      type === "agent_step" ? { goal: "", owner: "agent" } :
      type === "human_gate" ? { reason: "", prompt: "" } :
      type === "condition" ? { expr: "true" } :
      type === "acceptance" ? { criteria: [] } : {};
    setNodes((ns) => [
      ...ns,
      {
        id, type: "sop",
        position: { x: PAD + (ns.length % COLS) * (NODE_W + GAP_X), y: PAD + Math.floor(ns.length / COLS) * (NODE_H + GAP_Y) },
        data: { label: TYPE_LABEL[type] ?? type, detail: "（未配置）", color: TYPE_COLOR[type] ?? "#475569", nodeType: type, cfg },
        style: { width: NODE_W, minHeight: NODE_H },
      },
    ]);
    setSelNode(id); setSelEdge(null);
  }

  function removeSelected() {
    if (selNode) {
      setNodes((ns) => ns.filter((n) => n.id !== selNode));
      setEdges((es) => es.filter((e) => e.source !== selNode && e.target !== selNode));
      setSelNode(null);
    } else if (selEdge) {
      setEdges((es) => es.filter((e) => e.id !== selEdge));
      setSelEdge(null);
    }
  }

  const selNodeObj = nodes.find((n) => n.id === selNode);
  const selEdgeObj = edges.find((e) => e.id === selEdge);

  function patchCfg(patch: Record<string, unknown>) {
    if (!selNodeObj) return;
    setNodes((ns) => ns.map((n) =>
      n.id === selNodeObj.id
        ? { ...n, data: { ...(n.data as object), cfg: { ...((n.data as Record<string, unknown>).cfg as object ?? {}), ...patch } } }
        : n
    ));
  }
  function refreshDetail(n: Node) {
    const cfg = ((n.data as Record<string, unknown>).cfg as Record<string, unknown>) ?? {};
    const fake: AstNode = { id: n.id, type: String((n.data as Record<string, unknown>).nodeType), config: cfg };
    const detail = clip(nodeDetail(fake), 18);
    setNodes((ns) => ns.map((x) => x.id === n.id ? { ...x, data: { ...(x.data as object), detail } } : x));
  }

  async function save() {
    setSaving(true); setErr("");
    try {
      const def = flowToAst(nodes, edges, initial.version ?? 1);
      await sopUpsert({
        id: sop.id,
        name: name.trim() || sop.name,
        triggerJson: sop.triggerJson,
        risk: sop.risk,
        ownerPolicy: sop.ownerPolicy,
        definition: def as unknown as Record<string, unknown>,
        expectVersion: sop.version,
      });
      onSaved?.();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="stack" data-testid="sop-editor">
      <div className="row actions">
        <label className="row">名称 <input value={name} onChange={(e) => setName(e.target.value)} /></label>
        <button onClick={removeSelected} disabled={(!selNode && !selEdge) || saving}>删除选中</button>
        <button onClick={save} disabled={saving}>{saving ? "保存中…" : "保存 SOP"}</button>
      </div>
      <div className="row actions" data-testid="add-node-bar">
        <span className="sub-row">添加节点：</span>
        {TYPES.filter((t) => t !== "start" && t !== "end").map((t) => (
          <button key={t} onClick={() => addNode(t)}>+{TYPE_LABEL[t]}</button>
        ))}
      </div>
      {err && <p className="err" data-testid="editor-error">{err}</p>}
      <div className="row" style={{ alignItems: "stretch", gap: 12 }}>
        <div style={{ width: "68%", height: 460, border: "1px solid #d4d4d8", borderRadius: 8, background: "#fafafa" }}>
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            onNodeClick={(_, n) => { setSelNode(n.id); setSelEdge(null); }}
            onEdgeClick={(_, e) => { setSelEdge(e.id); setSelNode(null); }}
            onPaneClick={() => { setSelNode(null); setSelEdge(null); }}
            fitView
          >
            <Background />
            <Controls />
          </ReactFlow>
        </div>
        <div style={{ width: "32%", border: "1px solid #d4d4d8", borderRadius: 8, padding: 12 }} data-testid="prop-panel">
          {selNodeObj ? (
            <>
              <h3>{TYPE_LABEL[String((selNodeObj.data as Record<string, unknown>).nodeType)] ?? "节点"} · {selNodeObj.id}</h3>
              <NodeConfigEditor
                node={selNodeObj}
                onPatch={(p) => { patchCfg(p); setTimeout(() => refreshDetail({ ...selNodeObj, data: { ...(selNodeObj.data as object), cfg: { ...((selNodeObj.data as Record<string, unknown>).cfg as object ?? {}), ...p } } }), 0); }}
              />
            </>
          ) : selEdgeObj ? (
            <>
              <h3>连线 · {selEdgeObj.source} → {selEdgeObj.target}</h3>
              <label className="stack">
                条件（when，condition 分支用；留空=默认分支）
                <input
                  value={String((selEdgeObj.data as Record<string, unknown> | undefined)?.when ?? "")}
                  onChange={(e) => {
                    const v = e.target.value;
                    setEdges((es) => es.map((x) => x.id === selEdgeObj.id ? { ...x, data: { when: v }, label: v ? clip(v, 14) : undefined } : x));
                  }}
                />
              </label>
            </>
          ) : (
            <p className="empty">点选节点或连线以编辑。拖节点端点可连线，选中后点「删除选中」移除。</p>
          )}
        </div>
      </div>
      <details>
        <summary>预览 definition（AST JSON）</summary>
        <pre data-testid="ast-preview">{JSON.stringify(flowToAst(nodes, edges, initial.version ?? 1), null, 2)}</pre>
      </details>
      <details data-testid="sop-versions">
        <summary>版本历史（{versions.length} 次运行快照）</summary>
        {versions.length === 0 ? (
          <p className="empty">尚无运行快照（此 SOP 还没有 run 记录）。</p>
        ) : (
          <table className="info">
            <thead><tr><th>版本</th><th>最近运行</th><th>节点数</th></tr></thead>
            <tbody>
              {versions.map((v) => (
                <tr key={v.version}>
                  <td>v{v.version}</td>
                  <td>{v.ranAt ? new Date(v.ranAt * 1000).toLocaleString("zh-CN", { hour12: false }) : "—"}</td>
                  <td>{Array.isArray(v.definition?.nodes) ? (v.definition.nodes as unknown[]).length : "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </details>
    </div>
  );
}

/** 按节点类型渲染 config 编辑控件。 */
function NodeConfigEditor({ node, onPatch }: { node: Node; onPatch: (p: Record<string, unknown>) => void }) {
  const cfg = ((node.data as Record<string, unknown>).cfg as Record<string, unknown>) ?? {};
  const type = String((node.data as Record<string, unknown>).nodeType);
  const str = (key: string, label: string, placeholder = "") => (
    <label className="stack">{label}
      <input value={String(cfg[key] ?? "")} placeholder={placeholder}
        onChange={(e) => onPatch({ [key]: e.target.value })} />
    </label>
  );
  switch (type) {
    case "mcp_tool":
      return <div className="stack">{str("tool", "工具 id（须已在工具与权限中注册）", "demo:check_anomalies")}
        <label className="stack">参数 JSON（args）
          <textarea rows={4} value={typeof cfg.args === "string" ? cfg.args : JSON.stringify(cfg.args ?? {}, null, 2)}
            onChange={(e) => onPatch({ args: e.target.value })} /></label></div>;
    case "agent_step":
      return <div className="stack">{str("goal", "任务目标")}
        <label className="stack">owner
          <select value={String(cfg.owner ?? "agent")} onChange={(e) => onPatch({ owner: e.target.value })}>
            <option value="agent">agent</option><option value="human">human</option>
          </select></label></div>;
    case "human_gate":
      return <div className="stack">{str("reason", "原因标识")}{str("prompt", "给人工看的提示")}</div>;
    case "condition":
      return <div className="stack">{str("expr", "条件表达式（如 steps.n1.status == 200）", "true")}
        <p className="sub-row">condition 需至少一条带 when 的出边（在线连线上填 when）。</p></div>;
    case "acceptance":
      return <div className="stack">
        <label className="stack">判定标准（每行一条）
          <textarea rows={5} value={Array.isArray(cfg.criteria) ? cfg.criteria.join("\n") : String(cfg.criteria ?? "")}
            onChange={(e) => onPatch({ criteria: e.target.value.split("\n").map((s) => s.trim()).filter(Boolean) })} /></label>
      </div>;
    default:
      return <p className="sub-row">该类型无需额外配置。</p>;
  }
}
