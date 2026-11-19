import { useEffect, useState } from "react";
import {
  errMessage,
  exampleToolsInstall,
  maintenanceSopInstall,
  toolGrantSet,
  toolList,
  toolRemove,
  toolUpsert,
  type ToolRow,
} from "../lib/ipc";

const DEFAULT_SCOPE = `{
  "kind": "http",
  "method": "GET",
  "url": "https://your-service.example/api",
  "query": {},
  "timeout_s": 15,
  "plane": "public",
  "read_only": true
}`;

export default function CustomToolsPanel({ onError }: { onError: (s: string) => void }) {
  const [tools, setTools] = useState<ToolRow[]>([]);
  const [busy, setBusy] = useState("");
  const [id, setId] = useState("my:tool");
  const [title, setTitle] = useState("我的 HTTP 工具");
  const [risk, setRisk] = useState("low");
  const [scopeText, setScopeText] = useState(DEFAULT_SCOPE);

  const load = async () => {
    try {
      const t = await toolList("agent", "build");
      setTools(t); onError("");
    } catch (e) { onError(errMessage(e)); }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line */ }, []);

  const act = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try { await fn(); await load(); } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const save = () => act("保存", async () => {
    const scope = JSON.parse(scopeText);
    await toolUpsert({
      id, title, risk,
      server: scope.kind === "http" ? "http" : "builtin",
      command: scope.method ?? "GET",
      scope, enabled: true,
    });
    // 新工具默认三模式授权其 plane。
    for (const m of ["plan", "build", "review"]) {
      await toolGrantSet(id, m, true, [scope.plane ?? "public"]);
    }
  });

  return (
    <>
      <section className="card">
        <h2>一键示例（第一个使用者）</h2>
        <p className="sub-row">
          安装指向 agentbi.tech 的只读示例工具 + 一条「每日维护」SOP。这些只是可改的配置数据，核心引擎不绑定任何站点。
        </p>
        <div className="row actions">
          <button disabled={!!busy} onClick={() => act("装示例工具", () => exampleToolsInstall())}>安装示例工具</button>
          <button disabled={!!busy} onClick={() => act("装维护 SOP", () => maintenanceSopInstall())}>安装每日维护 SOP</button>
        </div>
      </section>

      <section className="card">
        <h2>自定义 HTTP 工具（工具即数据）</h2>
        <p className="sub-row">
          改 scope 里的 url/method/query 即可指向任何服务；鉴权用 <code>{"{{cred:ref_id}}"}</code>，
          参数用 <code>{"{{arg:name}}"}</code>。密钥只从 keystore 取，不落库不进日志。
        </p>
        <div className="row">
          <label>id<input value={id} onChange={(e) => setId(e.target.value)} /></label>
          <label>名称<input value={title} onChange={(e) => setTitle(e.target.value)} /></label>
          <label>风险
            <select value={risk} onChange={(e) => setRisk(e.target.value)}>
              <option value="low">low</option><option value="medium">medium</option><option value="high">high</option>
            </select>
          </label>
        </div>
        <label className="stack">scope（JSON，含 kind/method/url/query/headers/body/plane）
          <textarea rows={10} value={scopeText} onChange={(e) => setScopeText(e.target.value)} />
        </label>
        <div className="row actions"><button disabled={!!busy} onClick={save}>保存工具</button></div>
      </section>

      <section className="card">
        <h2>已注册工具 <span className="count">{tools.length}</span></h2>
        <ul className="list">
          {tools.map((t) => (
            <li key={t.id} data-testid="custom-tool-item">
              <div className="row">
                <strong>{t.id}</strong>
                <span className="pill">{t.risk}</span>
                <span className="ver">面 {t.plane}</span>
                {t.irreversible && <span className="pill off">不可逆</span>}
              </div>
              <div className="row actions">
                <button disabled={!!busy} onClick={() => act("删除", () => toolRemove(t.id))}>删除</button>
              </div>
            </li>
          ))}
          {tools.length === 0 && <li className="empty">暂无工具，先装示例或自定义一个。</li>}
        </ul>
      </section>
    </>
  );
}
