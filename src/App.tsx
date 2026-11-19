import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type AppInfo = { name: string; version: string; db_path: string; table_count: number };

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [err, setErr] = useState("");

  useEffect(() => {
    invoke<AppInfo>("app_info")
      .then(setInfo)
      .catch((e) => setErr(String(e)));
  }, []);

  return (
    <main className="wrap" data-app="abi-pilot">
      <h1>ABI-Pilot</h1>
      <p className="sub">人机协同电商运营工作台 · v0.1 骨架</p>
      {err ? (
        <p className="err" data-testid="error">加载失败：{err}</p>
      ) : !info ? (
        <p data-testid="loading">加载中…</p>
      ) : (
        <dl className="info" data-testid="app-info">
          <div><dt>名称</dt><dd>{info.name}</dd></div>
          <div><dt>版本</dt><dd>{info.version}</dd></div>
          <div><dt>数据库</dt><dd className="path">{info.db_path}</dd></div>
          <div><dt>已建表</dt><dd data-testid="table-count">{info.table_count}</dd></div>
        </dl>
      )}
    </main>
  );
}
