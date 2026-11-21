// 桌面入口。所有逻辑在 lib.rs，便于单测（cargo test 不启动 GUI）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    abi_flow_lib::run()
}
