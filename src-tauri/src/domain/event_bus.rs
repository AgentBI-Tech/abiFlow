//! 事件订阅触发（系统设计 §5.1「触发：cron + 事件订阅 + 手动」的第二类）。
//!
//! 与 `infra/events.rs` 的区别：events.rs 是「后端→前端」的 UI 刷新提示；
//! 这里处理的是「外部事件→启动 run」的调度器第二类触发。
//!
//! EventSource 是可插拔接口：当前用 InMemoryEventSource（进程内队列 + 假事件注入）跑通闭环，
//! 以后接 MCP 推送 / webhook / DB 轮询只需换实现，调度器与核心不动。
//! 去重：同一 event_type 在 DEDUP_WINDOW_SECS 内重复 emit 只入队一次，防重复事件刷 run。

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use serde_json::Value;

use crate::models::now_secs;

/// 去重窗口：同类型事件 60 秒内只消费一次。
pub const DEDUP_WINDOW_SECS: i64 = 60;

#[derive(Clone, Debug)]
pub struct Event {
    pub event_type: String,
    pub payload: Value,
    pub occurred_at: i64,
}

/// 可插拔事件源。emit 注入事件，drain 取出待处理事件（消费后清空）。
pub trait EventSource: Send + Sync {
    fn emit(&self, event_type: &str, payload: Value);
    fn drain(&self) -> Vec<Event>;
}

/// 进程内事件源：假事件从 IPC 注入，scheduler tick 时 drain。
/// 重启后队列丢失（假事件源，符合当前阶段定位）；真事件源以后换实现即可。
pub struct InMemoryEventSource {
    queue: Mutex<VecDeque<Event>>,
    last_seen: Mutex<HashMap<String, i64>>,
}

impl InMemoryEventSource {
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            last_seen: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSource for InMemoryEventSource {
    fn emit(&self, event_type: &str, payload: Value) {
        let now = now_secs();
        // 去重：同类型在窗口内已注入过则忽略（不覆盖 payload，保持首个事件为准）。
        {
            let mut seen = match self.last_seen.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if let Some(t) = seen.get(event_type) {
                if now.saturating_sub(*t) < DEDUP_WINDOW_SECS {
                    return;
                }
            }
            seen.insert(event_type.to_string(), now);
        }
        if let Ok(mut q) = self.queue.lock() {
            q.push_back(Event {
                event_type: event_type.to_string(),
                payload,
                occurred_at: now,
            });
        }
    }

    fn drain(&self) -> Vec<Event> {
        match self.queue.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn emit_then_drain_returns_event() {
        let src = InMemoryEventSource::new();
        src.emit("new_order", json!({"id": "o1"}));
        let events = src.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "new_order");
        assert_eq!(events[0].payload["id"], "o1");
        assert!(src.drain().is_empty(), "drain 后应清空");
    }

    #[test]
    fn dedup_same_type_within_window() {
        let src = InMemoryEventSource::new();
        src.emit("stock_change", json!({"sku": "a"}));
        src.emit("stock_change", json!({"sku": "b"}));
        let events = src.drain();
        assert_eq!(events.len(), 1, "同类型 60s 内应只入队一次");
        assert_eq!(events[0].payload["sku"], "a", "保留首个事件");
    }

    #[test]
    fn different_types_both_queued() {
        let src = InMemoryEventSource::new();
        src.emit("new_order", json!({}));
        src.emit("stock_change", json!({}));
        let events = src.drain();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn same_type_after_window_is_allowed() {
        let src = InMemoryEventSource::new();
        src.emit("new_order", json!({"n": 1}));
        // 人为把 last_seen 推到窗口之外。
        {
            let mut seen = src.last_seen.lock().unwrap();
            seen.insert("new_order".into(), now_secs() - DEDUP_WINDOW_SECS - 1);
        }
        src.emit("new_order", json!({"n": 2}));
        let events = src.drain();
        assert_eq!(events.len(), 2, "超出去重窗口后应可再次入队");
    }
}
