import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 2 约定：固定端口 1420，HMR 指向同一端口，关闭 clearScreen 避免日志清屏。
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    hmr: { protocol: "ws", host: "127.0.0.1", port: 1420 },
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: { target: "es2022" },
});
