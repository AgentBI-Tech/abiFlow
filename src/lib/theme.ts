// 主题管理：浅色（默认）/ 深色双主题。
// 持久化双写：localStorage 保证首屏无闪屏（main.tsx 在 React 挂载前同步读），
// 后端 config（config_get/config_set 的 "theme" 键）跨重启/机器权威。
// 组件只调 applyTheme / loadBackendTheme / saveTheme，不直接碰 document 或存储细节。

export type Theme = "light" | "dark";

const LS_KEY = "abiflow.theme";

/** 立即把主题写到 <html data-theme>，CSS token 随之切换。同步、可在 React 挂载前调用。 */
export function applyTheme(theme: Theme): void {
  document.documentElement.setAttribute("data-theme", theme);
}

/** 首屏同步读本地主题（main.tsx 用，防闪屏）；无记录默认 light。 */
export function initThemeFromStorage(): Theme {
  const v = localStorage.getItem(LS_KEY);
  const theme: Theme = v === "dark" ? "dark" : "light";
  applyTheme(theme);
  return theme;
}

/** 当前生效主题（从 DOM 读，单一真相）。 */
export function currentTheme(): Theme {
  return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}

/** 切换并双写：本地立即生效、localStorage 落盘、后端 config 异步持久化。 */
export async function saveTheme(
  theme: Theme,
  configSet: (k: string, v: string) => Promise<void>,
): Promise<void> {
  applyTheme(theme);
  localStorage.setItem(LS_KEY, theme);
  await configSet("theme", theme);
}

/** 用后端 config 的 theme 值校正本地（App 挂载后调）；缺值/非法则不覆盖本地。 */
export function themeFromBackend(cfg: Record<string, string>): Theme | null {
  const v = cfg.theme;
  return v === "dark" || v === "light" ? v : null;
}
