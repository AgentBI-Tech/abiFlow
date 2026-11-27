import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
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
  { key: "stats", label: "历史分析", icon: ChartNoAxesCombined },
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
  // 详情统一由内容区甘特图承载，不再用抽屉；事件按 runId 刷新当前展示的 run。
  const activeRunIdRef = useRef<string | null>(null);
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
  // 手动指定的 run（手动运行 / 点任务 / pill 下钻）；为空则展示日期范围内最新一条。
  const [targetRunId, setTargetRunId] = useState("");
  // 右上 pill 下钻："" 未展开 / all / done / failed；浮层坐标由点击的 pill 决定。
  const [pillFilter, setPillFilter] = useState<"" | "all" | "done" | "failed">("");
  const [pillPos, setPillPos] = useState<{ top: number; left: number } | null>(null);
  // 历史分析：子 tab 与查询条件。
  const [historySub, setHistorySub] = useState<"query" | "stats">("query");
  const [historyFrom, setHistoryFrom] = useState(() => dayStr(Date.now() / 1000 - 30 * 86400));
  const [historyTo, setHistoryTo] = useState(() => dayStr(Date.now() / 1000));
  const [historySop, setHistorySop] = useState("");
  const [historyStatus, setHistoryStatus] = useState("");

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
        if (rid && rid === activeRunIdRef.current) {
          void runGet(rid).then((d) => { if (alive) setGanttDetail(d); }).catch(() => {});
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

  // 不弹抽屉：直接切到运营看板的甘特图视图，并定位到指定 run。
  function focusRun(id: string) {
    setTab("board");
    setBoardView("gantt");
    setTargetRunId(id);
    setSopPickerOpen(false);
    setPillFilter("");
    setErr("");
    window.setTimeout(() => document.querySelector(".app-content")?.scrollTo({ top: 0, behavior: "smooth" }), 60);
  }

  function openRun(run: RunRow) {
    focusRun(run.id);
  }

  // pill 下钻浮层：位置跟随被点的 pill，复用 SOP 下拉的浮层样式。
  function openPill(e: MouseEvent<HTMLButtonElement>, filter: "all" | "done" | "failed") {
    if (pillFilter === filter) { setPillFilter(""); return; }
    const r = e.currentTarget.getBoundingClientRect();
    setPillPos({ top: r.bottom + 8, left: Math.max(8, Math.min(r.left, window.innerWidth - 428)) });
    setPillFilter(filter);
  }

  // 启动 run 后直接切到甘特图：后台线程逐步推进，事件到达会自动刷新详情，色带逐条长出。
  async function runAndOpen(start: () => Promise<{ runId: string }>) {
    setBusy("启动");
    setErr("");
    try {
      const { runId } = await start();
      focusRun(runId);
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
  // 右上按「运行次数」计数，避开与下方条目列的 待办/进行中/已完成 重名。
  const countRuns = scopedRuns.length;
  const countDone = scopedRuns.filter((r) => r.status === "done").length;
  const countFailed = scopedRuns.filter((r) => r.status === "failed" || r.status === "cancelled").length;
  const pillRuns = scopedRuns.filter((r) =>
    pillFilter === "all" ? true :
    pillFilter === "done" ? r.status === "done" :
    pillFilter === "failed" ? (r.status === "failed" || r.status === "cancelled") : false);
  const historyRuns = boardRuns.filter((r) =>
    runInRange(r, historyFrom, historyTo) &&
    (!historySop || r.sopId === historySop) &&
    (!historyStatus || r.status === historyStatus));
  // 甘特/泳道视图：优先展示手动指定的 run，否则展示当前 SOP 在日期范围内最新的一条。
  const activeGanttId = targetRunId || (scopedRuns[0]?.id ?? "");

  useEffect(() => {
    activeRunIdRef.current = activeGanttId;
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
                    ? <SopLaneCanvas sop={sop} steps={ganttDetail?.run.sopId === sop.id ? ganttDetail.steps : []} />
                    : <SopCanvas sop={sop} />;
              })()
            ) : (
              <p className="empty">先选一条 SOP。</p>
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
                <span className="board-stats-label">运行次数（点击查看任务）</span>
                <button className={`stat-pill doing${pillFilter === "all" ? " on" : ""}`}
                  onClick={(e) => openPill(e, "all")}>运行 <strong>{countRuns}</strong> 次</button>
                <button className={`stat-pill done${pillFilter === "done" ? " on" : ""}`}
                  onClick={(e) => openPill(e, "done")}>完成 <strong>{countDone}</strong> 次</button>
                <button className={`stat-pill failed${pillFilter === "failed" ? " on" : ""}`}
                  onClick={(e) => openPill(e, "failed")}>失败 <strong>{countFailed}</strong> 次</button>
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
                          <li
                            key={s.id}
                            className={`sop-picker-item${s.id === activeSopId ? " on" : ""}`}
                            data-sop-id={s.id}
                            role="button"
                            tabIndex={0}
                            onClick={() => { setSelectedSopId(s.id); setTargetRunId(""); setSopPickerOpen(false); }}
                            onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { setSelectedSopId(s.id); setSopPickerOpen(false); } }}
                          >
                            <div className="sop-picker-select">
                              <strong>{s.name}</strong> <span className="ver">v{s.version}</span>
                            </div>
                            <div className="row actions sop-picker-actions">
                              <button disabled={!!busy} onClick={(e) => { e.stopPropagation(); void runAndOpen(() => runStart(s.id)); }}>手动运行</button>
                              <button disabled={!!busy} onClick={(e) => { e.stopPropagation(); void runAndOpen(() => runAsAgent(s.id)); }}>以 agent 跑</button>
                              <button disabled={!!busy} onClick={(e) => { e.stopPropagation(); void act("启停", () => sopSetEnabled(s.id, !s.enabled)); }}>
                                {s.enabled ? "停用" : "启用"}
                              </button>
                              {sched ? (
                                <button disabled={!!busy} onClick={(e) => { e.stopPropagation(); void act("停自动", () => scheduleSet(s.id, false, 0, null)); }}>停自动</button>
                              ) : (
                                <select
                                  disabled={!!busy}
                                  value="0"
                                  onClick={(e) => e.stopPropagation()}
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

          {pillFilter && (
            <>
              <div className="sop-picker-away" onClick={() => setPillFilter("")} />
              <div className="sop-picker-menu pill-menu" data-testid="pill-drill" style={{ top: pillPos?.top, left: pillPos?.left }}>
                <div className="row pill-menu-head">
                  <strong>{pillFilter === "all" ? "全部运行" : pillFilter === "done" ? "完成" : "失败/取消"}任务（{pillRuns.length}）</strong>
                  <button className="pill-drill-close" onClick={() => setPillFilter("")}>收起</button>
                </div>
                {pillRuns.length === 0 ? (
                  <p className="empty">该状态下暂无任务。</p>
                ) : (
                  <ul className="list sop-picker-list">
                    {pillRuns.map((r) => (
                      <li key={r.id} data-testid="pill-task" className="sop-picker-item">
                        <div className="row">
                          <strong>{STATUS_ZH[r.status] ?? r.status}</strong>
                          <span className="ver">{r.sopId}</span>
                          <span>{dayStr(r.startedAt ?? 0)}</span>
                        </div>
                        <div className="row actions">
                          <button onClick={() => focusRun(r.id)}>查看甘特图</button>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </>
          )}

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
            <BoardPanel runs={scopedRuns} onOpen={openRun} activeStatus={ganttDetail?.run.status ?? null} />
          )}
          {boardView === "gantt" && (
            <section className="card board-gantt" data-testid="board-gantt">
              {!ganttDetail || ganttDetail.run.id !== activeGanttId ? (
                <p className="empty">{activeGanttId ? "加载运行详情…" : "该日期范围内没有运行记录。"}</p>
              ) : (
                <>
                  <div className="row gantt-detail-head">
                    <h2>运行详情</h2>
                    <span className="ver">{ganttDetail.run.id}</span>
                    <span className="pill">{STATUS_ZH[ganttDetail.run.status] ?? ganttDetail.run.status}</span>
                    {ganttDetail.gates.some((g) => g.decidedAt === null) && (
                      <span className="pill off" data-testid="gate-pending-badge">
                        待你批准 {ganttDetail.gates.filter((g) => g.decidedAt === null).length} 项
                      </span>
                    )}
                  </div>
                  {(() => {
                    const pending = ganttDetail.gates.filter((g) => g.decidedAt === null);
                    if (!pending.length) return null;
                    const reasonZh: Record<string, string> = {
                      anomaly_review: "异常清单审核", pause: "暂停确认", takeover: "人工接管确认",
                      acceptance: "验收确认",
                    };
                    return (
                      <div className="stack gantt-action-box gate-top" data-testid="gate-approve-box">
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
                                onClick={() => act("批准", async () => { await gateDecide(g.id, "approve", arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>批准通过</button>
                              <button disabled={!!busy} data-gate-action="reject"
                                onClick={() => act("驳回", async () => { await gateDecide(g.id, "reject", arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>驳回</button>
                            </div>
                          </div>
                        ))}
                        <label className="stack">批准 / 驳回说明（可选，留痕）
                          <input value={arbNote} onChange={(e) => setArbNote(e.target.value)} placeholder="写明理由" />
                        </label>
                      </div>
                    );
                  })()}
                  <RunGantt detail={ganttDetail} />
                  <details className="steps-fold">
                    <summary>步骤明细（{ganttDetail.steps.length}）</summary>
                    <ol className="steps">
                      {ganttDetail.steps.map((s) => (
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
                    {ganttDetail.acceptance.map((a) => (
                      <li key={a.id}>{a.passed ? "✓" : "✗"} {a.criterion}</li>
                    ))}
                    {ganttDetail.acceptance.length === 0 && <li className="empty">无验收项</li>}
                  </ul>
                  {(ganttDetail.run.status === "rejected" || ganttDetail.run.status === "review") && (
                    <div className="stack gantt-action-box" data-testid="arbitration-box">
                      <label className="stack">仲裁/重试说明（可选，留痕）
                        <input value={arbNote} onChange={(e) => setArbNote(e.target.value)} placeholder="写明理由" />
                      </label>
                      <div className="row actions">
                        {ganttDetail.run.status === "rejected" && (
                          <button disabled={!!busy} onClick={() => act("重试", async () => { await runRetry(ganttDetail.run.id); setGanttDetail(await runGet(ganttDetail.run.id)); })}>
                            重试（第 2 轮）
                          </button>
                        )}
                        {ganttDetail.run.status === "review" && (
                          <>
                            <button disabled={!!busy} onClick={() => act("仲裁放行", async () => { await runArbitrate(ganttDetail.run.id, "resume", arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>
                              仲裁放行
                            </button>
                            <button disabled={!!busy} onClick={() => act("仲裁失败", async () => { await runArbitrate(ganttDetail.run.id, "fail", arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>
                              仲裁失败
                            </button>
                          </>
                        )}
                      </div>
                    </div>
                  )}
                  {!["done", "failed", "cancelled"].includes(ganttDetail.run.status) && (
                    <div className="stack gantt-action-box" data-testid="control-box">
                      <h3>人工干预（保留上下文）</h3>
                      <p className="sub-row">接管把当前步交给人工；改路径只改本次 run 的剩余节点。暂停 / 恢复只放行「暂停门」，不绕过真实人工门——真实人工门请在上方批准。</p>
                      <label className="stack">改路径目标节点 id（当前步骤见上方步骤列表）
                        <input value={pathTarget} onChange={(e) => setPathTarget(e.target.value)} placeholder="如 n3" />
                      </label>
                      <div className="row actions">
                        {["running", "pending"].includes(ganttDetail.run.status) && (
                          <button disabled={!!busy} onClick={() => act("暂停", async () => { await runControl(ganttDetail.run.id, "pause"); setGanttDetail(await runGet(ganttDetail.run.id)); })}>暂停</button>
                        )}
                        {ganttDetail.run.status === "wait_human" &&
                          (() => {
                            const pending = ganttDetail.gates.filter((g) => g.decidedAt === null);
                            const onlyPause = pending.length === 0 || pending.every((g) => g.reason === "pause");
                            return onlyPause ? (
                              <button disabled={!!busy} onClick={() => act("恢复", async () => { await runControl(ganttDetail.run.id, "resume", undefined, arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>恢复（暂停门）</button>
                            ) : (
                              <span className="sub-row">有真实人工门待批准，「恢复」不可用——请在上方批准。</span>
                            );
                          })()}
                        <button disabled={!!busy} onClick={() => act("接管", async () => { await runControl(ganttDetail.run.id, "takeover", undefined, arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>接管当前步</button>
                        <button disabled={!!busy} onClick={() => act("改路径", async () => { await runControl(ganttDetail.run.id, "change_path", pathTarget, arbNote || undefined); setGanttDetail(await runGet(ganttDetail.run.id)); })}>改路径</button>
                      </div>
                    </div>
                  )}
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
          <section className="card">
            <div className="history-tabs" role="tablist" aria-label="历史分析">
              <button className={historySub === "query" ? "tab on" : "tab"} onClick={() => setHistorySub("query")}>历史查询</button>
              <button className={historySub === "stats" ? "tab on" : "tab"} onClick={() => setHistorySub("stats")}>历史统计</button>
            </div>
          </section>

          {historySub === "query" ? (
          <section className="card" data-testid="history-query">
            <h2>历史查询</h2>
            <div className="row history-filters">
              <label>起<input type="date" value={historyFrom} onChange={(e) => setHistoryFrom(e.target.value)} /></label>
              <label>止<input type="date" value={historyTo} onChange={(e) => setHistoryTo(e.target.value)} /></label>
              <label>SOP
                <select value={historySop} onChange={(e) => setHistorySop(e.target.value)}>
                  <option value="">全部</option>
                  {data.sops.map((s) => <option key={s.id} value={s.id}>{s.name}</option>)}
                </select>
              </label>
              <label>状态
                <select value={historyStatus} onChange={(e) => setHistoryStatus(e.target.value)}>
                  <option value="">全部</option>
                  {Object.entries(STATUS_ZH).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
                </select>
              </label>
            </div>
            <p className="sub-row">共 {historyRuns.length} 条记录（按开始日期）</p>
            {historyRuns.length === 0 ? (
              <p className="empty">没有符合条件的记录。</p>
            ) : (
              <ul className="list">
                {historyRuns.map((r) => (
                  <li key={r.id} data-testid="history-run">
                    <div className="row">
                      <strong>{STATUS_ZH[r.status] ?? r.status}</strong>
                      <span className="ver">{r.sopId}</span>
                      <span>{dayStr(r.startedAt ?? 0)}</span>
                    </div>
                    <div className="row actions">
                      <button onClick={() => focusRun(r.id)}>查看甘特图</button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>
          ) : (
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
          )}
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
