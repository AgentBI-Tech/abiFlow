// 用户管理与登录（2026-11-24）：左下角登录入口的完整面板。
// 本地多用户 + 角色 + 密码：角色决定可见数据面（content/ops/admin/agent）。
// 当前登录用户由后端进程内存维护，重启需重新登录；左下角展示登录态。
// 管理权限：仅 admin 可建/改/删/改角色/重置密码；users 表为空时首个用户可自定角色。
// 后端 user.rs::require_admin 是真正边界，前端隐藏只是体验。

import { useEffect, useState } from "react";
import {
  userList, userCurrent, userAdd, userLogin, userLogout, userRemove,
  userUpdate, userResetPassword, errMessage,
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
  const [loginUser, setLoginUser] = useState("");
  const [loginPw, setLoginPw] = useState("");
  const [nu, setNu] = useState({ username: "", displayName: "", role: "content", password: "" });
  const [editing, setEditing] = useState<string | null>(null);
  const [eu, setEu] = useState({ displayName: "", role: "content", newPassword: "" });

  // 是否允许管理：admin 登录，或当前无任何用户（首个用户可自定角色）。
  const canManage = current?.role === "admin" || users.length === 0;

  const load = async () => {
    try {
      const [list, cur] = await Promise.all([userList(), userCurrent()]);
      setUsers(list);
      setCurrent(cur);
      onUserChange(cur);
      if (!loginUser && list.length) setLoginUser(list[0].username);
      // 首个用户强制管理员：表空时把新建角色固定为 admin。
      if (list.length === 0) setNu((v) => ({ ...v, role: "admin" }));
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

  const startEdit = (u: UserRow) => {
    setEditing(u.id);
    setEu({ displayName: u.displayName, role: u.role, newPassword: "" });
  };

  const doUpdate = (u: UserRow) => act("update", async () => {
    if (!eu.displayName.trim()) { onError("显示名不能为空"); return; }
    await userUpdate(u.id, eu.displayName.trim(), eu.role);
    if (eu.newPassword) await userResetPassword(u.id, eu.newPassword);
    setEditing(null);
    setEu({ displayName: "", role: "content", newPassword: "" });
  });

  return (
    <div className="card" data-user-panel="1">
      <div className="row">
        <strong>用户与权限</strong>
        <span className="count">{users.length} 个本地用户</span>
        {current && <span className="pill">当前：{current.displayName}（{ROLE_ZH[current.role] ?? current.role}）</span>}
      </div>
      <p className="sub-row">
        角色决定可见数据面：内容角色看不到进价（cost 面），运营看不到财务面；资金/不可逆动作永远人工审核。
        仅管理员可建/改/删用户。
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
            <button disabled={!!busy} onClick={() => setOpen((v) => !v)}>{open ? "收起" : "登录 / 管理"}</button>
          </>
        )}
      </div>

      {open && (
        <div className="stack" style={{ marginTop: 14 }}>
          {/* 登录：选本地用户 + 密码 */}
          <div className="stack" data-login-form="1">
            <strong>登录</strong>
            <label className="stack">用户（本地账号）
              <select value={loginUser} onChange={(e) => setLoginUser(e.target.value)}>
                {users.length === 0 && <option value="">（暂无用户，请先新建）</option>}
                {users.map((u) => <option key={u.id} value={u.username}>{u.displayName}（@{u.username}）</option>)}
              </select>
            </label>
            <label className="stack">密码
              <input type="password" value={loginPw} onChange={(e) => setLoginPw(e.target.value)} placeholder="登录密码" />
            </label>
            <button disabled={!!busy || users.length === 0} onClick={() => void doLogin()}>
              {busy === "login" ? "登录中…" : "登录"}
            </button>
          </div>

          {!canManage && (
            <p className="sub-row" data-manage-denied="1">你不是管理员，仅能查看用户列表；建/改/删需管理员登录后操作。</p>
          )}

          {/* 用户列表 + 编辑 / 删除（仅 admin） */}
          <div className="stack" data-user-list="1" style={{ marginTop: 10 }}>
            <strong>本地用户（{users.length}）</strong>
            {users.length === 0 ? <p className="empty">暂无用户，先在下方新建第一个管理员。</p> : users.map((u) => (
              <div className="stack" key={u.id} data-user-row={u.id} style={{ gap: 6 }}>
                <div className="row">
                  <span style={{ flex: "1 1 auto" }}>{u.displayName}（@{u.username}）· {ROLE_ZH[u.role] ?? u.role}</span>
                  {canManage && (
                    <>
                      <button disabled={!!busy} onClick={() => startEdit(u)}>编辑</button>
                      <button disabled={!!busy || current?.id === u.id} onClick={() => void act("remove", () => userRemove(u.id))}>删除</button>
                    </>
                  )}
                </div>
                {editing === u.id && (
                  <div className="stack" style={{ marginLeft: 12 }} data-edit-form={u.id}>
                    <label className="stack">显示名
                      <input value={eu.displayName} onChange={(e) => setEu({ ...eu, displayName: e.target.value })} />
                    </label>
                    <label className="stack">角色{u.role === "admin" && users.filter((x) => x.role === "admin").length <= 1 ? "（唯一管理员不可降级）" : ""}
                      <select
                        value={eu.role}
                        disabled={u.role === "admin" && users.filter((x) => x.role === "admin").length <= 1}
                        onChange={(e) => setEu({ ...eu, role: e.target.value })}
                      >
                        {ROLES.map((r) => <option key={r} value={r}>{ROLE_ZH[r]}</option>)}
                      </select>
                    </label>
                    <label className="stack">重置密码（留空不改）
                      <input type="password" value={eu.newPassword} onChange={(e) => setEu({ ...eu, newPassword: e.target.value })} placeholder="留空则不改密码" />
                    </label>
                    <div className="row">
                      <button disabled={!!busy} onClick={() => void doUpdate(u)}>{busy === "update" ? "保存中…" : "保存"}</button>
                      <button disabled={!!busy} onClick={() => setEditing(null)}>取消</button>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </div>

          {/* 新建用户（仅 admin） */}
          {canManage && (
            <div className="stack" data-user-add="1" style={{ marginTop: 10 }}>
              <strong>新建用户</strong>
              <label className="stack">用户名（唯一）
                <input value={nu.username} onChange={(e) => setNu({ ...nu, username: e.target.value })} placeholder="username" />
              </label>
              <label className="stack">显示名
                <input value={nu.displayName} onChange={(e) => setNu({ ...nu, displayName: e.target.value })} placeholder="选填，默认同用户名" />
              </label>
              <label className="stack">角色{users.length === 0 ? "（首个用户强制管理员）" : ""}
                <select
                  value={users.length === 0 ? "admin" : nu.role}
                  disabled={users.length === 0}
                  onChange={(e) => setNu({ ...nu, role: e.target.value })}
                >
                  {(users.length === 0 ? ["admin"] : ROLES).map((r) => <option key={r} value={r}>{ROLE_ZH[r]}</option>)}
                </select>
              </label>
              <label className="stack">密码
                <input type="password" value={nu.password} onChange={(e) => setNu({ ...nu, password: e.target.value })} placeholder="登录密码" />
              </label>
              <button disabled={!!busy} onClick={() => void doAdd()}>{busy === "add" ? "新建中…" : "新建用户"}</button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
