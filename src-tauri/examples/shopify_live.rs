//! 真店只读验收：用真实 abi.db + 真实 mcp_servers.json 跑一条 Shopify SOP。
//! 用法：cargo run --example shopify_live -- <sop_id>
//! 只跑只读 SOP；对真店不发任何写操作。凭据从 secrets.env / shell 读，不打印值。

use std::sync::Arc;

use abi_flow_lib::domain::executor;
use abi_flow_lib::domain::keystore::{KeyStore, MemoryKeyStore};
use abi_flow_lib::domain::llm::MockProvider;
use abi_flow_lib::infra::{app_config, db, store, tool_engine::ToolEngine};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let sop_id = args.get(1).cloned().unwrap_or_else(|| "sop-shopify-storefront".into());
    let data_dir = std::path::PathBuf::from(
        std::env::var("HOME").unwrap_or_default(),
    )
    .join(".local/share/com.agentbi.abiflow/abiflow");

    app_config::set_data_dir(data_dir.clone());
    app_config::load_secrets_env();
    let ks: Arc<dyn KeyStore> = Arc::new(MemoryKeyStore::new());
    let engine = ToolEngine::new(ks);
    let conn = db::open(&data_dir.join("abi.db")).expect("打开真库失败");
    let llm = MockProvider;

    let run_id = executor::start_run(&conn, &sop_id, "build", "probe", &json!({}), &engine, &llm)
        .unwrap_or_else(|e| panic!("启动 {} 失败：{}", sop_id, e));
    println!("RUN_ID={}", run_id);
    let run = store::get_run(&conn, &run_id).expect("读 run 失败");
    println!("STATUS={} NOTE={}", run.status, run.note.unwrap_or_default());
    for s in store::list_steps(&conn, &run_id).expect("读 steps 失败") {
        let out = s.output_json.unwrap_or_default();
        let preview: String = out.chars().take(400).collect();
        println!("STEP {} {} {} => {}", s.node_id.unwrap_or_default(), s.node_type, s.status, preview);
    }
}
