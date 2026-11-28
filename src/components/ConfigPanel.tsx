// 应用配置 + MCP server 配置（§3.1 补齐 IPC）。
// 配置不含密钥值；改 LLM 配置后需重启才生效（AppLlm 在 setup 构建一次）。
// MCP server 配置会被执行器真连：stdio 可透传 env 变量名，值由用户自填 secrets.env。

import { useEffect, useState } from "react";
import {
  configGet, configSet, mcpServerList, mcpServerAdd, mcpServerRemove, secretsEnvPath,
  builtinToolsInstall,
  errMessage,
  type McpServerInfo,
} from "../lib/ipc";
import { currentTheme, saveTheme, type Theme } from "../lib/theme";

export default function ConfigPanel({ onError }: { onError: (s: string) => void }) {
  const [cfg, setCfg] = useState<Record<string, string>>({});
  const [servers, setServers] = useState<McpServerInfo[]>([]);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [newSrv, setNewSrv] = useState<McpServerInfo>({ id: "", command: "", args: [], note: "", transport: "stdio", url: "", env: [] });
  const [secretsPath, setSecretsPath] = useState("");
  const [busy, setBusy] = useState("");
  const [theme, setTheme] = useState<Theme>(currentTheme());

  const switchTheme = async (t: Theme) => {
    if (t === theme) return;
    setTheme(t);
    try { await saveTheme(t, configSet); onError(""); }
    catch (e) { onError(errMessage(e)); }  // DOM 已切、本地已存；后端写失败只提示
  };

  const load = async () => {
    try {
      const [c, s, sp] = await Promise.all([configGet(), mcpServerList(), secretsEnvPath()]);
      setCfg(c); setDraft({ ...c }); setServers(s); setSecretsPath(sp);
      onError("");
    } catch (e) { onError(errMessage(e)); }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line */ }, []);

  const saveCfg = async () => {
    setBusy("save-cfg");
    try {
      // 只写有变化的非密钥项（后端也会拒绝密钥类键名）。
      for (const [k, v] of Object.entries(draft)) {
        if (cfg[k] !== v) await configSet(k, v);
      }
      await load();
    } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const addSrv = async () => {
    if (!newSrv.id.trim() || !newSrv.command.trim()) { onError("id 与 command 必填"); return; }
    setBusy("add-srv");
    try {
      setServers(await mcpServerAdd({ ...newSrv, args: newSrv.args.filter(Boolean), env: (newSrv.env ?? []).filter(Boolean) }));
      setNewSrv({ id: "", command: "", args: [], note: "", transport: "stdio", url: "", env: [] });
      onError("");
    } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const removeSrv = async (id: string) => {
    setBusy(`rm-${id}`);
    try { setServers(await mcpServerRemove(id)); onError(""); }
    catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const installBuiltin = async () => {
    setBusy("install-builtin");
    try {
      const ids = await builtinToolsInstall();
      onError(`已安装 ${ids.length} 个内置工具：${ids.join("、")}`);
    } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const llmKeys = ["llm.provider", "llm.host", "llm.model"];

  return (
    <div className="stack" data-testid="config-panel">
      <section className="card">
        <h2>外观主题</h2>
        <p className="sub-row">默认浅色。切换立即生效并记住选择（重启后保持）。</p>
        <div className="row actions" data-testid="theme-switch">
          <button data-active={theme === "light"} onClick={() => switchTheme("light")}>☀ 浅色</button>
          <button data-active={theme === "dark"} onClick={() => switchTheme("dark")}>☾ 深色</button>
        </div>
      </section>

      <section className="card">
        <h2>应用配置</h2>
        <p className="sub-row">非敏感配置；密钥只进密钥链/llm.env，不在此显示或保存。LLM 配置改后需重启生效；云 API 的 ABI_LLM_API_KEY 只写到 llm.env。</p>
        {llmKeys.map((k) => (
          <label className="stack" key={k}>{k}
            <input
              value={draft[k] ?? ""}
              placeholder={k === "llm.host" ? "http://127.0.0.1:11434 或 https://open.bigmodel.cn/api/paas/v4" : k === "llm.model" ? "qwen2.5:3b 或 glm-4.7-flash" : "ollama / openai-compatible / 留空=Mock"}
              onChange={(e) => setDraft((d) => ({ ...d, [k]: e.target.value }))}
            />
          </label>
        ))}
        <div className="row actions">
          <button disabled={!!busy} onClick={saveCfg}>{busy === "save-cfg" ? "保存中…" : "保存配置"}</button>
        </div>
      </section>

      <section className="card">
        <h2>MCP server 配置 <span className="count">{servers.length}</span></h2>
        <div className="row actions">
          <button data-testid="builtin-tools-install" disabled={!!busy} onClick={installBuiltin}>
            {busy === "install-builtin" ? "安装中…" : "安装内置工具（filesystem / shell / browser）"}
          </button>
        </div>
        <p className="sub-row">
          内置 server 无需手工配置 mcp_servers.json；filesystem 限 sandbox 目录、shell 走只读白名单，
          browser 遇登录/验证码插人工门转人工（不绕过）。
        </p>
        <p className="sub-row">
          执行器会按此配置真连 MCP server（写 mcp_servers.json）。stdio 可填要透传的<strong>环境变量名</strong>；
          值请自己写进 secrets.env：<code>{secretsPath || "（启动后可见）"}</code>（600 权限，不进配置/日志/git）。
        </p>
        <ul className="list">
          {servers.map((s) => (
            <li key={s.id} data-testid="mcp-server-item">
              <div className="row"><strong>{s.id}</strong><span className="ver">{s.transport === "http" ? s.url : `${s.command} ${s.args.join(" ")}`}</span><span className="pill">{s.transport ?? "stdio"}</span></div>
              {(s.env ?? []).length > 0 && <p className="sub-row">透传 env 名：{s.env.join("、")}</p>}
              {s.note && <p className="sub-row">{s.note}</p>}
              <div className="row actions">
                <button disabled={!!busy} onClick={() => removeSrv(s.id)}>删除</button>
              </div>
            </li>
          ))}
          {servers.length === 0 && <li className="empty">暂无 MCP server 配置。</li>}
        </ul>
        <div className="stack">
          <label className="stack">id <input value={newSrv.id} onChange={(e) => setNewSrv({ ...newSrv, id: e.target.value })} /></label>
          <label className="stack">传输方式
            <select value={newSrv.transport ?? "stdio"} onChange={(e) => setNewSrv({ ...newSrv, transport: e.target.value })}>
              <option value="stdio">stdio（command+args 启子进程）</option>
              <option value="http">http（POST 到 url）</option>
            </select>
          </label>
          {newSrv.transport === "http" ? (
            <label className="stack">url <input value={newSrv.url ?? ""} onChange={(e) => setNewSrv({ ...newSrv, url: e.target.value })} placeholder="http://127.0.0.1:8891/" /></label>
          ) : (
            <>
              <label className="stack">command <input value={newSrv.command} onChange={(e) => setNewSrv({ ...newSrv, command: e.target.value })} placeholder="npx / uvx / python3" /></label>
              <label className="stack">args（空格分隔） <input value={newSrv.args.join(" ")} onChange={(e) => setNewSrv({ ...newSrv, args: e.target.value.split(/\s+/).filter(Boolean) })} /></label>
              <label className="stack">透传 env 变量名（逗号分隔，只填名字不填值）
                <input value={(newSrv.env ?? []).join(",")} onChange={(e) => setNewSrv({ ...newSrv, env: e.target.value.split(/[,，\s]+/).filter(Boolean) })} placeholder="SHOPIFY_STORE_DOMAIN,SHOPIFY_ACCESS_TOKEN" />
              </label>
            </>
          )}
          <label className="stack">说明 <input value={newSrv.note} onChange={(e) => setNewSrv({ ...newSrv, note: e.target.value })} /></label>
          <div className="row actions"><button disabled={!!busy} onClick={addSrv}>添加</button></div>
        </div>
      </section>
    </div>
  );
}
