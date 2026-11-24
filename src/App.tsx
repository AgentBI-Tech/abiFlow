import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  LayoutDashboard, Workflow, Wrench, Puzzle, BookOpen, Sparkles, Settings2, LogIn, UserRound, LockKeyhole,
} from "lucide-react";
import ToolsPanel from "./components/ToolsPanel";
import ExperiencePanel from "./components/ExperiencePanel";
import CustomToolsPanel from "./components/CustomToolsPanel";
import LlmPanel from "./components/LlmPanel";
import ConfigPanel from "./components/ConfigPanel";
import UserPanel from "./components/UserPanel";
import { applyTheme, currentTheme, themeFromBackend } from "./lib/theme";
import SopCanvas from "./components/SopCanvas";
import SopEditor from "./components/SopEditor";
import abiLogo from "./assets/abilogo.svg";
import {
  configGet,
  dashboardGet,
  demoInstall,
  errMessage,
  gateDecide,
  runGet,
  runArbitrate,
  runControl,
  runRetry,
  runStart,
  runAsAgent,
  scheduleSet,
  schedulerTick,
  sopSetEnabled,
  type DashboardData,
  type RunDetail,
  type RunRow,
  type CurrentUser,
} from "./lib/ipc";

const STATUS_ZH: Record<string, string> = {
  pending: "待运行", running: "运行中", wait_human: "待人工", rejected: "已打回",
  review: "待仲裁", done: "完成", failed: "失败", cancelled: "已取消",
};
const VERDICT_ZH: Record<string, string> = { pass: "通过", fail: "未通过", partial: "部分" };

// 侧栏导航项：icon 用 lucide，key 与 tab state 一致。
const NAV_ITEMS = [
  { key: "board", label: "运营看板", icon: LayoutDashboard },
  { key: "canvas", label: "SOP 画布", icon: Workflow },
  { key: "tools", label: "工具与权限", icon: Wrench },
  { key: "custom", label: "自定义工具", icon: Puzzle },
  { key: "experience", label: "经验沉淀", icon: BookOpen },
  { key: "llm", label: "AI / 建 SOP", icon: Sparkles },
  { key: "config", label: "配置 / MCP", icon: Settings2 },
  { key: "user", label: "用户与权限", icon: UserRound },
] as const;

function fmtTime(ts: number | null): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false });
}

export default function App() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [arbNote, setArbNote] = useState("");
  const [pathTarget, setPathTarget] = useState("");
  const [busy, setBusy] = useState("");
  const [err, setErr] = useState("");
  const [tab, setTab] = useState<"board" | "canvas" | "tools" | "custom" | "experience" | "llm" | "config" | "user">("board");
  const [currentUser, setCurrentUser] = useState<CurrentUser | null>(null);
  const [selCanvasSop, setSelCanvasSop] = useState("");
  const [canvasMode, setCanvasMode] = useState<"read" | "edit">("edit");
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

  // 后端 config 的 theme 是跨重启/机器权威：挂载后用它校正首屏的 localStorage 值。
  // 后端无值则保留首屏选择（首次启动时由 saveTheme 写入后端）。
  useEffect(() => {
    void configGet().then((cfg) => {
      const t = themeFromBackend(cfg);
      if (t && t !== currentTheme()) {
        applyTheme(t);
        localStorage.setItem("abiflow.theme", t);
      }
    }).catch(() => { /* 非 Tauri 环境无 config，保留首屏主题 */ });
  }, []);

  // 后端状态变更事件只当「该刷新了」的提示；真相仍以 dashboardGet / run_get 拉取为准。
  useEffect(() => {
    let alive = true;
    const events = ["run:status", "step:update", "gate:requested", "review:result", "tool:audit"];
    let cancels: Array<() => void> = [];
    Promise.all(events.map((name) =>
      listen(name, () => { if (alive) void refresh(); })
    )).then((fns) => {
      if (!alive) { fns.forEach((f) => f()); return; }
      cancels = fns;
    }).catch(() => { /* 非 Tauri 环境（如浏览器预览）无事件，忽略 */ });
    return () => { alive = false; cancels.forEach((f) => f()); };
  }, [refresh]);

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
    <div className="app-root" data-app="abiflow">
      {/* 顶部导航 bar：logo + 产品名 + 全局状态 */}
      <header className="app-topbar">
        <div className="app-brand">
          <span className="app-brand-mark" aria-hidden="true"><img src={abiLogo} alt="" /></span>
          <span className="app-brand-name">abiFlow</span>
          <span className="app-brand-sub">人机协同电商运营工作台</span>
        </div>
        <div className="app-topbar-right">
          <span className="app-topbar-tag">工具即数据</span>
          <span className="app-topbar-tag">agentbi.tech</span>
        </div>
      </header>

      <div className="app-shell">
        {/* 左侧功能区（固定宽度），底部用户占位 */}
        <aside className="app-sidebar">
          <nav className="app-nav" aria-label="功能区">
            {NAV_ITEMS.map((item) => {
              const Icon = item.icon;
              return (
                <button
                  key={item.key}
                  className={tab === item.key ? "app-nav-item on" : "app-nav-item"}
                  onClick={() => setTab(item.key)}
                >
                  <Icon size={16} strokeWidth={1.8} />
                  <span>{item.label}</span>
                </button>
              );
            })}
          </nav>
          <div className="app-user">
            <span className="app-user-avatar" aria-hidden="true">👤</span>
            <span className="app-user-label" data-user-label="1" title={currentUser ? `${currentUser.displayName}（@${currentUser.username}）· ${currentUser.role}` : "未登录"}>
              {currentUser ? `${currentUser.displayName} · ${currentUser.role}` : "未登录"}
            </span>
            <button className="app-user-login" onClick={() => setTab("user")} title="用户与权限">
              {currentUser ? <UserRound size={13} /> : <LogIn size={13} />}
              {currentUser ? "管理" : "登录"}
            </button>
          </div>
        </aside>

        {/* 右侧浅色内容区（随窗口缩放） */}
        <main className="app-content">
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
              <button className={canvasMode === "edit" ? "tab on" : "tab"} onClick={() => setCanvasMode("edit")}>编辑</button>
              <button className={canvasMode === "read" ? "tab on" : "tab"} onClick={() => setCanvasMode("read")}>只读</button>
            </section>
            {selCanvasSop ? (
              (() => {
                const sop = data.sops.find((s) => s.id === selCanvasSop);
                if (!sop) return <p className="empty">SOP 不存在。</p>;
                return canvasMode === "edit"
                  ? <SopEditor sop={sop} onSaved={refresh} />
                  : <SopCanvas sop={sop} />;
              })()
            ) : (
              <p className="empty">先选一条 SOP。还没有 SOP 的话，去「运营看板」点「安装演示 SOP」。</p>
            )}
          </>
        )
      ) : tab === "tools" ? (
        <ToolsPanel onError={setErr} role={currentUser?.role ?? "agent"} />
      ) : tab === "user" ? (
        <UserPanel onError={setErr} onUserChange={setCurrentUser} />
      ) : tab === "custom" ? (
        <CustomToolsPanel onError={setErr} />
      ) : tab === "experience" ? (
        <ExperiencePanel onError={setErr} />
      ) : tab === "llm" ? (
        <LlmPanel onError={setErr} />
      ) : tab === "config" ? (
        <ConfigPanel onError={setErr} />
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
                      {r.rejectCount > 0 && <span className="pill off" data-testid="reject-count">打回 {r.rejectCount}</span>}
                    </div>
                    <div className="row sub-row">
                      <span>开始 {fmtTime(r.startedAt)}</span>
                      <span>结束 {fmtTime(r.finishedAt)}</span>
                      <span>{r.sopId}</span>
                    </div>
                    <div className="row actions">
                      <button disabled={!!busy} onClick={() => openRun(r)}>查看详情</button>
                      {r.status === "rejected" && (
                        <button disabled={!!busy} onClick={() => act("重试", () => runRetry(r.id))}>重试（第 2 轮）</button>
                      )}
                      {r.status === "review" && (
                        <>
                          <button disabled={!!busy} onClick={() => act("仲裁放行", () => runArbitrate(r.id, "resume"))}>仲裁放行</button>
                          <button disabled={!!busy} onClick={() => act("仲裁失败", () => runArbitrate(r.id, "fail"))}>仲裁失败</button>
                        </>
                      )}
                    </div>
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
              {(detail.run.status === "rejected" || detail.run.status === "review") && (
                <div className="stack" data-testid="arbitration-box">
                  <label className="stack">
                    仲裁/重试说明（可选，留痕）
                    <input value={arbNote} onChange={(e) => setArbNote(e.target.value)} placeholder="写明理由" />
                  </label>
                  <div className="row actions">
                    {detail.run.status === "rejected" && (
                      <button disabled={!!busy} onClick={() => act("重试", async () => { await runRetry(detail.run.id); setDetail(await runGet(detail.run.id)); })}>
                        重试（第 2 轮）
                      </button>
                    )}
                    {detail.run.status === "review" && (
                      <>
                        <button disabled={!!busy} onClick={() => act("仲裁放行", async () => { await runArbitrate(detail.run.id, "resume", arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>
                          仲裁放行
                        </button>
                        <button disabled={!!busy} onClick={() => act("仲裁失败", async () => { await runArbitrate(detail.run.id, "fail", arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>
                          仲裁失败
                        </button>
                      </>
                    )}
                  </div>
                </div>
              )}
              {!["done", "failed", "cancelled"].includes(detail.run.status) && (
                <div className="stack" data-testid="control-box">
                  <h3>人工干预（保留上下文）</h3>
                  <p className="sub-row">暂停/恢复只放行「暂停门」，不绕过真实人工门；接管把当前步交给人工；改路径只改本次 run 的剩余节点。</p>
                  <label className="stack">
                    改路径目标节点 id（当前步骤见上方步骤列表）
                    <input value={pathTarget} onChange={(e) => setPathTarget(e.target.value)} placeholder="如 n3" />
                  </label>
                  <div className="row actions">
                    {["running", "pending"].includes(detail.run.status) && (
                      <button disabled={!!busy} onClick={() => act("暂停", async () => { await runControl(detail.run.id, "pause"); setDetail(await runGet(detail.run.id)); })}>暂停</button>
                    )}
                    {detail.run.status === "wait_human" && (
                      <button disabled={!!busy} onClick={() => act("恢复", async () => { await runControl(detail.run.id, "resume", undefined, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>恢复</button>
                    )}
                    <button disabled={!!busy} onClick={() => act("接管", async () => { await runControl(detail.run.id, "takeover", undefined, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>接管当前步</button>
                    <button disabled={!!busy} onClick={() => act("改路径", async () => { await runControl(detail.run.id, "change_path", pathTarget, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>改路径</button>
                  </div>
                </div>
              )}
            </section>
          )}
        </>
      )}
      </>
      )}

      {/* 未登录锁屏：内容区半透明遮罩，提示先登录（用户与权限 tab 不遮，否则没法登录/建号）。 */}
      {!currentUser && tab !== "user" && (
        <div className="app-lock" data-lock="1">
          <div className="app-lock-card">
            <LockKeyhole size={30} strokeWidth={1.6} />
            <div className="lock-title">请先登录</div>
            <div className="lock-sub">登录后可查看内容、按角色使用工具与运行 SOP。</div>
            <button onClick={() => setTab("user")}>去登录 / 管理用户</button>
          </div>
        </div>
      )}
        </main>
      </div>
    </div>
  );
}
