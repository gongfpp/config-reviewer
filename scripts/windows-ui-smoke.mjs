// Windows runner 上启动真实已安装 EXE / WebView2；仅文件保存对话框返回值被替换。
import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';

const executable = process.argv[2];
if (!executable) throw new Error('Expected installed EXE path');
const root = await mkdtemp(path.join(tmpdir(), 'reviewer-ui-'));
const inputs = path.join(root, '中文发布包'); await mkdir(inputs);
await writeFile(path.join(inputs, 'apollo-config.json'), '{"retry":3}', 'utf8');
const reply = '有条件通过\n【依据】\n- 已配置重试\n【风险】\n- 缺少监控告警\n【整改项】\n- 补充告警阈值\n引用：$.retry | 3';
let requestCount = 0;
const server = http.createServer(async (req, res) => {
  let body = ''; for await (const piece of req) body += piece;
  const value = JSON.parse(body); assert.equal(req.headers.authorization, 'Bearer mock-key'); requestCount++;
  const text = value.messages[0].content.includes('回复两个字') ? '可用' : reply;
  if (value.stream) {
    res.writeHead(200, { 'Content-Type': 'text/event-stream' });
    res.end(`data: ${JSON.stringify({ choices: [{ delta: { content: text }, finish_reason: 'stop' }] })}\n\ndata: [DONE]\n\n`);
  } else { res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify({ choices: [{ message: { content: text }, finish_reason: 'stop' }] })); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const gateway = `http://127.0.0.1:${server.address().port}/v1`;
const artifacts = path.resolve('artifacts/ui-smoke'); await mkdir(artifacts, { recursive: true });
let app, browser, page;
const errors = [];
async function launch() {
  app = spawn(executable, [], { env: { ...process.env, APPDATA: path.join(root, 'appdata'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9222', WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'webview') } });
  for (let i = 0; i < 60; i++) {
    try { browser = await chromium.connectOverCDP('http://127.0.0.1:9222'); break; }
    catch { await new Promise(resolve => setTimeout(resolve, 500)); }
  }
  if (!browser) throw new Error('WebView2 did not expose CDP');
  for (let i = 0; i < 30; i++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => !p.url().startsWith('devtools:'));
    if (page) break; await new Promise(resolve => setTimeout(resolve, 200));
  }
  if (!page) throw new Error('Application page not found');
  page.on('pageerror', error => errors.push(error.message));
  await page.getByRole('button', { name: /模型与规则/ }).waitFor();
}
async function stop() {
  if (browser) { await browser.close(); browser = undefined; }
  if (app) { try { execFileSync('taskkill', ['/pid', String(app.pid), '/T', '/F']); } catch {} app = undefined; }
}
try {
  await launch();
  await page.getByRole('button', { name: /模型与规则/ }).click();
  await page.getByPlaceholder('http://llm-gateway.intra.corp/v1').fill(gateway);
  await page.getByPlaceholder('例如 deepseek-v4-flash').fill('mock-model');
  await page.locator('input[type=password]').fill('mock-key');
  await page.getByRole('button', { name: '保存设置', exact: true }).first().click();
  await page.getByText('设置已保存', { exact: true }).waitFor();
  await page.getByRole('button', { name: '测试连接', exact: true }).click();
  await page.getByText('连接正常：可用', { exact: true }).waitFor();
  await page.getByRole('button', { name: /新建评审/ }).click();
  await page.getByRole('button', { name: '扫描本地目录', exact: true }).click();
  await page.getByPlaceholder('D:\\release\\2026Q3-mixed').fill(inputs);
  await page.getByRole('button', { name: '下一步：扫描解析 →', exact: true }).click();
  await page.getByText('apollo-config.json', { exact: true }).first().waitFor();
  await page.getByRole('button', { name: '预览', exact: true }).click();
  assert.match(await page.locator('.preview-box').textContent().catch(() => page.locator('body').textContent()), /retry/);
  await page.getByRole('button', { name: '下一步：评审配置 →', exact: true }).click();
  await page.getByRole('button', { name: '开始评审', exact: true }).click();
  await page.getByRole('heading', { name: '确认数据去向' }).waitFor();
  await page.getByRole('button', { name: '确认并开始', exact: true }).click();
  await page.getByText('评审完成', { exact: true }).waitFor();
  await page.getByRole('button', { name: '详情', exact: true }).click();
  await page.getByText('补充告警阈值', { exact: true }).first().waitFor();
  await page.screenshot({ path: path.join(artifacts, 'results.png'), fullPage: true });
  for (const [format, extension] of [['JSON', 'json'], ['Markdown', 'md'], ['HTML', 'html'], ['Excel', 'xlsx']]) {
    const filename = path.join(root, `报告.${extension}`);
    await page.evaluate(filename => {
      const original = window.__TAURI_INTERNALS__.invoke;
      window.__TAURI_INTERNALS__.invoke = (command, args, options) => command === 'plugin:dialog|save' ? Promise.resolve(filename) : original(command, args, options);
    }, filename);
    await page.getByRole('button', { name: `导出 ${format}`, exact: true }).click();
    for (let i = 0; i < 40; i++) {
      try { await readFile(filename); break; } catch { await new Promise(resolve => setTimeout(resolve, 100)); }
    }
    const data = await readFile(filename); assert.ok(data.length > 0);
    if (extension === 'json') assert.equal(JSON.parse(data.toString())[0].verdict, '有条件通过');
    if (extension === 'xlsx') assert.equal(data.subarray(0, 2).toString(), 'PK');
  }
  await stop(); await launch();
  await page.getByRole('button', { name: /评审历史/ }).click();
  await page.getByText(inputs, { exact: false }).waitFor();
  await page.getByRole('button', { name: '查看', exact: true }).click();
  await page.getByText('apollo-config.json', { exact: true }).first().waitFor();
  await page.getByRole('button', { name: /模型与规则/ }).click();
  await page.getByRole('button', { name: '测试连接', exact: true }).click();
  await page.getByText('连接正常：可用', { exact: true }).waitFor();
  assert.equal(errors.length, 0, errors.join('\n')); assert.ok(requestCount >= 3);
  await writeFile(path.join(artifacts, 'result.json'), JSON.stringify({ passed: true, requestCount,
    checked: ['installed EXE', 'WebView2', 'settings and key persistence', 'stream connection', 'Chinese directory', 'scan preview', 'review', 'details', 'four report exports', 'restart history'],
    limitation: 'Save dialog response was stubbed; other Tauri IPC and file writes used the real installed application.' }, null, 2));
  console.log('Windows installed application UI smoke passed');
} catch (error) {
  if (page) { await page.screenshot({ path: path.join(artifacts, 'failure.png') }).catch(() => {}); await writeFile(path.join(artifacts, 'failure.txt'), String(error)); }
  throw error;
} finally { await stop(); server.close(); }
