import { useEffect, useState } from "react";
import {
  correctionAdd,
  delegationDemoInstall,
  errMessage,
  ownerDecisionList,
  reportGet,
  runGet,
  templateCreate,
  templateExport,
  templateImport,
  templateList,
  upgradeApprove,
  upgradeCandidateList,
  runList,
  type EfficiencyReport,
  type OwnerDecisionRow,
  type RunDetail,
  type RunRow,
  type TemplateRow,
  type UpgradeCandidate,
} from "../lib/ipc";

function bytesToBase64(bytes: number[]): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}
function base64ToBytes(b64: string): number[] {
  const s = atob(b64.trim());
  return Array.from(s, (c) => c.charCodeAt(0));
}

export default function ExperiencePanel({ onError }: { onError: (s: string) => void }) {
  const [templates, setTemplates] = useState<TemplateRow[]>([]);
  const [runs, setRuns] = useState<RunRow[]>([]);
  const [candidates, setCandidates] = useState<UpgradeCandidate[]>([]);
  const [report, setReport] = useState<EfficiencyReport | null>(null);
  const [decisions, setDecisions] = useState<OwnerDecisionRow[]>([]);
  const [selRun, setSelRun] = useState("");
  const [tplName, setTplName] = useState("");
  const [importText, setImportText] = useState("");
  const [busy, setBusy] = useState("");
  // 人工修正录入：选运行 → 选 human-owned 的可放权步骤 → 填原输出/修正后 → 记录。
  const [corrRun, setCorrRun] = useState("");
  const [detail, setDetail] = useState<RunDetail | null>(null);
  const [corrStep, setCorrStep] = useState("");
  const [corrOriginal, setCorrOriginal] = useState("{}");
  const [corrCorrected, setCorrCorrected] = useState("{}");
  const [corrNote, setCorrNote] = useState("");

  const load = async () => {
    try {
      const [t, r, c, p, d] = await Promise.all([
        templateList(), runList(undefined, undefined, 20),
        upgradeCandidateList(), reportGet(), ownerDecisionList(),
      ]);
      setTemplates(t); setRuns(r); setCandidates(c); setReport(p); setDecisions(d); onError("");
    } catch (e) { onError(errMessage(e)); }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line */ }, []);

  // 选中运行后拉取其步骤，供修正录入选择。
  useEffect(() => {
    if (!corrRun) { setDetail(null); setCorrStep(""); return; }
    let alive = true;
    void runGet(corrRun)
      .then((d) => { if (alive) { setDetail(d); setCorrStep(""); } })
      .catch((e) => onError(errMessage(e)));
    return () => { alive = false; };
    /* eslint-disable-next-line */
  }, [corrRun]);

  const act = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try { await fn(); await load(); } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const doneRuns = runs.filter((r) => r.status === "done");

  return (
    <>
      <section className="card">
        <h2>从运行捕获经验模板</h2>
        <p className="sub-row">仅「实测通过（verdict=通过）」的运行捕获的模板 validated=1，才可导入复用。</p>
        <div className="row">
          <select value={selRun} onChange={(e) => setSelRun(e.target.value)}>
            <option value="">选一条已完成的运行</option>
            {doneRuns.map((r) => <option key={r.id} value={r.id}>{r.id} · {r.verdict ?? "—"}</option>)}
          </select>
          <input placeholder="模板名" value={tplName} onChange={(e) => setTplName(e.target.value)} />
          <button disabled={!!busy || !selRun || !tplName} onClick={() => act("捕获", () =>
            templateCreate(selRun, tplName, null))}>捕获为模板</button>
        </div>
      </section>

      <section className="card">
        <h2>模板库 <span className="count">{templates.length}</span></h2>
        <ul className="list">
          {templates.map((t) => (
            <li key={t.id} data-testid="template-item">
              <div className="row">
                <strong>{t.name}</strong>
                <span className={`pill ${t.validated ? "on" : "off"}`}>
                  {t.validated ? "实测通过" : "未验证"}
                </span>
                <span className="ver">{t.sourceShop ?? "无来源店"}</span>
              </div>
              <div className="row actions">
                <button disabled={!!busy} onClick={() => act("导出", async () => {
                  const bytes = await templateExport(t.id);
                  setImportText(bytesToBase64(bytes));
                })}>导出(base64)</button>
              </div>
            </li>
          ))}
          {templates.length === 0 && <li className="empty">暂无模板。</li>}
        </ul>
        <div className="row" style={{ marginTop: 12 }}>
          <input className="wide" placeholder="粘贴 zip 的 base64 导入" value={importText}
            onChange={(e) => setImportText(e.target.value)} />
          <button disabled={!!busy || !importText} onClick={() => act("导入", () =>
            templateImport(base64ToBytes(importText)))}>导入模板</button>
        </div>
      </section>

      <section className="card">
        <h2>记录人工修正</h2>
        <p className="sub-row">对 human-owned 的 Agent 步记录「原输出 vs 修正后」；累积到阈值才会出现升级候选。结构人工门不可放权。</p>
        <div className="row" style={{ marginBottom: 8 }}>
          <button disabled={!!busy} onClick={() => act("安装演示", () => delegationDemoInstall())}>
            安装放权演示 SOP（含 1 个 human-owned Agent 步，本地 Mock）
          </button>
        </div>
        <div className="row">
          <select value={corrRun} onChange={(e) => setCorrRun(e.target.value)}>
            <option value="">选一条运行</option>
            {runs.map((r) => <option key={r.id} value={r.id}>{r.id} · {r.status}</option>)}
          </select>
          <select value={corrStep} onChange={(e) => setCorrStep(e.target.value)} disabled={!detail}>
            <option value="">选 human-owned 步骤</option>
            {(detail?.steps ?? [])
              .filter((s) => s.owner === "human" && ["agent_step", "mcp_tool"].includes(s.nodeType))
              .map((s) => <option key={s.id} value={s.id}>{s.nodeType} · {s.nodeId ?? s.id}</option>)}
          </select>
        </div>
        <div className="row" style={{ marginTop: 8 }}>
          <input className="wide" placeholder="原输出 JSON" value={corrOriginal} onChange={(e) => setCorrOriginal(e.target.value)} />
          <input className="wide" placeholder="修正后 JSON" value={corrCorrected} onChange={(e) => setCorrCorrected(e.target.value)} />
        </div>
        <div className="row" style={{ marginTop: 8 }}>
          <input className="wide" placeholder="备注（可选）" value={corrNote} onChange={(e) => setCorrNote(e.target.value)} />
          <button disabled={!!busy || !corrStep || !corrOriginal.trim() || !corrCorrected.trim()}
            onClick={() => act("记录", () => correctionAdd(corrStep, corrOriginal, corrCorrected, corrNote.trim() || null))}>
            记录修正
          </button>
        </div>
      </section>

      <section className="card">
        <h2>owner 升级候选 <span className="count">{candidates.length}</span></h2>
        <p className="sub-row">人工修正累积达阈值只产生候选；确认后真正把该节点的 owner 改为 agent（SOP 版本递增），后续运行不再停人工门。资金/不可逆动作永不升级。</p>
        <ul className="list">
          {candidates.map((c) => (
            <li key={`${c.sopId}:${c.nodeId}`} data-testid="upgrade-candidate">
              <div className="row">
                <strong>{c.nodeType}</strong>
                <span className="ver">{c.nodeId}</span>
                <span className="pill">{c.correctionCount}/{c.threshold}</span>
                <span className={`pill ${c.eligible ? "on" : "off"}`}>{c.eligible ? "可确认" : "禁止升级"}</span>
              </div>
              <div className="row sub-row"><span>SOP: {c.sopId} · {c.reason}</span></div>
              {c.eligible && (
                <div className="row actions">
                  <button disabled={!!busy} onClick={() => act("确认", () => upgradeApprove(c.sopId, c.nodeId, true))}>确认升级</button>
                  <button disabled={!!busy} onClick={() => act("驳回", () => upgradeApprove(c.sopId, c.nodeId, false))}>驳回</button>
                </div>
              )}
            </li>
          ))}
          {candidates.length === 0 && <li className="empty">暂无候选（修正未达阈值，或只有结构人工门）。</li>}
        </ul>
      </section>

      <section className="card">
        <h2>放权决策记录 <span className="count">{decisions.length}</span></h2>
        <ul className="list">
          {decisions.map((d) => (
            <li key={d.id} data-testid="owner-decision">
              <div className="row">
                <span className={`pill ${d.decision === "approve" ? "on" : "off"}`}>
                  {d.decision === "approve" ? "已放权" : "已驳回"}
                </span>
                <strong>{d.nodeType} · {d.nodeId}</strong>
                <span className="ver">{d.fromOwner} → {d.toOwner}</span>
                <span className="ver">{d.correctionCount} 条修正</span>
              </div>
              <div className="row sub-row"><span>SOP: {d.sopId} · 由 {d.decidedBy} 决定</span></div>
            </li>
          ))}
          {decisions.length === 0 && <li className="empty">暂无放权决策。</li>}
        </ul>
      </section>

      <section className="card">
        <h2>效率报表</h2>
        {!report ? <p className="empty">加载中…</p> : (
          <dl className="info">
            <div><dt>运行</dt><dd>{report.completedRuns} / {report.totalRuns} 完成</dd></div>
            <div><dt>步骤</dt><dd>{report.agentDoneSteps} / {report.totalSteps} 由 Agent 独立完成</dd></div>
            <div><dt>自动化率</dt><dd>{(report.automationRate * 100).toFixed(0)}%</dd></div>
            <div><dt>人工介入</dt><dd>{report.humanInterventions} 次</dd></div>
            <div><dt>验收轮数</dt><dd>平均 {report.avgAcceptanceRounds.toFixed(1)} 轮/运行</dd></div>
            <div><dt>口径</dt><dd>{report.savedStepsNote}</dd></div>
          </dl>
        )}
      </section>
    </>
  );
}
