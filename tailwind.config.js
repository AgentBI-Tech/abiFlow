/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        // 深色专业风设计 token（与 styles.css 的 CSS 变量保持一致）
        ink: {
          950: "#0b1220", // 最底背景
          900: "#0f172a", // 页面背景
          850: "#16213a",
          800: "#1e293b", // 面板
          700: "#334155", // 边框
          600: "#475569",
          400: "#94a3b8", // 次要文字
          200: "#cbd5e1",
          100: "#e2e8f0", // 主文字（浅）
        },
        brand: {
          DEFAULT: "#14b8a6", // teal 主色
          deep: "#0d9488",
          soft: "rgba(20,184,166,0.12)",
        },
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "-apple-system", "Segoe UI", "sans-serif"],
        mono: ["JetBrains Mono", "ui-monospace", "SFMono-Regular", "monospace"],
      },
    },
  },
  plugins: [],
};
