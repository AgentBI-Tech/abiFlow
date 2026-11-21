# abiFlow

面向电商独立站运营者的人机协同桌面工作台：本地优先，Tauri 2 + React + Rust + SQLite。

状态：**v0.1**（Tauri 壳 + SQLite 12 表建表 + `app_info` IPC）。尚未连接任何真实店铺、支付或订单。

## 设计边界

- 本地优先：业务数据在应用数据目录的 SQLite 中；密钥只进系统密钥链，不落明文。
- 资金动作永不自动执行，始终停在人工确认门；不做合规判定；不跨店比绝对值。
- 端到端验证只用本地 fake MCP server，不连真实支付 / 订单。

## 开发

需要 Node 22+、Rust 1.77+ 与各平台 WebView 依赖。rustup 若用 `--no-modify-path` 安装，先：

```sh
export PATH="$HOME/.cargo/bin:$PATH"
npm install
npm run tauri dev
```

`cargo test` 跑后端单测；`npm run build` 跑前端类型检查与打包。

## 许可

**AGPL-3.0-only** —— 见 [LICENSE](LICENSE)。
