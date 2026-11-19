import { useEffect, useState } from "react";
import {
  errMessage,
  reportGet,
  templateCreate,
  templateExport,
  templateImport,
  templateList,
  upgradeApprove,
  upgradeCandidateList,
  runList,
  type EfficiencyReport,
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
  const [selRun, setSelRun] = useState("");
  const [tplName, setTplName] = useState("");
  const [importText, setImportText] = useState("");
  const [busy, setBusy] = useState("");

  const load = async () => {
    try {
      const [t, r, c, p] = await Promise.all([
        templateList(), runList(undefined, undefined, 20),
        upgradeCandidateList(), reportGet(),
      ]);
      setTemplates(t); setRuns(r); setCandidates(c); setReport(p); onError("");
    } catch (e) { onError(errMessage(e)); }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line */ }, []);

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
        <h2>owner 升级候选 <span className="count">{candidates.length}</span></h2>
        <p className="sub-row">人工修正累积达阈值只产生候选，须人确认；资金/不可逆动作永不升级。</p>
        <ul className="list">
          {candidates.map((c) => (
            <li key={c.nodeId} data-testid="upgrade-candidate">
              <div className="row">
                <strong>{c.nodeType}</strong>
                <span className="ver">{c.nodeId}</span>
                <span className="pill">{c.correctionCount}/{c.threshold}</span>
                <span className={`pill ${c.eligible ? "on" : "off"}`}>{c.eligible ? "可确认" : "禁止升级"}</span>
              </div>
              <div className="row sub-row"><span>{c.reason}</span></div>
              {c.eligible && (
                <div className="row actions">
                  <button disabled={!!busy} onClick={() => act("确认", () => upgradeApprove(c.nodeId, true))}>确认升级</button>
                  <button disabled={!!busy} onClick={() => act("驳回", () => upgradeApprove(c.nodeId, false))}>驳回</button>
                </div>
              )}
            </li>
          ))}
          {candidates.length === 0 && <li className="empty">暂无候选（修正未达阈值）。</li>}
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
