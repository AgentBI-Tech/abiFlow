import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  LayoutDashboard, Workflow, Wrench, Puzzle, BookOpen, Sparkles, Settings2, LogIn, UserRound, LockKeyhole, ChevronDown, ChartNoAxesCombined,
} from "lucide-react";
import ToolsPanel from "./components/ToolsPanel";
import ExperiencePanel from "./components/ExperiencePanel";
import CustomToolsPanel from "./components/CustomToolsPanel";
import LlmPanel from "./components/LlmPanel";
import ConfigPanel from "./components/ConfigPanel";
import UserPanel from "./components/UserPanel";
import { applyTheme, currentTheme, themeFromBackend } from "./lib/theme";
import SopCanvas from "./components/SopCanvas";
import SopLaneCanvas from "./components/SopLaneCanvas";
import SopEditor from "./components/SopEditor";
import BoardPanel from "./components/BoardPanel";
import RunGantt from "./components/RunGantt";
import abiLogo from "./assets/abilogo.svg";
import {
  configGet,
  dashboardGet,
  errMessage,
  gateDecide,
  runGet,
  runList,
  runArbitrate,
  runControl,
  runRetry,
  runStart,
  runAsAgent,
  scheduleSet,
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
// 侧栏导航项：icon 用 lucide，key 与 tab state 一致。
const NAV_ITEMS = [
  { key: "board", label: "运营看板", icon: LayoutDashboard },
  { key: "stats", label: "统计分析", icon: ChartNoAxesCombined },
  { key: "canvas", label: "自定义 SOP", icon: Workflow },
  { key: "tools", label: "工具与权限", icon: Wrench },
  { key: "custom", label: "自定义工具", icon: Puzzle },
  { key: "experience", label: "经验沉淀", icon: BookOpen },
  { key: "llm", label: "AI / 建 SOP", icon: Sparkles },
  { key: "config", label: "配置 / MCP", icon: Settings2 },
  { key: "user", label: "用户与权限", icon: UserRound },
] as const;

// 本地日期 yyyy-mm-dd，用于运营看板的日期范围过滤（字符串比较即按时间先后）。
function dayStr(sec: number): string {
  const d = new Date(sec * 1000);
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${d.getFullYear()}-${m}-${day}`;
}
function runInRange(r: RunRow, from: string, to: string): boolean {
  if (r.startedAt == null) return false; // 未开始的 run 没有「开始日期」，不参与按日过滤
  const d = dayStr(r.startedAt);
  return (!from || d >= from) && (!to || d <= to);
}
// definition 存的是 AST（对象或 JSON 字符串），内容区只给用户看概览，不暴露技术细节。
function sopNodeCount(def: unknown): number {
  try {
    const o = typeof def === "string" ? JSON.parse(def) : def;
    return Array.isArray((o as { nodes?: unknown })?.nodes) ? (o as { nodes: unknown[] }).nodes.length : 0;
  } catch { return 0; }
}

export default function App() {
  const [data, setData] = useState<DashboardData | null>(null);
  const [detail, setDetail] = useState<RunDetail | null>(null);
  // 当前打开的 run id：事件按 payload.runId 判断是否要刷新详情（避免闭包陈旧）。
  const detailRunIdRef = useRef<string | null>(null);
  const detailRef = useRef<HTMLElement | null>(null);
  const [arbNote, setArbNote] = useState("");
  const [pathTarget, setPathTarget] = useState("");
  const [busy, setBusy] = useState("");
  const [err, setErr] = useState("");
  const [tab, setTab] = useState<"board" | "stats" | "canvas" | "tools" | "custom" | "experience" | "llm" | "config" | "user">("board");
  const [currentUser, setCurrentUser] = useState<CurrentUser | null>(null);
  const [selCanvasSop, setSelCanvasSop] = useState("");
  const [canvasMode, setCanvasMode] = useState<"read" | "edit" | "lane">("edit");

  // 运营看板：左上视图、右上按日期范围统计 + SOP 下拉选择。
  const [boardView, setBoardView] = useState<"board" | "gantt" | "lane">("board");
  const [boardRuns, setBoardRuns] = useState<RunRow[]>([]);
  const [dateFrom, setDateFrom] = useState(() => dayStr(Date.now() / 1000));
  const [dateTo, setDateTo] = useState(() => dayStr(Date.now() / 1000));
  const [selectedSopId, setSelectedSopId] = useState("");
  const [sopPickerOpen, setSopPickerOpen] = useState(false);
  const [pickerPos, setPickerPos] = useState<{ top: number; left: number } | null>(null);
  const pickerBtnRef = useRef<HTMLButtonElement | null>(null);
  const [contentFull, setContentFull] = useState(false);
  const [ganttDetail, setGanttDetail] = useState<RunDetail | null>(null);

  const refresh = useCallback(async () => {
    try {
      // 运营看板要按日期范围统计，dashboardGet 的 runs 只含最近 30 条；单独拉一批供过滤。
      const [dash, runs] = await Promise.all([dashboardGet(), runList(undefined, undefined, 500)]);
      setData(dash);
      setBoardRuns(runs);
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
      listen(name, (e) => {
        if (!alive) return;
        void refresh();
        const rid = (e.payload as { runId?: string } | undefined)?.runId;
        if (rid && rid === detailRunIdRef.current) {
          void runGet(rid).then((d) => { if (alive) setDetail(d); }).catch(() => {});
        }
      })
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

  async function openRunById(id: string) {
    try {
      const d = await runGet(id);
      setDetail(d);
      detailRunIdRef.current = d.run.id;
      setErr("");
      // 详情渲染在页面底部：打开后滚过去，否则用户点了「查看详情」却看不到甘特图。
      window.setTimeout(() => detailRef.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 60);
    } catch (e) {
      setErr(errMessage(e));
    }
  }

  async function openRun(run: RunRow) {
    await openRunById(run.id);
  }

  // 启动 run 后立即打开详情：后台线程逐步推进，事件到达会自动刷新详情，甘特图色带逐条长出。
  async function runAndOpen(start: () => Promise<{ runId: string }>) {
    setBusy("启动");
    setErr("");
    try {
      const { runId } = await start();
      await openRunById(runId);
      await refresh();
    } catch (e) {
      setErr(errMessage(e));
    } finally {
      setBusy("");
    }
  }

  // ---- 运营看板派生数据：日期范围 → SOP 列表 / 计数 / 当前视图内容 ----
  const datedRuns = boardRuns.filter((r) => runInRange(r, dateFrom, dateTo));
  const datedSopIds = new Set(datedRuns.map((r) => r.sopId));
  const pickerSops = (data?.sops ?? []).filter((s) => datedSopIds.has(s.id));
  const activeSopId = selectedSopId && pickerSops.some((s) => s.id === selectedSopId)
    ? selectedSopId : (pickerSops[0]?.id ?? "");
  const activeSop = pickerSops.find((s) => s.id === activeSopId) ?? null;
  const scopedRuns = activeSopId ? datedRuns.filter((r) => r.sopId === activeSopId) : datedRuns;
  const countDoing = scopedRuns.filter((r) => r.status === "running" || r.status === "wait_human").length;
  const countDone = scopedRuns.filter((r) => r.status === "done").length;
  const countTodo = scopedRuns.filter((r) => r.status === "pending").length;
  // 甘特/泳道视图默认展示当前 SOP 在日期范围内最新的一条 run。
  const activeGanttId = scopedRuns[0]?.id ?? "";

  useEffect(() => {
    if (!activeGanttId) { setGanttDetail(null); return; }
    let alive = true;
    void runGet(activeGanttId).then((d) => { if (alive) setGanttDetail(d); }).catch(() => { if (alive) setGanttDetail(null); });
    return () => { alive = false; };
  }, [activeGanttId]);

  return (
    <div className={`app-root${contentFull ? " content-full" : ""}`} data-app="abiflow">
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
              <button className={canvasMode === "lane" ? "tab on" : "tab"} onClick={() => setCanvasMode("lane")}>泳道</button>
            </section>
            {selCanvasSop ? (
              (() => {
                const sop = data.sops.find((s) => s.id === selCanvasSop);
                if (!sop) return <p className="empty">SOP 不存在。</p>;
                return canvasMode === "edit"
                  ? <SopEditor sop={sop} onSaved={refresh} />
                  : canvasMode === "lane"
                    ? <SopLaneCanvas sop={sop} steps={detail?.run.sopId === sop.id ? detail.steps : []} />
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
      {!data ? (
        <p>加载中…</p>
      ) : (
        <>
          {data.sops.length === 0 && (
            <section className="card"><p className="empty">还没有 SOP。</p></section>
          )}

          {tab === "board" && (
          <>
          <section className="card board-header" data-testid="board-header">
            <div className="board-header-left">
              <div className="board-view-switch" role="tablist" aria-label="看板视图">
                <button className={boardView === "board" ? "tab on" : "tab"} onClick={() => setBoardView("board")}>看板·状态机</button>
                <button className={boardView === "gantt" ? "tab on" : "tab"} onClick={() => setBoardView("gantt")}>时间·甘特图</button>
                <button className={boardView === "lane" ? "tab on" : "tab"} onClick={() => setBoardView("lane")}>泳道图</button>
              </div>
            </div>
            <div className="board-header-right">
              <div className="board-stats" data-testid="board-stats">
                <span className="stat-pill doing">正在运行 <strong>{countDoing}</strong></span>
                <span className="stat-pill done">已完成 <strong>{countDone}</strong></span>
                <span className="stat-pill todo">待办 <strong>{countTodo}</strong></span>
              </div>
              <button
                className="fullscreen-toggle"
                onClick={() => setContentFull((v) => !v)}
                title={contentFull ? "退出全屏" : "内容区全屏"}
              >{contentFull ? "退出全屏" : "全屏"}</button>
              <div className="sop-picker" data-testid="sop-picker">
                <button
                  ref={pickerBtnRef}
                  className="sop-picker-btn"
                  aria-expanded={sopPickerOpen}
                  onClick={() => {
                    if (sopPickerOpen) { setSopPickerOpen(false); return; }
                    const rect = pickerBtnRef.current?.getBoundingClientRect();
                    setPickerPos(rect
                      ? { top: rect.bottom + 8, left: Math.max(8, Math.min(rect.left, window.innerWidth - 428)) }
                      : { top: 120, left: 300 });
                    setSopPickerOpen(true);
                  }}
                >
                  {activeSop ? `${activeSop.name} · v${activeSop.version}` : "选择 SOP"}<ChevronDown size={14} />
                </button>
                {sopPickerOpen && <div className="sop-picker-away" onClick={() => setSopPickerOpen(false)} />}
                {sopPickerOpen && <div className="sop-picker-menu" data-sop-menu="1" style={{ top: pickerPos?.top, left: pickerPos?.left }}>
                  <div className="sop-picker-filters">
                    <label>起<input type="date" value={dateFrom} onChange={(e) => setDateFrom(e.target.value)} /></label>
                    <label>止<input type="date" value={dateTo} onChange={(e) => setDateTo(e.target.value)} /></label>
                    <button disabled={!!busy} onClick={() => act("刷新", refresh)}>刷新</button>
                  </div>
                  {pickerSops.length === 0 ? (
                    <p className="empty">该日期范围内没有运行记录，换个日期试试。</p>
                  ) : (
                    <ul className="list sop-picker-list">
                      {pickerSops.map((s) => {
                        const sched = data.schedules.find((x) => x.sopId === s.id);
                        return (
                          <li key={s.id} className={`sop-picker-item${s.id === activeSopId ? " on" : ""}`} data-sop-id={s.id}>
                            <button
                              className="sop-picker-select"
                              onClick={() => { setSelectedSopId(s.id); setSopPickerOpen(false); }}
                            >
                              <strong>{s.name}</strong> <span className="ver">v{s.version}</span>
                            </button>
                            <div className="row actions sop-picker-actions">
                              <button disabled={!!busy} onClick={() => runAndOpen(() => runStart(s.id))}>手动运行</button>
                              <button disabled={!!busy} onClick={() => runAndOpen(() => runAsAgent(s.id))}>以 agent 跑</button>
                              <button disabled={!!busy} onClick={() => act("启停", () => sopSetEnabled(s.id, !s.enabled))}>
                                {s.enabled ? "停用" : "启用"}
                              </button>
                              {sched ? (
                                <button disabled={!!busy} onClick={() => act("停自动", () => scheduleSet(s.id, false, 0, null))}>停自动</button>
                              ) : (
                                <select
                                  disabled={!!busy}
                                  value="0"
                                  onChange={(e) => { if (e.target.value !== "0") void act("设自动", () => scheduleSet(s.id, true, Number(e.target.value), null)); }}
                                >
                                  <option value="0">设自动</option>
                                  <option value="600">10 分钟</option>
                                  <option value="1800">30 分钟</option>
                                  <option value="3600">1 小时</option>
                                  <option value="86400">1 天</option>
                                </select>
                              )}
                            </div>
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>}
              </div>
            </div>
          </section>

          <section className="card sop-content" data-testid="sop-content">
            {activeSop ? (
              <>
                <h2>{activeSop.name} <span className="ver">v{activeSop.version}</span></h2>
                <p className="sop-definition">
                  {sopNodeCount(activeSop.definition)} 个节点 · 风险 {activeSop.risk} · 归属 {activeSop.ownerPolicy}
                </p>
                <div className="row sub-row">
                  <span>统计范围：{dateFrom || "不限"} ~ {dateTo || "不限"} · {activeSop.name}</span>
                </div>
              </>
            ) : (
              <p className="empty">该日期范围内暂无可展示的 SOP，试试放宽日期或从右上角选择。</p>
            )}
          </section>

          {boardView === "board" && (
            <BoardPanel runs={scopedRuns} onOpen={openRun} activeStatus={detail?.run.status ?? null} />
          )}
          {boardView === "gantt" && (
            <section className="card" data-testid="board-gantt">
              {scopedRuns.length === 0 ? (
                <p className="empty">该日期范围内没有运行记录。</p>
              ) : (
                <>
                  {ganttDetail && ganttDetail.run.id === activeGanttId
                    ? <RunGantt detail={ganttDetail} />
                    : <p className="empty">加载运行详情…</p>}
                </>
              )}
            </section>
          )}
          {boardView === "lane" && (
            <section className="card" data-testid="board-lane">
              {activeSop
                ? <SopLaneCanvas sop={activeSop} steps={ganttDetail && ganttDetail.run.sopId === activeSop.id ? ganttDetail.steps : []} />
                : <p className="empty">先选一条 SOP。</p>}
            </section>
          )}
          </>
          )}

          {tab === "stats" && (
          <>
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
            <details className="stats-fold">
              <summary>统计明细（按 SOP / 按风险，点击展开）</summary>
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
            </details>
          </section>
          </>
          )}

          {detail && (
            <>
            <div className="drawer-scrim" onClick={() => { setDetail(null); detailRunIdRef.current = null; }} data-drawer-scrim="1" />
            <aside className="run-drawer" data-testid="run-detail" ref={detailRef}>
              <div className="row drawer-head">
                <h2>运行详情</h2>
                <button className="close" onClick={() => { setDetail(null); detailRunIdRef.current = null; }}>关闭 ✕</button>
              </div>
              <p className="sub-row">{detail.run.id} · {STATUS_ZH[detail.run.status] ?? detail.run.status}</p>
              <RunGantt detail={detail} />
              <details className="steps-fold">
                <summary>步骤明细（{detail.steps.length}）</summary>
                <ol className="steps">
                  {detail.steps.map((s) => (
                    <li key={s.id} data-testid="step-item">
                      <strong>{s.nodeType}</strong> — {s.status}
                      {s.toolRef && <span className="ver"> · {s.toolRef}</span>}
                      {s.outputJson && <pre>{s.outputJson}</pre>}
                    </li>
                  ))}
                </ol>
              </details>
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
              {/* 待裁决人工门：在详情里直接批准/驳回，不必跳到「人工待办」翻找。 */}
              {(() => {
                const pending = detail.gates.filter((g) => g.decidedAt === null);
                if (!pending.length) return null;
                const reasonZh: Record<string, string> = {
                  anomaly_review: "异常清单审核", pause: "暂停确认", takeover: "人工接管确认",
                  acceptance: "验收确认",
                };
                return (
                  <div className="stack" data-testid="gate-approve-box">
                    <h3>待你处理 · {pending.length} 个人工门</h3>
                    <p className="sub-row">危险操作（退款 / 补发 / 补偿）必须在这里由人确认；批准或驳回后，流程才会继续。</p>
                    {pending.map((g) => (
                      <div className="gate-card" key={g.id} data-gate-id={g.id}>
                        <div className="row">
                          <strong>{reasonZh[g.reason] ?? g.reason}</strong>
                          <span className="ver">步骤 {g.stepId}</span>
                        </div>
                        <p className="sub-row">{g.prompt}</p>
                        <div className="row actions">
                          <button disabled={!!busy} data-gate-action="approve"
                            onClick={() => act("批准", async () => {
                              await gateDecide(g.id, "approve", arbNote || undefined);
                              setDetail(await runGet(detail.run.id));
                            })}>批准通过</button>
                          <button disabled={!!busy} data-gate-action="reject"
                            onClick={() => act("驳回", async () => {
                              await gateDecide(g.id, "reject", arbNote || undefined);
                              setDetail(await runGet(detail.run.id));
                            })}>驳回</button>
                        </div>
                      </div>
                    ))}
                    <label className="stack">
                      批准 / 驳回说明（可选，留痕）
                      <input value={arbNote} onChange={(e) => setArbNote(e.target.value)} placeholder="写明理由" />
                    </label>
                  </div>
                );
              })()}
              {!["done", "failed", "cancelled"].includes(detail.run.status) && (
                <div className="stack" data-testid="control-box">
                  <h3>人工干预（保留上下文）</h3>
                  <p className="sub-row">接管把当前步交给人工；改路径只改本次 run 的剩余节点。暂停 / 恢复只放行「暂停门」，不绕过真实人工门——真实人工门请在上方批准。</p>
                  <label className="stack">
                    改路径目标节点 id（当前步骤见上方步骤列表）
                    <input value={pathTarget} onChange={(e) => setPathTarget(e.target.value)} placeholder="如 n3" />
                  </label>
                  <div className="row actions">
                    {["running", "pending"].includes(detail.run.status) && (
                      <button disabled={!!busy} onClick={() => act("暂停", async () => { await runControl(detail.run.id, "pause"); setDetail(await runGet(detail.run.id)); })}>暂停</button>
                    )}
                    {detail.run.status === "wait_human" &&
                      (() => {
                        const pending = detail.gates.filter((g) => g.decidedAt === null);
                        const onlyPause = pending.length === 0 || pending.every((g) => g.reason === "pause");
                        return onlyPause ? (
                          <button disabled={!!busy} onClick={() => act("恢复", async () => { await runControl(detail.run.id, "resume", undefined, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>恢复（暂停门）</button>
                        ) : (
                          <span className="sub-row">有真实人工门待批准，「恢复」不可用——请在上方批准。</span>
                        );
                      })()}
                    <button disabled={!!busy} onClick={() => act("接管", async () => { await runControl(detail.run.id, "takeover", undefined, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>接管当前步</button>
                    <button disabled={!!busy} onClick={() => act("改路径", async () => { await runControl(detail.run.id, "change_path", pathTarget, arbNote || undefined); setDetail(await runGet(detail.run.id)); })}>改路径</button>
                  </div>
                </div>
              )}
            </aside>
            </>
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
