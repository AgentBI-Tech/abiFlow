import { useCallback, useEffect, useState } from "react";
import ToolsPanel from "./components/ToolsPanel";
import ExperiencePanel from "./components/ExperiencePanel";
import {
  dashboardGet,
  demoInstall,
  errMessage,
  gateDecide,
  runGet,
  runStart,
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
  const [tab, setTab] = useState<"board" | "tools" | "experience">("board");

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
    <main className="wrap" data-app="abi-pilot">
      <header>
        <h1>ABI-Pilot</h1>
        <p className="sub">人机协同电商运营工作台 · v0.4 经验沉淀（本地演示数据，非真实店铺）</p>
        <div className="tabs">
          <button className={tab === "board" ? "tab on" : "tab"} onClick={() => setTab("board")}>运营看板</button>
          <button className={tab === "tools" ? "tab on" : "tab"} onClick={() => setTab("tools")}>工具与权限</button>
          <button className={tab === "experience" ? "tab on" : "tab"} onClick={() => setTab("experience")}>经验沉淀</button>
        </div>
      </header>

      {err && <p className="err" data-testid="error">{err}</p>}

      {tab === "tools" ? (
        <ToolsPanel onError={setErr} />
      ) : tab === "experience" ? (
        <ExperiencePanel onError={setErr} />
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

          <section className="card">
            <h2>SOP 列表</h2>
            <ul className="list">
              {data.sops.map((s) => (
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
                    <button disabled={!!busy} onClick={() => act("启动", () => runStart(s.id))}>手动运行</button>
                    <button disabled={!!busy} onClick={() => act("启停", () => sopSetEnabled(s.id, !s.enabled))}>
                      {s.enabled ? "停用" : "启用"}
                    </button>
                  </div>
                </li>
              ))}
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
