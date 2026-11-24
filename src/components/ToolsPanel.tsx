import { useEffect, useState } from "react";
import {
  credentialList,
  credentialRemove,
  credentialSet,
  errMessage,
  toolGrantList,
  toolGrantSet,
  toolList,
  type CredentialRef,
  type ToolGrantRow,
  type ToolRow,
} from "../lib/ipc";

const MODES = ["plan", "build", "review"];
const PLANES = ["public", "content", "cost", "ops", "finance"];

export default function ToolsPanel({ onError, role: roleProp }: { onError: (s: string) => void; role?: string }) {
  // 角色由登录用户决定（未登录回退 agent）；不再允许在此手动切换，避免与权限体系脱节。
  const role = roleProp ?? "agent";
  const [mode, setMode] = useState("build");
  const [tools, setTools] = useState<ToolRow[]>([]);
  const [grants, setGrants] = useState<ToolGrantRow[]>([]);
  const [creds, setCreds] = useState<CredentialRef[]>([]);
  const [busy, setBusy] = useState("");
  const [newRef, setNewRef] = useState("");
  const [newProvider, setNewProvider] = useState("");
  const [newValue, setNewValue] = useState("");

  const load = async () => {
    try {
      const [t, g, c] = await Promise.all([toolList(role, mode), toolGrantList(), credentialList()]);
      setTools(t); setGrants(g); setCreds(c); onError("");
    } catch (e) { onError(errMessage(e)); }
  };

  useEffect(() => { void load(); /* eslint-disable-next-line */ }, [role, mode]);

  const act = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try { await fn(); await load(); } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const grantFor = (toolId: string, m: string) =>
    grants.find((g) => g.toolId === toolId && g.mode === m);

  const toggleGrant = (tool: ToolRow, m: string) => {
    const g = grantFor(tool.id, m);
    const planes = g?.planes ?? ["public", "content"];
    void act("授权", () => toolGrantSet(tool.id, m, !(g?.allowed ?? false), planes));
  };

  return (
    <>
      <section className="card">
        <h2>工具注册表</h2>
        <div className="row">
          <span className="pill">角色：{role}（由登录用户决定，未登录为 agent）</span>
          <label>模式
            <select value={mode} onChange={(e) => setMode(e.target.value)}>
              {MODES.map((m) => <option key={m} value={m}>{m}</option>)}
            </select>
          </label>
        </div>
        <ul className="list">
          {tools.map((t) => (
            <li key={t.id} data-testid="tool-item">
              <div className="row">
                <strong>{t.title}</strong>
                <span className="pill">{t.risk}</span>
                <span className="ver">面 {t.plane}</span>
                <span className={`pill ${t.visible ? "on" : "off"}`}>
                  {t.visible ? "当前可见" : "不可见"}
                </span>
              </div>
              <div className="row sub-row">
                <span>{t.id}</span>
                {t.requiresApproval && <span>需批准</span>}
                {t.irreversible && <span className="pill off">不可逆·永人工</span>}
              </div>
              <div className="row actions">
                {MODES.map((m) => (
                  <button key={m} disabled={!!busy} onClick={() => toggleGrant(t, m)}>
                    {m}:{grantFor(t.id, m)?.allowed ? "开" : "关"}
                  </button>
                ))}
              </div>
            </li>
          ))}
        </ul>
        <p className="sub-row">数据面：{PLANES.join(" / ")} · 内容角色看不到 cost（进价）面。</p>
      </section>

      <section className="card">
        <h2>密钥引用（值只在 keystore，不落库）</h2>
        <div className="row">
          <input placeholder="ref_id" value={newRef} onChange={(e) => setNewRef(e.target.value)} />
          <input placeholder="provider" value={newProvider} onChange={(e) => setNewProvider(e.target.value)} />
          <input type="password" placeholder="值（不落库）" value={newValue} onChange={(e) => setNewValue(e.target.value)} />
          <button disabled={!!busy} onClick={() => act("保存", async () => {
            await credentialSet(newRef, newProvider || "custom", null, newValue);
            setNewRef(""); setNewProvider(""); setNewValue("");
          })}>保存</button>
        </div>
        <ul className="list">
          {creds.map((c) => (
            <li key={c.refId}>
              <div className="row">
                <strong>{c.refId}</strong><span className="ver">{c.provider}</span>
                <button disabled={!!busy} onClick={() => act("删除", () => credentialRemove(c.refId))}>删除</button>
              </div>
              <div className="row sub-row"><span>storage_key: {c.storageKey}</span></div>
            </li>
          ))}
          {creds.length === 0 && <li className="empty">暂无密钥引用。</li>}
        </ul>
      </section>
    </>
  );
}
