use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::models::{AppError, AppResult};

/// 密钥链抽象（实现设计 §6 / 定稿 §6）：值只进 keystore，库里只有 ref_id。
/// v0.3 框架期用进程内存实现；接真实店铺时替换为 OS 密钥链（libsecret/Keychain/Credential Manager）
/// —— trait 不变，调用方不动。
pub trait KeyStore: Send + Sync {
    fn set(&self, ref_id: &str, value: &str) -> AppResult<()>;
    fn get(&self, ref_id: &str) -> AppResult<Option<String>>;
    fn delete(&self, ref_id: &str) -> AppResult<()>;
    fn list(&self) -> AppResult<Vec<String>>;
}

/// 进程内存 keystore：值只在本进程内存，进程退出即失效；绝不写盘、不进日志。
/// 明确标注为框架期实现——不是 OS 密钥链，不提供重启持久化。
#[derive(Clone, Default)]
pub struct MemoryKeyStore {
    inner: Arc<Mutex<HashMap<String, String>>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }
}

impl KeyStore for MemoryKeyStore {
    fn set(&self, ref_id: &str, value: &str) -> AppResult<()> {
        let mut m = self.inner.lock().map_err(|_| AppError::Db("keystore 锁中毒".into()))?;
        m.insert(ref_id.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, ref_id: &str) -> AppResult<Option<String>> {
        let m = self.inner.lock().map_err(|_| AppError::Db("keystore 锁中毒".into()))?;
        Ok(m.get(ref_id).cloned())
    }

    fn delete(&self, ref_id: &str) -> AppResult<()> {
        let mut m = self.inner.lock().map_err(|_| AppError::Db("keystore 锁中毒".into()))?;
        m.remove(ref_id);
        Ok(())
    }

    fn list(&self) -> AppResult<Vec<String>> {
        let m = self.inner.lock().map_err(|_| AppError::Db("keystore 锁中毒".into()))?;
        let mut v: Vec<String> = m.keys().cloned().collect();
        v.sort();
        Ok(v)
    }
}

/// 只回显引用，不回显值——所有对 keystore 的读都应只拿到 ref。
pub fn redact_ref_list(refs: &[String]) -> Vec<String> {
    refs.iter().map(|_| "[redacted]".to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_delete_roundtrip() {
        let ks = MemoryKeyStore::new();
        ks.set("ga4-key", "sk-secret-123").unwrap();
        assert_eq!(ks.get("ga4-key").unwrap().as_deref(), Some("sk-secret-123"));
        assert!(ks.list().unwrap().contains(&"ga4-key".to_string()));
        ks.delete("ga4-key").unwrap();
        assert_eq!(ks.get("ga4-key").unwrap(), None);
    }

    #[test]
    fn refs_never_expose_values() {
        let refs = vec!["ga4-key".to_string(), "shopify-token".to_string()];
        let shown = redact_ref_list(&refs);
        assert!(shown.iter().all(|s| s == "[redacted]"));
    }

    #[test]
    fn isolated_instances_do_not_share_values() {
        let a = MemoryKeyStore::new();
        let b = MemoryKeyStore::new();
        a.set("k", "secret").unwrap();
        assert_eq!(b.get("k").unwrap(), None, "不同实例不共享密钥");
    }
}
