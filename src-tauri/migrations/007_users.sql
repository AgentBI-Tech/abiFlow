-- abiFlow schema v7（用户管理与权限接入）
-- 本地多用户：用户名唯一，角色决定可见数据面（content/ops/admin/agent → ROLE_PLANES）。
-- 密码不存明文：password_hash 格式为 "salt$hash"（salt 随机十六进制，hash = SHA-256(salt || 明文)）。
-- 全 IF NOT EXISTS，可安全重放。
CREATE TABLE IF NOT EXISTS users (
  id            TEXT PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE,
  display_name  TEXT NOT NULL,
  role          TEXT NOT NULL CHECK(role IN ('content','ops','admin','agent')),
  password_hash TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);
