// 用户管理与登录（2026-11-24）：左下角登录入口的完整面板。
// 本地多用户 + 角色 + 密码：角色决定可见数据面（content/ops/admin/agent）。
// 当前登录用户由后端进程内存维护，重启需重新登录；左下角展示登录态。

import { useEffect, useState } from "react";
import {
  userList, userCurrent, userAdd, userLogin, userLogout, userSetRole, userRemove,
  errMessage,
  type UserRow, type CurrentUser,
} from "../lib/ipc";

const ROLE_ZH: Record<string, string> = {
  content: "内容", ops: "运营", admin: "管理员", agent: "Agent",
};
const ROLES = ["content", "ops", "admin", "agent"];

export default function UserPanel({
  onError,
  onUserChange,
}: {
  onError: (s: string) => void;
  onUserChange: (u: CurrentUser | null) => void;
}) {
  const [users, setUsers] = useState<UserRow[]>([]);
  const [current, setCurrent] = useState<CurrentUser | null>(null);
  const [busy, setBusy] = useState("");
  const [open, setOpen] = useState(false);
  // 登录表单
  const [loginUser, setLoginUser] = useState("");
  const [loginPw, setLoginPw] = useState("");
  // 新建用户表单
  const [nu, setNu] = useState({ username: "", displayName: "", role: "content", password: "" });

  const load = async () => {
    try {
      const [list, cur] = await Promise.all([userList(), userCurrent()]);
      setUsers(list);
      setCurrent(cur);
      onUserChange(cur);
      if (!loginUser && list.length) setLoginUser(list[0].username);
      onError("");
    } catch (e) { onError(errMessage(e)); }
  };
  useEffect(() => { void load(); /* eslint-disable-next-line */ }, []);

  const act = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try { await fn(); await load(); onError(""); }
    catch (e) { onError(errMessage(e)); }
    finally { setBusy(""); }
  };

  const doLogin = () => act("login", async () => {
    if (!loginUser || !loginPw) { onError("请选择用户并输入密码"); return; }
    const cur = await userLogin(loginUser, loginPw);
    setCurrent(cur);
    onUserChange(cur);
    setLoginPw("");
  });

  const doLogout = () => act("logout", async () => {
    await userLogout();
    setCurrent(null);
    onUserChange(null);
  });

  const doAdd = () => act("add", async () => {
    if (!nu.username.trim() || !nu.password) { onError("用户名与密码必填"); return; }
    await userAdd(nu.username.trim(), nu.displayName.trim() || nu.username.trim(), nu.role, nu.password);
    setNu({ username: "", displayName: "", role: "content", password: "" });
  });

  return (
    <div className="card" data-user-panel="1">
      <div className="row">
        <strong>用户与权限</strong>
        <span className="count">{users.length} 个本地用户</span>
      </div>
      <p className="sub-row">
        角色决定可见数据面：内容角色看不到进价（cost 面），运营看不到财务面；资金/不可逆动作永远人工审核。
      </p>

      {/* 当前登录态 */}
      <div className="row" data-current-user="1">
        {current ? (
          <>
            <span className="pill">{current.displayName}（@{current.username}）</span>
            <span className="pill">{ROLE_ZH[current.role] ?? current.role}</span>
            <button disabled={!!busy} onClick={() => void doLogout()}>登出</button>
          </>
        ) : (
          <>
            <span className="sub-row">未登录 · run 暂按 Agent 角色运行</span>
            <button disabled={!!busy} onClick={() => setOpen((v) => !v)}>
              {open ? "收起" : "登录 / 管理"}
            </button>
          </>
        )}
      </div>

      {open && (
        <div className="stack">
          {/* 登录：选本地用户 + 密码 */}
          <div className="stack" data-login-form="1">
            <label className="row">用户
              <select value={loginUser} onChange={(e) => setLoginUser(e.target.value)}>
                {users.map((u) => <option key={u.id} value={u.username}>{u.displayName}（@{u.username}）</option>)}
              </select>
            </label>
            <label className="row">密码
              <input type="password" value={loginPw} onChange={(e) => setLoginPw(e.target.value)} placeholder="登录密码" />
            </label>
            <button disabled={!!busy} onClick={() => void doLogin()}>{busy === "login" ? "登录中…" : "登录"}</button>
          </div>

          {/* 用户列表 + 改角色 / 删除 */}
          <div className="list" data-user-list="1">
            {users.length === 0 ? <p className="empty">暂无用户，先在下方新建。</p> : users.map((u) => (
              <div className="row" key={u.id} data-user-row={u.id}>
                <span>{u.displayName}（@{u.username}）</span>
                <select
                  value={u.role}
                  aria-label={`设置 ${u.username} 的角色`}
                  onChange={(e) => void act("role", () => userSetRole(u.id, e.target.value))}
                >
                  {ROLES.map((r) => <option key={r} value={r}>{ROLE_ZH[r]}</option>)}
                </select>
                <button disabled={!!busy || current?.id === u.id} onClick={() => void act("remove", () => userRemove(u.id))}>
                  删除
                </button>
              </div>
            ))}
          </div>

          {/* 新建用户 */}
          <div className="stack" data-user-add="1">
            <div className="row">
              <input value={nu.username} onChange={(e) => setNu({ ...nu, username: e.target.value })} placeholder="用户名（唯一）" />
              <input value={nu.displayName} onChange={(e) => setNu({ ...nu, displayName: e.target.value })} placeholder="显示名" />
            </div>
            <div className="row">
              <select value={nu.role} onChange={(e) => setNu({ ...nu, role: e.target.value })}>
                {ROLES.map((r) => <option key={r} value={r}>{ROLE_ZH[r]}</option>)}
              </select>
              <input type="password" value={nu.password} onChange={(e) => setNu({ ...nu, password: e.target.value })} placeholder="密码" />
              <button disabled={!!busy} onClick={() => void doAdd()}>{busy === "add" ? "新建中…" : "新建用户"}</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
