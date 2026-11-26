import type { RunRow } from "../lib/ipc";

// 首页看板 + 状态机（参考 ShopPilot 原型前三项之一，2026-11-26）。
// 只读展示：数据来自 runList；操作仍走运行历史里的既有按钮，不在此处新增执行入口。
// 纯 DOM + 原生 SVG，不用图表库，WebKitGTK 友好。

const STATUS_ZH: Record<string, string> = {
  pending: "待运行", running: "运行中", wait_human: "待人工", rejected: "已打回",
  review: "待仲裁", done: "完成", failed: "失败", cancelled: "已取消",
};

const COLS = [
  { key: "todo", label: "待办", statuses: ["pending"] },
  { key: "doing", label: "执行中", statuses: ["running"] },
  { key: "human", label: "待我处理", statuses: ["wait_human", "rejected", "review"] },
  { key: "done", label: "已完成", statuses: ["done", "failed", "cancelled"] },
] as const;

const STATES = [
  { key: "pending", cn: "待触发", en: "ready", x: 20, y: 88, w: 140, h: 56 },
  { key: "running", cn: "执行中", en: "running", x: 230, y: 88, w: 140, h: 56 },
  { key: "wait_human", cn: "等待人工", en: "waiting", x: 450, y: 88, w: 140, h: 56 },
  { key: "done", cn: "已完成", en: "done", x: 680, y: 88, w: 140, h: 56 },
  { key: "rejected", cn: "返工重跑", en: "rework", x: 230, y: 10, w: 140, h: 48 },
  { key: "failed", cn: "失败", en: "failed", x: 230, y: 170, w: 140, h: 48 },
] as const;

const LINKS = [
  { d: "M160 116 H230", tag: "", x: 0, y: 0 },
  { d: "M370 116 H450", tag: "人工关口", x: 410, y: 108 },
  { d: "M590 116 H680", tag: "批准", x: 635, y: 108 },
  { d: "M300 144 V170", tag: "异常", x: 316, y: 161 },
  { d: "M520 88 V66 H370", tag: "驳回", x: 452, y: 60 },
  { d: "M300 66 V88", tag: "重试", x: 316, y: 82 },
] as const;

function fmt(ts: number | null): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

export default function BoardPanel({
  runs,
  onOpen,
  activeStatus,
}: {
  runs: RunRow[];
  onOpen: (r: RunRow) => void;
  activeStatus?: string | null;
}) {
  return (
    <section className="card board-panel" data-testid="board-panel">
      <div className="row">
        <h2>任务看板 + 状态机</h2>
        <span className="ver">卡在哪个状态、卡在谁那里</span>
      </div>
      <ol className="board-guide">
        <li><b>① 看状态</b>：下方四列是所有任务；点任务卡 → 自动跳到该 run 的甘特图。</li>
        <li><b>② 跑 SOP</b>：到「SOP 列表」点「手动运行」，看板会随执行自动更新。</li>
        <li><b>③ 看时间线</b>：甘特图色带＝耗时，并排＝并行；点色带看输入/输出。</li>
      </ol>

      <div className="state-machine-wrap">
        <svg viewBox="0 0 840 230" width="100%" height="auto" role="img" aria-label="SOP 任务状态机">
          <defs>
            <marker id="sm-arrow" viewBox="0 0 10 10" refX="9" refY="5"
              markerWidth="7" markerHeight="7" orient="auto-start-reverse">
              <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--text-dim)" />
            </marker>
          </defs>
          {LINKS.map((l, i) => (
            <g key={i}>
              <path d={l.d} fill="none" stroke="var(--text-dim)" strokeWidth={1.8}
                markerEnd="url(#sm-arrow)" />
              {l.tag && (
                <text x={l.x} y={l.y} textAnchor="middle" fontSize={11}
                  fill="var(--text-dim)">{l.tag}</text>
              )}
            </g>
          ))}
          {STATES.map((s) => {
            const active = activeStatus === s.key;
            return (
              <g key={s.key} data-state={s.key} data-active={active ? "1" : "0"}>
                <rect x={s.x} y={s.y} width={s.w} height={s.h} rx={12}
                  fill={active ? "var(--brand-soft)" : "var(--panel-2)"}
                  stroke={active ? "var(--brand)" : "var(--border)"}
                  strokeWidth={active ? 2.5 : 1.2} />
                <text x={s.x + s.w / 2} y={s.y + 24} textAnchor="middle" fontSize={14}
                  fontWeight={600} fill="var(--text)">{s.cn}</text>
                <text x={s.x + s.w / 2} y={s.y + 43} textAnchor="middle" fontSize={11}
                  fill="var(--text-dim)" fontFamily="ui-monospace, monospace">{s.en}</text>
              </g>
            );
          })}
        </svg>
      </div>

      <div className="board-cols">
        {COLS.map((col) => {
          const list = runs.filter((r) => (col.statuses as readonly string[]).includes(r.status));
          return (
            <div key={col.key} className="board-col" data-col={col.key}>
              <div className="board-col-head">
                <span>{col.label}</span><span className="board-col-count">{list.length}</span>
              </div>
              <div className="board-col-list">
                {list.length === 0 ? (
                  <p className="board-empty">—</p>
                ) : list.map((r) => (
                  <button key={r.id} type="button" className="board-task-card"
                    data-run-id={r.id} onClick={() => onOpen(r)}>
                    <span className="board-task-top">
                      <span className={`pill ${r.status === "running" ? "on" : ""}`}>
                        {STATUS_ZH[r.status] ?? r.status}
                      </span>
                      <span className="ver">{r.mode}</span>
                    </span>
                    <strong className="board-task-name">{r.sopId}</strong>
                    <span className="board-task-meta">
                      {r.currentNode ? `节点 ${r.currentNode} · ` : ""}
                      {fmt(r.startedAt)}
                    </span>
                  </button>
                ))}
              </div>
            </div>
          );
        })}
      </div>
      <p className="sub-row">状态机只读；暂停 / 仲裁等操作在运行详情里进行。</p>
    </section>
  );
}
