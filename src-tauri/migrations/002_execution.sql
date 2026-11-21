-- abiFlow schema v2（v0.2 执行闭环）
-- 给 runs 增加本次执行的状态快照：run 绑定 sop_version + definition，运行中不因 SOP 改版而改变路径。
-- 给 schedules 增加调度间隔（秒），供 scheduler tick 计算下次触发时间。
-- 幂等由 db.rs 列检测保证（SQLite 的 ALTER TABLE ADD COLUMN 无 IF NOT EXISTS）。
ALTER TABLE runs ADD COLUMN definition_snapshot TEXT;
ALTER TABLE runs ADD COLUMN current_node TEXT;
ALTER TABLE runs ADD COLUMN outputs_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE runs ADD COLUMN branch_skipped TEXT NOT NULL DEFAULT '[]';
ALTER TABLE steps ADD COLUMN node_id TEXT;
ALTER TABLE schedules ADD COLUMN interval_secs INTEGER NOT NULL DEFAULT 86400;
