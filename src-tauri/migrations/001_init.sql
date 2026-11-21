-- abiFlow schema v1（对齐《方案_ABI-Pilot_实现设计》§2）
-- 幂等：全部 CREATE ... IF NOT EXISTS，可安全重放。
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sops (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  version       INTEGER NOT NULL DEFAULT 1,
  trigger_json  TEXT NOT NULL,
  definition    TEXT NOT NULL,
  risk          TEXT NOT NULL CHECK(risk IN ('low','medium','high')),
  owner_policy  TEXT NOT NULL CHECK(owner_policy IN ('human','agent','hybrid')),
  enabled       INTEGER NOT NULL DEFAULT 1,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  UNIQUE(id, version)
);

CREATE TABLE IF NOT EXISTS runs (
  id           TEXT PRIMARY KEY,
  sop_id       TEXT NOT NULL REFERENCES sops(id),
  sop_version  INTEGER NOT NULL,
  status       TEXT NOT NULL CHECK(status IN
               ('pending','running','wait_human','rejected','done','failed','cancelled')),
  mode         TEXT NOT NULL CHECK(mode IN ('plan','build','review')),
  trigger_by   TEXT NOT NULL,
  started_at   INTEGER,
  finished_at  INTEGER,
  verdict      TEXT CHECK(verdict IN ('pass','fail','partial')),
  note         TEXT
);
CREATE INDEX IF NOT EXISTS idx_runs_sop ON runs(sop_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);

CREATE TABLE IF NOT EXISTS steps (
  id           TEXT PRIMARY KEY,
  run_id       TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq          INTEGER NOT NULL,
  node_type    TEXT NOT NULL,
  owner        TEXT NOT NULL CHECK(owner IN ('human','agent')),
  tool_ref     TEXT,
  status       TEXT NOT NULL CHECK(status IN
               ('pending','running','wait_human','done','failed','skipped')),
  input_json   TEXT,
  output_json  TEXT,
  started_at   INTEGER,
  finished_at  INTEGER,
  UNIQUE(run_id, seq)
);

CREATE TABLE IF NOT EXISTS step_logs (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  step_id       TEXT NOT NULL REFERENCES steps(id) ON DELETE CASCADE,
  actor         TEXT NOT NULL CHECK(actor IN ('agent','human')),
  tool_command  TEXT,
  input_redact  TEXT,
  output_redact TEXT,
  verdict       TEXT CHECK(verdict IN ('ok','fail','needs_human')),
  ts            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_logs_step ON step_logs(step_id, ts);

CREATE TABLE IF NOT EXISTS human_gates (
  id           TEXT PRIMARY KEY,
  run_id       TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  step_id      TEXT NOT NULL REFERENCES steps(id) ON DELETE CASCADE,
  reason       TEXT NOT NULL,
  prompt       TEXT NOT NULL,
  payload_json TEXT,
  requested_at INTEGER NOT NULL,
  decided_at   INTEGER,
  decision     TEXT CHECK(decision IN ('approve','reject','input','takeover','cancel')),
  note         TEXT
);
CREATE INDEX IF NOT EXISTS idx_gates_pending ON human_gates(decided_at, requested_at);

CREATE TABLE IF NOT EXISTS acceptance (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id      TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  criterion   TEXT NOT NULL,
  passed      INTEGER NOT NULL DEFAULT 0,
  checked_by  TEXT NOT NULL CHECK(checked_by IN ('rule','llm','human')),
  note        TEXT,
  checked_at  INTEGER
);

CREATE TABLE IF NOT EXISTS tools (
  id          TEXT PRIMARY KEY,
  server      TEXT NOT NULL,
  command     TEXT NOT NULL,
  title       TEXT NOT NULL,
  risk        TEXT NOT NULL CHECK(risk IN ('low','medium','high')),
  scope_json  TEXT NOT NULL,
  schema_json TEXT,
  enabled     INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS tool_grants (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  tool_id     TEXT NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
  mode        TEXT NOT NULL CHECK(mode IN ('plan','build','review')),
  allowed     INTEGER NOT NULL DEFAULT 0,
  planes_json TEXT,
  UNIQUE(tool_id, mode)
);

CREATE TABLE IF NOT EXISTS templates (
  id               TEXT PRIMARY KEY,
  name             TEXT NOT NULL,
  sop_snapshot     TEXT NOT NULL,
  trajectory_json  TEXT NOT NULL,
  corrections_json TEXT NOT NULL,
  source_shop      TEXT,
  validated        INTEGER NOT NULL DEFAULT 0,
  created_at       INTEGER NOT NULL
);

-- 密钥只存引用；真实值在系统密钥链，永不入库。
CREATE TABLE IF NOT EXISTS credentials (
  ref_id      TEXT PRIMARY KEY,
  storage_key TEXT NOT NULL,
  provider    TEXT NOT NULL,
  label       TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS schedules (
  sop_id      TEXT PRIMARY KEY REFERENCES sops(id) ON DELETE CASCADE,
  next_run_at INTEGER,
  last_run_at INTEGER,
  status      TEXT NOT NULL DEFAULT 'idle' CHECK(status IN ('idle','running','error')),
  updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_meta (
  version    INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO schema_meta(version, applied_at) VALUES (1, CAST(strftime('%s','now') AS INTEGER));
