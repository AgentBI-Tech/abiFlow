import { useEffect, useState } from "react";
import {
  errMessage,
  llmStatus,
  sopParseText,
  sopUpsert,
} from "../lib/ipc";

export default function LlmPanel({ onError }: { onError: (s: string) => void }) {
  const [status, setStatus] = useState<{ name: string; mock: boolean } | null>(null);
  const [text, setText] = useState("每日检查 agentbi.tech 的异常订单，异常转人工");
  const [draft, setDraft] = useState("");
  const [name, setName] = useState("生成的 SOP");
  const [busy, setBusy] = useState("");
  const [saved, setSaved] = useState("");

  useEffect(() => {
    llmStatus().then(setStatus).catch((e) => onError(errMessage(e)));
  }, [onError]);

  const parse = async () => {
    setBusy("parse"); setSaved("");
    try {
      const d = await sopParseText(text);
      setDraft(JSON.stringify(d, null, 2));
      onError("");
    } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const confirmSave = async () => {
    setBusy("save");
    try {
      const def = JSON.parse(draft);
      if (!def.nodes || !def.edges) throw new Error("草稿缺 nodes/edges");
      // 人工确认后才落库；清掉 mock 元字段。
      delete def._mock; delete def._hint;
      const r = await sopUpsert({
        id: undefined, name, triggerJson: '{"type":"manual"}', risk: "low",
        ownerPolicy: "hybrid", definition: def, expectVersion: undefined,
      });
      setSaved(`已保存：${r.id}（v${r.version}）`);
      onError("");
    } catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  return (
    <>
      <section className="card">
        <h2>LLM Provider 状态</h2>
        {!status ? <p className="empty">加载中…</p> : (
          <dl className="info">
            <div><dt>Provider</dt><dd>{status.name}</dd></div>
            <div><dt>模式</dt><dd>{status.mock ? "Mock（离线、未接真模型）" : "真实模型"}</dd></div>
          </dl>
        )}
        <p className="sub-row">
          本机未装 Ollama，默认 Mock（输出标注 mock）。装好 Ollama + 模型后，
          设环境变量 ABI_LLM_PROVIDER=ollama、ABI_LLM_HOST=http://127.0.0.1:11434、
          ABI_LLM_MODEL=qwen2.5:3b，重启即可切真模型，核心代码不动。
        </p>
      </section>

      <section className="card">
        <h2>自然语言生成 SOP（草稿→人工确认）</h2>
        <p className="sub-row">模型只产出草稿，不落库、不放行；确认后才保存为可运行 SOP。</p>
        <label className="stack">用一句话描述需求
          <textarea rows={3} value={text} onChange={(e) => setText(e.target.value)} />
        </label>
        <div className="row actions"><button disabled={!!busy} onClick={parse}>生成草稿</button></div>

        {draft && (
          <>
            <h3>草稿（可编辑，确认后保存）</h3>
            <label className="stack">SOP 名称<input value={name} onChange={(e) => setName(e.target.value)} /></label>
            <label className="stack">AST JSON
              <textarea rows={14} value={draft} onChange={(e) => setDraft(e.target.value)} />
            </label>
            <div className="row actions">
              <button disabled={!!busy} onClick={confirmSave}>确认并保存为 SOP</button>
            </div>
            {saved && <p className="pill on">{saved}</p>}
            <p className="sub-row">提示：草稿里若出现 PLACEHOLDER_TOOL，需改成「自定义工具」tab 里已注册的工具 id。</p>
          </>
        )}
      </section>
    </>
  );
}
