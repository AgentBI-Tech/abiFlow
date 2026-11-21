-- abiFlow schema v4（v0.5 实际放权）
-- owner 决策审计：人确认/驳回一次 owner 升级的留痕。
-- from_owner 记「human」，to_owner 记「agent」（驳回时为 human）；资金/不可逆动作由 domain 层硬拦。
-- 真正的 owner 变更落在 SOP 定义节点 config.owner 上（版本递增），本表只留审计事实。
CREATE TABLE IF NOT EXISTS owner_decisions (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  sop_id          TEXT NOT NULL REFERENCES sops(id) ON DELETE CASCADE,
  node_id         TEXT NOT NULL,
  node_type       TEXT NOT NULL,
  from_owner      TEXT NOT NULL,
  to_owner        TEXT NOT NULL,
  decision        TEXT NOT NULL CHECK(decision IN ('approve','reject')),
  correction_count INTEGER NOT NULL DEFAULT 0,
  note            TEXT,
  decided_by      TEXT NOT NULL DEFAULT 'human',
  created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_owner_decisions_sop ON owner_decisions(sop_id, created_at);
