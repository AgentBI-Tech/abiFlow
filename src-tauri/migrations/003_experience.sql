-- ABI-Pilot schema v3（v0.4 经验沉淀）
-- 人工修正样本：某 step 上「原 Agent 输出 vs 人工修正后」的对照，供 owner 升级候选累积。
-- 只记事实对照，不自动放权；资金/不可逆动作由 domain 层硬拦，永不升级。
CREATE TABLE IF NOT EXISTS corrections (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  step_id       TEXT NOT NULL REFERENCES steps(id) ON DELETE CASCADE,
  run_id        TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  node_type     TEXT NOT NULL,
  original_json TEXT NOT NULL,
  corrected_json TEXT NOT NULL,
  note          TEXT,
  author        TEXT NOT NULL CHECK(author IN ('human')),
  created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_corrections_step ON corrections(step_id, created_at);
