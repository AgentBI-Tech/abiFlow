#!/usr/bin/env node
// 内置 browser MCP server（stdio）：换行分隔 JSON-RPC 2.0，走 McpClient 的 StdioTransport。
// 用 Playwright 驱动系统 chromium，维持一个隔离会话。
// 遇到登录/验证码 → 返回 requires_human 标记，由 executor 插 human_gate 转人工（不绕过、不硬闯）。
// stderr 只写日志，stdout 只写 JSON-RPC（不混）。

import { createInterface } from 'node:readline';
import { chromium } from 'playwright';

const CHROME = process.env.ABIFLOW_BROWSER_BIN || '/usr/bin/chromium';
const TOOLS = [
  { name: 'navigate', description: '打开 URL（隔离会话）', inputSchema: { type: 'object', properties: { url: { type: 'string' } }, required: ['url'] } },
  { name: 'snapshot', description: '取当前页面可见文本与标题，并检测登录/验证码', inputSchema: { type: 'object', properties: {} } },
  { name: 'click', description: '点选元素', inputSchema: { type: 'object', properties: { selector: { type: 'string' } }, required: ['selector'] } },
  { name: 'fill', description: '填写输入框', inputSchema: { type: 'object', properties: { selector: { type: 'string' }, value: { type: 'string' } }, required: ['selector', 'value'] } },
  { name: 'close', description: '关闭浏览器会话', inputSchema: { type: 'object', properties: {} } },
];

let browser = null;
let page = null;

function respond(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}
function respondErr(id, message) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, error: { code: -32000, message } }) + '\n');
}
function log(msg) { process.stderr.write('[browser-mcp] ' + msg + '\n'); }

async function ensureBrowser() {
  if (browser && page && !page.isClosed()) return;
  browser = await chromium.launch({ headless: true, executablePath: CHROME, args: ['--no-sandbox'] });
  const ctx = await browser.newContext();
  page = await ctx.newPage();
}

// 启发式检测：登录表单 / 验证码。不绕过，只上报。
async function detectHuman() {
  if (!page || page.isClosed()) return null;
  const info = await page.evaluate(() => {
    const pwd = document.querySelectorAll('input[type=password]').length;
    const captchaInputs = document.querySelectorAll(
      'input[name*=captcha i], input[id*=captcha i], input[placeholder*=验证码], input[placeholder*=captcha i]'
    ).length;
    const text = (document.body?.innerText || '').slice(0, 4000);
    const captchaWord = /验证码|verify you are human|are you a robot|recaptcha|hcaptcha|captcha/i.test(text);
    const loginWord = /log\s?in|sign\s?in|登录|登陆/i.test(text);
    return { pwd, captchaInputs, captchaWord, loginWord };
  });
  if (info.captchaInputs > 0 || info.captchaWord) {
    return { requires_human: true, reason: 'captcha', note: '页面出现验证码，需人工处理；未尝试绕过。' };
  }
  if (info.pwd > 0 && info.loginWord) {
    return { requires_human: true, reason: 'login', note: '页面出现登录表单，需人工登录；未尝试自动登录。' };
  }
  return null;
}

async function callTool(name, args) {
  if (name === 'navigate') {
    const url = String(args.url || '');
    if (!/^https?:\/\//.test(url)) throw new Error('url 必须是 http/https');
    await ensureBrowser();
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30000 });
    const human = await detectHuman();
    const text = await page.evaluate(() => ({ title: document.title, text: (document.body?.innerText || '').slice(0, 6000) }));
    return { content: [{ type: 'text', text: JSON.stringify({ ...text, ...(human || {}), url: page.url() }) }] };
  }
  if (name === 'snapshot') {
    if (!page || page.isClosed()) throw new Error('尚未 navigate');
    const human = await detectHuman();
    const text = await page.evaluate(() => ({ title: document.title, text: (document.body?.innerText || '').slice(0, 6000) }));
    return { content: [{ type: 'text', text: JSON.stringify({ ...text, ...(human || {}), url: page.url() }) }] };
  }
  if (name === 'click') {
    if (!page || page.isClosed()) throw new Error('尚未 navigate');
    await page.click(String(args.selector || ''), { timeout: 10000 });
    const human = await detectHuman();
    return { content: [{ type: 'text', text: JSON.stringify({ clicked: args.selector, ...(human || {}) }) }] };
  }
  if (name === 'fill') {
    if (!page || page.isClosed()) throw new Error('尚未 navigate');
    await page.fill(String(args.selector || ''), String(args.value || ''), { timeout: 10000 });
    const human = await detectHuman();
    return { content: [{ type: 'text', text: JSON.stringify({ filled: args.selector, ...(human || {}) }) }] };
  }
  if (name === 'close') {
    if (browser) { await browser.close(); browser = null; page = null; }
    return { content: [{ type: 'text', text: 'closed' }] };
  }
  throw new Error('未知工具：' + name);
}

const rl = createInterface({ input: process.stdin });
log('browser mcp stdio ready');
rl.on('line', async (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  const method = msg.method;
  const id = msg.id;
  try {
    if (method === 'initialize') {
      respond(id, { protocolVersion: '2025-06-18', capabilities: { tools: {} }, serverInfo: { name: 'builtin-browser', version: '0.1.0' } });
    } else if (method === 'tools/list') {
      respond(id, { tools: TOOLS });
    } else if (method === 'tools/call') {
      const name = msg.params?.name;
      const args = msg.params?.arguments || {};
      respond(id, await callTool(name, args));
    } else if (method === 'notifications/initialized') {
      // notification 无响应
    } else {
      respondErr(id, '未知方法：' + method);
    }
  } catch (e) {
    if (id !== undefined) respondErr(id, String(e.message || e));
    else log('error: ' + String(e.message || e));
  }
});
