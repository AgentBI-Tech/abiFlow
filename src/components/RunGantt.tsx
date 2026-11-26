import { useState } from "react";

import type { RunDetail, StepRow } from "../lib/ipc";

// 时间轴 / 甘特图（参考 ShopPilot 原型前三项之一，2026-11-26）：
// 什么时候跑、耗多久、哪几段并行。数据全部来自 RunDetail 真实字段，缺失显示「—」，不造数。
// 两种视图：顺序（按 seq 等分、色带首尾衔接，Mock 秒级完成也清晰）/ 时间（真实时间语义）。

const KIND: Record<string, string> = {
  agent_step: "agent", mcp_tool: "tool", human_gate: "human",
  acceptance: "acceptance", condition: "other", start: "other", end: "other",
};
const KIND_LABEL: Record<string, string> = {
  agent: "Agent", tool: "工具", human: "人工", acceptance: "验收", other: "其它",
};

function dur(sec: number): string {
  if (!Number.isFinite(sec) || sec < 0) return "—";
  return sec < 60 ? `${sec.toFixed(1)}s` : `${(sec / 60).toFixed(1)}min`;
}
function clock(ts: number | null): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleTimeString("zh-CN", { hour12: false });
}
function prettyJson(raw: string | null): string {
  if (!raw) return "—";
  try { return JSON.stringify(JSON.parse(raw), null, 2); } catch { return raw; }
}
function truncate(s: string, n = 2000): string {
  return s.length > n ? s.slice(0, n) + "\n…（已截断）" : s;
}

type View = "seq" | "time";

export default function RunGantt({ detail }: { detail: RunDetail }) {
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<View>("seq");
  const steps = [...detail.steps].sort((a, b) => a.seq - b.seq);
  const run = detail.run;
  const start = run.startedAt ?? Math.min(Infinity, ...steps.map((s) => s.startedAt ?? Infinity));
  const end = run.finishedAt ?? Math.floor(Date.now() / 1000);
  const base = Number.isFinite(start) ? start : end;
  const total = Math.max(1, end - base);
  const nowSec = Math.floor(Date.now() / 1000);
  const n = steps.length || 1;

  // 顺序视图：按 seq 等分轨道，色带首尾衔接；运行中步骤画满整段，pending 画浅色空槽。
  const seqPos = (s: StepRow) => {
    const left = (s.seq / n) * 100;
    const width = 100 / n;
    return { left, width };
  };
  // 时间视图：真实时间语义；0 时长给最小可见宽度，但不再挤在起点（用 seq 做下限偏移）。
  const timePos = (s: StepRow) => {
    if (!s.startedAt) return { left: 0, width: 0 };
    const left = ((s.startedAt - base) / total) * 100;
    const finish = s.finishedAt ?? nowSec;
    const width = ((finish - s.startedAt) / total) * 100;
    return {
      left: Math.max(0, Math.min(99, left)),
      // 秒级时间戳下 width 常为 0：给 1.5% 最小宽度，但不超过剩余轨道。
      width: Math.max(1.5, Math.min(100 - left, width)),
    };
  };
  const pos = view === "seq" ? seqPos : timePos;

  // 只有「已开始、未结束」的运行中步骤才脉冲；pending（startedAt 为空）一律不闪。
  const isLive = (s: StepRow) => Boolean(s.startedAt && !s.finishedAt && run.status !== "wait_human" && !["done", "failed", "cancelled"].includes(run.status));

  const longest = steps.reduce<StepRow | null>((best, s) => {
    if (!s.startedAt) return best;
    const d = (s.finishedAt ?? nowSec) - s.startedAt;
    return !best || d > ((best.finishedAt ?? nowSec) - (best.startedAt ?? nowSec)) ? s : best;
  }, null);
  let parallel = 0;
  for (let i = 1; i < steps.length; i++) {
    const a = steps[i - 1], b = steps[i];
    if (a.startedAt && b.startedAt && a.finishedAt && b.startedAt < a.finishedAt) parallel++;
  }
  const humanWait = steps
    .filter((s) => s.nodeType === "human_gate" && s.startedAt)
    .reduce((n2, s) => n2 + ((s.finishedAt ?? nowSec) - s.startedAt!), 0);

  const selStep = steps.find((s) => s.id === selected) ?? null;

  return (
    <section className="card gantt" data-testid="run-gantt">
      <div className="row">
        <h3>执行流程时间线</h3>
        <span className="ver">共 {steps.length} 步 · {clock(base)} – {clock(end)}</span>
      </div>
      <div className="gantt-toolbar">
        <div className="gantt-view-switch">
          <button className={view === "seq" ? "tab on" : "tab"} onClick={() => setView("seq")} data-view="seq">顺序</button>
          <button className={view === "time" ? "tab on" : "tab"} onClick={() => setView("time")} data-view="time">时间</button>
        </div>
        <span className="sub-row">
          {view === "seq"
            ? "按步骤顺序等分、色带首尾衔接；耗时见色带标签，点击看输入 / 输出。"
            : "横轴为真实时间、色带长度＝耗时；并排表示并行。秒级完成时色带会很短。"}
        </span>
      </div>

      <div className="gantt-scroll">
        <div className="gantt-inner">
          <div className="gantt-head">
            <div>步骤 / 执行主体</div>
            <div className="gantt-ticks">
              {view === "seq"
                ? steps.map((s) => <span key={s.id}>{s.seq}</span>)
                : [0, 0.25, 0.5, 0.75, 1].map((p) => <span key={p}>{clock(base + total * p)}</span>)}
            </div>
          </div>
          {steps.length === 0 && <p className="empty">本次 run 尚无步骤。</p>}
          {steps.map((s) => {
            const kind = KIND[s.nodeType] ?? "other";
            const p = pos(s);
            const d = s.startedAt ? (s.finishedAt ?? nowSec) - s.startedAt : 0;
            const live = isLive(s);
            const pending = !s.startedAt;
            return (
              <div key={s.id} className="gantt-row" data-step-id={s.id}>
                <div className="gantt-label">
                  <span className="gantt-seq">{s.seq}</span>
                  <span className={`gantt-badge ${kind}`}>{KIND_LABEL[kind]}</span>
                  <span className="gantt-name">{s.nodeId ?? s.nodeType}</span>
                  <span className="gantt-dur">{pending ? "待执行" : dur(d)}</span>
                </div>
                <div className="gantt-track">
                  <button type="button"
                    className={`gantt-bar ${kind} ${live ? "live" : ""} ${pending ? "pending" : ""} ${selected === s.id ? "sel" : ""}`}
                    style={{ left: `${p.left}%`, width: `${p.width}%` }}
                    data-bar-id={s.id}
                    title={`${clock(s.startedAt)} · ${pending ? "待执行" : dur(d)} · ${s.status}`}
                    onClick={() => setSelected(s.id)} />
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <div className="gantt-summary">
        <span>最长：{longest && (longest.finishedAt ?? nowSec) - (longest.startedAt ?? nowSec) > 0
          ? `${longest.nodeId ?? longest.nodeType} · ${dur((longest.finishedAt ?? nowSec) - (longest.startedAt ?? nowSec))}`
          : longest ? "均 <1s（秒级时间戳）" : "—"}</span>
        <span>并行区间：{parallel} 处</span>
        <span>人工等待占比：{total > 0 ? `${((humanWait / total) * 100).toFixed(0)}%` : "—"}</span>
      </div>
      <p className="sub-row">摘要只陈述观测，不据此自动改流程或催方向。</p>

      {selStep && (
        <div className="gantt-detail" data-selected-step={selStep.id}>
          <div className="row">
            <strong>步骤 {selStep.seq} · {selStep.nodeId ?? selStep.nodeType}</strong>
            <button className="close" onClick={() => setSelected(null)}>关闭</button>
          </div>
          <dl className="detail-dl">
            <div><dt>状态</dt><dd>{selStep.status}</dd></div>
            <div><dt>主体</dt><dd>{selStep.owner}</dd></div>
            <div><dt>工具</dt><dd>{selStep.toolRef ?? "—"}</dd></div>
            <div><dt>起止</dt><dd>{clock(selStep.startedAt)} – {clock(selStep.finishedAt)}</dd></div>
          </dl>
          <h4>输入</h4><pre>{truncate(prettyJson(selStep.inputJson))}</pre>
          <h4>输出</h4><pre>{truncate(prettyJson(selStep.outputJson))}</pre>
        </div>
      )}
    </section>
  );
}
