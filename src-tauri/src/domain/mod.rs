// 领域层：执行闭环逻辑，尽量与 UI/网络解耦，cargo test 可测。
pub mod audit;
pub mod cron;
pub mod examples;
pub mod executor;
pub mod experience;
pub mod gateway;
pub mod keystore;
pub mod llm;
pub mod scheduler;
pub mod sop;
