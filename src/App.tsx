import { useCallback, useEffect, useState } from "react";
import ToolsPanel from "./components/ToolsPanel";
import ExperiencePanel from "./components/ExperiencePanel";
import CustomToolsPanel from "./components/CustomToolsPanel";
import LlmPanel from "./components/LlmPanel";
import SopCanvas from "./components/SopCanvas";
import {
  dashboardGet,
  demoInstall,
  errMessage,
  gateDecide,
  runGet,
  runStart,
  runAsAgent,
  scheduleSet,
  schedulerTick,
  sopSetEnabled,
  type DashboardData,
  type RunDetail,
  type RunRow,
} from "./lib/ipc";

const STATUS_ZH: Record<string, string> = {
  pending: "待运行", running: "运行中", wait_human: "待人工", rejected: "已驳回",
  done: "完成", failed: "失败", cancelled: "已取消",
};
const VERDICT_ZH: Record<string, string> = { pass: "通过", fail: "未通过", partial: "部分" };

function fmtTime(ts: number | null): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

export default function App() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [busy, setBusy] = useState("");
  const [err, setErr] = useState("");
  const [tab, setTab] = useState<"board" | "canvas" | "tools" | "custom" | "experience" | "llm">("board");
  const [selCanvasSop, setSelCanvasSop] = useState("");
  const [cronDraft, setCronDraft] = useState<Record<string, string>>({});

  const refresh = useCallback(async () => {
    try {
      setData(await dashboardGet());
      setErr("");
    } catch (e) {
      setErr(errMessage(e));
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  async function act(label: string, fn: () => Promise<unknown>) {
    setBusy(label);
    setErr("");
    try {
      await fn();
      await refresh();
    } catch (e) {
      setErr(errMessage(e));
    } finally {
      setBusy("");
    }
  }

  async function openRun(run: RunRow) {
    try {
      setDetail(await runGet(run.id));
      setErr("");
    } catch (e) {
      setErr(errMessage(e));
    }
  }

  return (
    <main className="wrap" data-app="abiflow">
      <header>
        <h1>abiFlow</h1>
        <p className="sub">人机协同电商运营工作台 · 通用工具（工具即数据，agentbi.tech 为首个示例）</p>
        <div className="tabs">
          <button className={tab === "board" ? "tab on" : "tab"} onClick={() => setTab("board")}>运营看板</button>
          <button className={tab === "canvas" ? "tab on" : "tab"} onClick={() => setTab("canvas")}>SOP 画布</button>
          <button className={tab === "tools" ? "tab on" : "tab"} onClick={() => setTab("tools")}>工具与权限</button>
          <button className={tab === "custom" ? "tab on" : "tab"} onClick={() => setTab("custom")}>自定义工具</button>
          <button className={tab === "experience" ? "tab on" : "tab"} onClick={() => setTab("experience")}>经验沉淀</button>
          <button className={tab === "llm" ? "tab on" : "tab"} onClick={() => setTab("llm")}>AI / 建 SOP</button>
        </div>
      </header>

      {err && <p className="err" data-testid="error">{err}</p>}

      {tab === "canvas" ? (
        !data ? <p>加载中…</p> : (
          <>
            <section className="actions">
              <select value={selCanvasSop} onChange={(e) => setSelCanvasSop(e.target.value)}>
                <option value="">选一条 SOP 查看画布</option>
                {data.sops.map((s) => (
                  <option key={s.id} value={s.id}>{s.name} · v{s.version}</option>
                ))}
              </select>
            </section>
            {selCanvasSop ? (
              (() => {
                const sop = data.sops.find((s) => s.id === selCanvasSop);
                return sop ? <SopCanvas sop={sop} /> : <p className="empty">SOP 不存在。</p>;
              })()
            ) : (
              <p className="empty">先选一条 SOP。还没有 SOP 的话，去「运营看板」点「安装演示 SOP」。</p>
            )}
          </>
        )
      ) : tab === "tools" ? (
        <ToolsPanel onError={setErr} />
      ) : tab === "custom" ? (
        <CustomToolsPanel onError={setErr} />
      ) : tab === "experience" ? (
        <ExperiencePanel onError={setErr} />
      ) : tab === "llm" ? (
        <LlmPanel onError={setErr} />
      ) : (
      <>
      <section className="actions">
        <button disabled={!!busy} onClick={() => act("安装演示", () => demoInstall())}>
          安装演示 SOP
        </button>
        <button disabled={!!busy} onClick={() => act("手动调度", () => schedulerTick())}>
          手动调度一次
        </button>
        <button disabled={!!busy} onClick={() => act("刷新", refresh)}>刷新</button>
      </section>

      {!data ? (
        <p>加载中…</p>
      ) : (
        <>
          {data.sops.length === 0 && (
            <section className="card"><p className="empty">还没有 SOP，点「安装演示 SOP」开始。</p></section>
          )}

          <section className="card" data-testid="board-summary">
            <h2>完成度看板</h2>
            <div className="row board-periods">
              {[data.board.day, data.board.week].map((p) => (
                <div key={p.label} className="board-period">
                  <strong>{p.label}</strong>
                  <span>共 {p.total}</span>
                  <span>完成 {p.done}</span>
                  <span>进行中 {p.running}</span>
                  <span>等人工 {p.waitHuman}</span>
                  <span>打回 {p.rejected}</span>
                  <span>失败 {p.failed}</span>
                  <span>取消 {p.cancelled}</span>
                </div>
              ))}
            </div>
            <h3>按 SOP 聚合（累计）</h3>
            <table className="board-table">
              <thead><tr><th>SOP</th><th>总</th><th>完成</th><th>等人工</th><th>失败</th></tr></thead>
              <tbody>
                {data.board.bySop.map((g) => (
                  <tr key={g.key} data-board-sop={g.key}>
                    <td>{g.label}</td><td>{g.total}</td><td>{g.done}</td><td>{g.waitHuman}</td><td>{g.failed}</td>
                  </tr>
                ))}
                {data.board.bySop.length === 0 && <tr><td colSpan={5} className="empty">暂无运行记录</td></tr>}
              </tbody>
            </table>
            <h3>按风险分类聚合（累计，risk 维度）</h3>
            <table className="board-table">
              <thead><tr><th>风险</th><th>总</th><th>完成</th><th>等人工</th><th>失败</th></tr></thead>
              <tbody>
                {data.board.byRisk.map((g) => (
                  <tr key={g.key} data-board-risk={g.key}>
                    <td>{g.key}</td><td>{g.total}</td><td>{g.done}</td><td>{g.waitHuman}</td><td>{g.failed}</td>
                  </tr>
                ))}
                {data.board.byRisk.length === 0 && <tr><td colSpan={5} className="empty">暂无运行记录</td></tr>}
              </tbody>
            </table>
            <p className="sub-row" style={{ marginTop: 10 }}>{data.board.scopeNote}</p>
          </section>

          <section className="card">
            <h2>SOP 列表</h2>
            <ul className="list">
              {data.sops.map((s) => {
                const sched = data.schedules.find((x) => x.sopId === s.id);
                return (
                <li key={s.id} data-testid="sop-item">
                  <div className="row">
                    <strong>{s.name}</strong>
                    <span className={`pill ${s.enabled ? "on" : "off"}`}>{s.enabled ? "启用" : "停用"}</span>
                    <span className="ver">v{s.version}</span>
                  </div>
                  <div className="row sub-row">
                    <span>风险：{s.risk}</span>
                    <span>归属：{s.ownerPolicy}</span>
                    <span>{s.id}</span>
                  </div>
                  <div className="row actions">
                    <button disabled={!!busy} onClick={() => act("手动", () => runStart(s.id))}>手动运行</button>
                    <button disabled={!!busy} onClick={() => act("agent", () => runAsAgent(s.id))}>以 agent 跑一次</button>
                    <button disabled={!!busy} onClick={() => act("启停", () => sopSetEnabled(s.id, !s.enabled))}>
                      {s.enabled ? "停用" : "启用"}
                    </button>
                  </div>
                  <div className="row actions sched">
                    <span className={`pill ${sched ? "on" : "off"}`}>{sched ? "自动运行中" : "未设自动"}</span>
                    {sched?.cronExpr ? (
                      <span className="pill on" data-cron-expr={sched.cronExpr}>cron {sched.cronExpr}</span>
                    ) : null}
                  </div>
                  <div className="row actions sched">
                    <label>间隔
                      <select
                        value={sched && !sched.cronExpr ? String(sched.intervalSecs) : "0"}
                        disabled={!!busy}
                        onChange={(e) => act("设置间隔", () => scheduleSet(s.id, true, Number(e.target.value), null))}
                      >
                        <option value="0" disabled>选择间隔</option>
                        <option value="600">10 分钟</option>
                        <option value="1800">30 分钟</option>
                        <option value="3600">1 小时</option>
                        <option value="86400">1 天</option>
                      </select>
                    </label>
                    <label>cron（分 时 日 月 周，如 0 9 * * *）
                      <input
                        className="cron-input"
                        placeholder="0 9 * * *"
                        value={cronDraft[s.id] ?? sched?.cronExpr ?? ""}
                        disabled={!!busy}
                        onChange={(e) => setCronDraft((d) => ({ ...d, [s.id]: e.target.value }))}
                      />
                    </label>
                    <button
                      disabled={!!busy || !(cronDraft[s.id] ?? sched?.cronExpr ?? "").trim()}
                      onClick={() => act("设置 cron", () => scheduleSet(s.id, true, 0, (cronDraft[s.id] ?? "").trim()))}
                    >用 cron</button>
                    {sched ? (
                      <button disabled={!!busy} onClick={() => act("停调度", () => scheduleSet(s.id, false, 0, null))}>停自动</button>
                    ) : null}
                  </div>
                  {sched ? (
                    <div className="row sub-row"><span>下次 {fmtTime(sched.nextRunAt)} · {sched.cronExpr ? "按本地时区对齐" : "固定间隔"}</span></div>
                  ) : null}
                </li>
                );
              })}
            </ul>
          </section>

          <section className="card">
            <h2>人工待办 <span className="count">{data.pendingGates.length}</span></h2>
            {data.pendingGates.length === 0 ? (
              <p className="empty">暂无待人工处理的门。</p>
            ) : (
              <ul className="list">
                {data.pendingGates.map((g) => (
                  <li key={g.id} data-testid="gate-item">
                    <p className="gate-prompt">{g.prompt}</p>
                    <p className="sub-row">原因：{g.reason} · run {g.runId}</p>
                    <div className="row actions">
                      <button disabled={!!busy} onClick={() => act("批准", () => gateDecide(g.id, "approve"))}>批准</button>
                      <button disabled={!!busy} onClick={() => act("驳回", () => gateDecide(g.id, "reject"))}>驳回</button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          <section className="card">
            <h2>运行历史</h2>
            {data.runs.length === 0 ? (
              <p className="empty">还没有运行记录。</p>
            ) : (
              <ul className="list">
                {data.runs.map((r) => (
                  <li key={r.id} data-testid="run-item">
                    <div className="row">
                      <strong>{STATUS_ZH[r.status] ?? r.status}</strong>
                      <span className="pill">{VERDICT_ZH[r.verdict ?? ""] ?? "—"}</span>
                      <span className="ver">{r.mode} · {r.triggerBy}</span>
                    </div>
                    <div className="row sub-row">
                      <span>开始 {fmtTime(r.startedAt)}</span>
                      <span>结束 {fmtTime(r.finishedAt)}</span>
                      <span>{r.sopId}</span>
                    </div>
                    <div className="row actions"><button disabled={!!busy} onClick={() => openRun(r)}>查看详情</button></div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {detail && (
            <section className="card" data-testid="run-detail">
              <div className="row">
                <h2>运行详情</h2>
                <button className="close" onClick={() => setDetail(null)}>关闭</button>
              </div>
              <p className="sub-row">{detail.run.id} · {STATUS_ZH[detail.run.status] ?? detail.run.status}</p>
              <h3>步骤</h3>
              <ol className="steps">
                {detail.steps.map((s) => (
                  <li key={s.id} data-testid="step-item">
                    <strong>{s.nodeType}</strong> — {s.status}
                    {s.toolRef && <span className="ver"> · {s.toolRef}</span>}
                    {s.outputJson && <pre>{s.outputJson}</pre>}
                  </li>
                ))}
              </ol>
              <h3>验收</h3>
              <ul className="list">
                {detail.acceptance.map((a) => (
                  <li key={a.id}>{a.passed ? "✓" : "✗"} {a.criterion}</li>
                ))}
                {detail.acceptance.length === 0 && <li className="empty">无验收项</li>}
              </ul>
            </section>
          )}
        </>
      )}
      </>
      )}
    </main>
  );
}
