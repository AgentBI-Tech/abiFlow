use serde::{Deserialize, Serialize};

/// 前端展示用的应用信息（app_info 命令返回）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub db_path: String,
    pub table_count: usize,
}

/// 统一错误：前端按 code 决定提示与跳转（对齐实现设计 §3.1）。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("版本冲突：SOP 已被他人改动")]
    VersionConflict,
    #[error("需要人工确认后才能继续")]
    GateRequired,
    #[error("工具未授权：{0}")]
    ToolDenied(String),
    #[error("高风险动作被拦截：{0}")]
    RiskBlocked(String),
    #[error("MCP 服务不可用：{0}")]
    McpUnavailable(String),
    #[error("LLM 调用失败：{0}")]
    LlmError(String),
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("输入非法：{0}")]
    InvalidInput(String),
    #[error("数据库错误：{0}")]
    Db(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let (code, msg) = match self {
            AppError::VersionConflict => ("VersionConflict", self.to_string()),
            AppError::GateRequired => ("GateRequired", self.to_string()),
            AppError::ToolDenied(m) => ("ToolDenied", format!("工具未授权：{}", m)),
            AppError::RiskBlocked(m) => ("RiskBlocked", format!("高风险动作被拦截：{}", m)),
            AppError::McpUnavailable(m) => ("McpUnavailable", format!("MCP 服务不可用：{}", m)),
            AppError::LlmError(m) => ("LlmError", format!("LLM 调用失败：{}", m)),
            AppError::NotFound(m) => ("NotFound", format!("未找到：{}", m)),
            AppError::InvalidInput(m) => ("InvalidInput", format!("输入非法：{}", m)),
            AppError::Db(m) => ("Db", format!("数据库错误：{}", m)),
        };
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("code", code)?;
        map.serialize_entry("message", &msg)?;
        map.end()
    }
}

pub type AppResult<T> = Result<T, AppError>;
