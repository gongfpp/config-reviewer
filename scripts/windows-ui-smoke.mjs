// Windows runner 上启动真实已安装 EXE / WebView2；仅文件保存对话框返回值被替换。
import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, readFile, copyFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';

const executable = process.argv[2];
if (!executable) throw new Error('Expected installed EXE path');
const root = await mkdtemp(path.join(tmpdir(), 'reviewer-ui-'));
const inputs = path.join(root, '中文发布包'); await mkdir(inputs);
const moduleName = '阿波罗验收';
const docName = `${moduleName}操作手册.DOC`;
const textName = `${moduleName}.txt`;
const inputInstructions = 'QA_INPUT_CURRENT：输入为发布配置与操作手册，缺失资料需明确指出。';
const reviewInstructions = 'QA_REVIEW_CURRENT：检查订单同步任务的重试、告警与回滚措施。';
const outputInstructions = 'QA_OUTPUT_CURRENT：列出依据、风险、整改项和原文引用。';
await copyFile(path.resolve('src-tauri/tests/fixtures/legacy-word.doc'), path.join(inputs, docName));
await writeFile(path.join(inputs, textName), 'QA_TEXT_CONTENT：订单同步 orderSyncJob 已配置 3 次重试，尚缺监控告警。');
const reviewedInputs = new Set();
const gatewayErrors = [];
const reply = '有条件通过\n【依据】\n- 已配置重试\n【风险】\n- 缺少监控告警\n【整改项】\n- 补充告警阈值\n引用：订单同步 | orderSyncJob | 3';
let requestCount = 0;
const server = http.createServer(async (req, res) => {
  try {
    let body = ''; for await (const piece of req) body += piece;
    const value = JSON.parse(body); assert.equal(req.headers.authorization, 'Bearer mock-key'); requestCount++;
    const connectivity = value.messages[0].content.includes('回复两个字');
    const text = connectivity ? '可用' : reply;
    if (!connectivity) {
      const prompt = value.messages.map(message => message.content).join('\n');
      assert.ok(prompt.includes('orderSyncJob') && prompt.includes('订单同步'), 'Parsed input must reach the gateway');
      for (const expected of [inputInstructions, reviewInstructions, outputInstructions]) {
        assert.ok(prompt.includes(expected), `Current module specification missing from gateway request: ${expected}`);
      }
      assert.ok(!prompt.includes('QA_OUTPUT_OLD'), 'Saved module edits must replace the old specification');
      if (prompt.includes(docName)) reviewedInputs.add(docName);
      if (prompt.includes('QA_TEXT_CONTENT')) reviewedInputs.add(textName);
      await new Promise(resolve => setTimeout(resolve, 1500));
    }
    if (value.stream) {
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.end(`data: ${JSON.stringify({ choices: [{ delta: { content: text }, finish_reason: 'stop' }] })}\n\ndata: [DONE]\n\n`);
    } else { res.writeHead(200, { 'Content-Type': 'application/json' }); res.end(JSON.stringify({ choices: [{ message: { content: text }, finish_reason: 'stop' }] })); }
  } catch (error) {
    gatewayErrors.push(String(error));
    res.writeHead(500, { 'Content-Type': 'application/json' }); res.end(JSON.stringify({ error: String(error) }));
  }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const gateway = `http://127.0.0.1:${server.address().port}/v1`;
const artifacts = path.resolve('artifacts/ui-smoke'); await mkdir(artifacts, { recursive: true });
let app, browser, page;
let appOutput = '';
const errors = [];
async function launch() {
  app = spawn(executable, ['--qa-data-dir', path.join(root, 'appdata')], { env: { ...process.env, PATH: process.env.REVIEWER_RUNTIME_PATH || process.env.PATH, APPDATA: path.join(root, 'appdata'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: '--remote-debugging-port=9222', WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'webview') } });
  app.stdout?.on('data', data => { appOutput += data.toString(); });
  app.stderr?.on('data', data => { appOutput += data.toString(); });
  app.on('error', error => { appOutput += String(error); });
  for (let i = 0; i < 60; i++) {
    try { browser = await chromium.connectOverCDP('http://127.0.0.1:9222'); break; }
    catch { await new Promise(resolve => setTimeout(resolve, 500)); }
  }
  if (!browser) {
    await writeFile(path.join(artifacts, 'app-startup.txt'), `Exit code: ${app.exitCode}\n${appOutput}`);
    console.log(`App startup diagnostics: exit=${app.exitCode}\n${appOutput}`);
    throw new Error('WebView2 did not expose CDP');
  }
  for (let i = 0; i < 30; i++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => !p.url().startsWith('devtools:'));
    if (page) break; await new Promise(resolve => setTimeout(resolve, 200));
  }
  if (!page) throw new Error('Application page not found');
  page.on('pageerror', error => errors.push(error.message));
  await page.getByRole('button', { name: /模型与运行/ }).waitFor();
  if (process.env.REVIEWER_EXPECT_SOURCE_EDIT) {
    assert.ok((await page.locator('body').textContent()).includes(process.env.REVIEWER_EXPECT_SOURCE_EDIT), 'The rebuilt application must show the edited source text');
  }
}
async function stop() {
  if (browser) { await browser.close(); browser = undefined; }
  if (app) { try { execFileSync('taskkill', ['/pid', String(app.pid), '/T', '/F']); } catch {} app = undefined; }
}
async function openModule() {
  await page.getByRole('button', { name: /评审模块/ }).first().click();
  await page.getByRole('button', { name: new RegExp(`${moduleName}.*条启用的匹配条件`) }).click();
  await expandModuleAdvanced();
}
async function expandModuleAdvanced() {
  for (const label of ['输入说明', '输出补充要求']) {
    const details = page.locator('details.module-advanced').filter({ has: page.getByLabel(label, { exact: true }) });
    if (await details.getAttribute('open') === null) await details.locator('summary').click();
  }
}
async function saveModule() {
  await page.getByRole('button', { name: '保存模块配置', exact: true }).click();
  await page.getByRole('status').filter({ hasText: '评审模块已保存，后续扫描和评审使用此规范。' }).waitFor();
}
async function scanInputs() {
  await page.getByRole('button', { name: /新建评审/ }).click();
  await page.getByRole('button', { name: '扫描本地目录', exact: true }).click();
  await page.locator('input[placeholder*="2026Q3-mixed"]').fill(inputs);
  await page.getByRole('button', { name: '下一步：扫描解析 →', exact: true }).click();
  for (const filename of [docName, textName]) {
    const row = page.getByRole('row').filter({ has: page.getByText(filename, { exact: true }) });
    await row.getByText(moduleName, { exact: true }).waitFor();
    assert.equal(await row.locator('select').count(), 0, 'File rows must not independently select a template');
    assert.equal(await row.getByRole('checkbox').isChecked(), true, 'Both supported extensions must match the module');
  }
  assert.equal(requestCount, scanRequestBaseline, 'Scanning must never send file content to the model');
}
let scanRequestBaseline = 0;
try {
  await launch();
  await page.getByRole('button', { name: /模型与运行/ }).click();
  await page.getByPlaceholder('http://llm-gateway.intra.corp/v1').fill(gateway);
  await page.getByPlaceholder('例如 deepseek-v4-flash').fill('mock-model');
  await page.locator('input[type=password]').fill('mock-key');
  await page.getByRole('button', { name: '保存设置', exact: true }).first().click();
  await page.getByText('设置已保存', { exact: true }).waitFor();
  await page.getByRole('button', { name: '测试连接', exact: true }).click();
  await page.getByText('连接正常：可用', { exact: true }).waitFor();
  await page.getByRole('button', { name: /评审模块/ }).first().click();
  await page.getByRole('button', { name: '新建模块', exact: true }).click();
  for (const heading of ['1 文件匹配', '2 评审要求', '3 输出结果']) {
    await page.getByRole('heading', { name: heading, exact: true }).waitFor();
  }
  assert.equal(await page.getByLabel('输入说明', { exact: true }).isVisible(), false, 'Optional input instructions should start collapsed');
  assert.equal(await page.getByLabel('输出补充要求', { exact: true }).isVisible(), false, 'Optional output instructions should start collapsed');
  await expandModuleAdvanced();
  await page.getByLabel('模块名称', { exact: true }).fill(moduleName);
  await page.getByRole('button', { name: '添加匹配条件', exact: true }).click();
  await page.getByLabel('匹配方式 1', { exact: true }).selectOption('keyword');
  await page.getByLabel('匹配条件 1', { exact: true }).fill(moduleName);
  await page.getByLabel('输入说明', { exact: true }).fill(inputInstructions);
  await page.getByLabel('评审要求', { exact: true }).fill(reviewInstructions);
  await page.getByLabel('输出格式', { exact: true }).selectOption('standard');
  await page.getByLabel('输出补充要求', { exact: true }).fill('QA_OUTPUT_OLD');
  await saveModule();
  scanRequestBaseline = requestCount;
  await scanInputs();
  await openModule();
  await page.getByLabel('输出补充要求', { exact: true }).fill(outputInstructions);
  await saveModule();
  await page.screenshot({ path: path.join(artifacts, 'review-module.png'), fullPage: true });
  await scanInputs();
  await page.getByRole('row').filter({ has: page.getByText(docName, { exact: true }) }).getByRole('button', { name: '预览', exact: true }).click();
  assert.match(await page.locator('.preview-box').textContent(), /orderSyncJob/);
  await page.screenshot({ path: path.join(artifacts, 'module-file-matching.png'), fullPage: true });
  await page.getByRole('button', { name: '下一步：评审配置 →', exact: true }).click();
  await page.getByRole('button', { name: '开始评审', exact: true }).click();
  await page.getByRole('heading', { name: '确认数据去向' }).waitFor();
  await page.getByRole('button', { name: '确认并开始', exact: true }).click();
  await page.getByRole('button', { name: '取消评审', exact: true }).waitFor();
  for (let i = 0; i < 100 && requestCount < 2; i++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.ok(requestCount >= 2, 'Review must reach the gateway before overlapping command probes');
  const overlap = await page.evaluate(async ({ inputs, gateway }) => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const errors = [];
    try { await invoke('run_review', { request: { task_id: 'overlap-probe', files: [], cfg: { base_url: gateway, model: 'mock-model', api_key: '', allow_http: true, timeout_secs: 3 } } }); }
    catch (error) { errors.push(String(error)); }
    try { await invoke('scan_directory', { path: inputs, ruleSet: { id: 'probe', name: 'probe', builtin: false, rules: [] }, recursive: true, matchedOnly: false, threshold: 12000, chunkSize: 8000 }); }
    catch (error) { errors.push(String(error)); }
    return errors;
  }, { inputs, gateway });
  assert.equal(overlap.length, 2); assert.ok(overlap.every(error => error.includes('评审正在运行')));
  await page.getByText('评审完成', { exact: true }).waitFor();
  assert.deepEqual([...reviewedInputs].sort(), [docName, textName].sort(), 'Both DOC and TXT must be reviewed with the module specification');
  await page.getByRole('button', { name: '详情', exact: true }).first().click();
  await page.getByText('补充告警阈值', { exact: true }).first().waitFor();
  await page.screenshot({ path: path.join(artifacts, 'results.png'), fullPage: true });
  for (const [format, extension] of [['JSON', 'json'], ['Markdown', 'md'], ['HTML', 'html'], ['Excel', 'xlsx']]) {
    const filename = path.join(root, `报告.${extension}`);
    const hitsBefore = await page.evaluate(() => window.__QA_DIALOG_HITS__ ?? 0);
    await page.evaluate(filename => {
      const originalFetch = window.fetch.bind(window);
      window.fetch = (input, options) => {
        const url = typeof input === 'string' ? input : input.url;
        if (decodeURIComponent(url).includes('plugin:dialog|save')) {
          window.__QA_DIALOG_HITS__ = (window.__QA_DIALOG_HITS__ ?? 0) + 1;
          return Promise.resolve(new Response(JSON.stringify(filename), {
            status: 200, headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' }
          }));
        }
        return originalFetch(input, options);
      };
    }, filename);
    await page.getByRole('button', { name: `导出 ${format}`, exact: true }).click();
    for (let i = 0; i < 40; i++) {
      try { await readFile(filename); break; } catch { await new Promise(resolve => setTimeout(resolve, 100)); }
    }
    const hitsAfter = await page.evaluate(() => window.__QA_DIALOG_HITS__ ?? 0);
    assert.equal(hitsAfter, hitsBefore + 1, 'Save dialog response stub must be used exactly once');
    const data = await readFile(filename); assert.ok(data.length > 0);
    if (extension === 'json') {
      const reports = JSON.parse(data.toString()); assert.equal(reports.length, 2);
      assert.ok(reports.every(report => report.verdict === '有条件通过'));
    }
    if (extension === 'xlsx') assert.equal(data.subarray(0, 2).toString(), 'PK');
  }
  await stop(); await launch();
  await page.getByRole('button', { name: /评审历史/ }).click();
  await page.getByText(inputs, { exact: false }).waitFor();
  await page.getByRole('button', { name: '查看', exact: true }).click();
  await page.getByText(docName, { exact: true }).first().waitFor();
  await openModule();
  for (const [label, value] of [['模块名称', moduleName], ['标识颜色', '#3a5a78'], ['匹配条件 1', moduleName],
    ['输入说明', inputInstructions], ['评审要求', reviewInstructions], ['输出补充要求', outputInstructions], ['输出格式', 'standard']]) {
    assert.equal(await page.getByLabel(label, { exact: true }).inputValue(), value, `Module field must persist after restart: ${label}`);
  }
  await page.getByRole('button', { name: /模型与运行/ }).click();
  await page.getByRole('button', { name: '测试连接', exact: true }).click();
  await page.getByText('连接正常：可用', { exact: true }).waitFor();
  scanRequestBaseline = requestCount;
  await scanInputs();
  const requestsBeforeCancel = requestCount;
  await page.getByRole('button', { name: '下一步：评审配置 →', exact: true }).click();
  await page.getByRole('button', { name: '开始评审', exact: true }).click();
  await page.getByRole('button', { name: '确认并开始', exact: true }).click();
  await page.getByRole('button', { name: '取消评审', exact: true }).waitFor();
  for (let i = 0; i < 100 && requestCount <= requestsBeforeCancel; i++) await new Promise(resolve => setTimeout(resolve, 20));
  assert.ok(requestCount > requestsBeforeCancel, 'Cancellation must occur after the review reaches the gateway');
  await page.getByRole('button', { name: '取消评审', exact: true }).click();
  await page.getByText('已取消', { exact: true }).waitFor();
  await page.getByRole('button', { name: /重试失败项/ }).click();
  await page.getByText('评审完成', { exact: true }).waitFor();
  assert.equal(gatewayErrors.length, 0, gatewayErrors.join("\n"));
  assert.equal(errors.length, 0, errors.join('\n')); assert.ok(requestCount >= 5);
  await writeFile(path.join(artifacts, 'result.json'), JSON.stringify({ passed: true, requestCount, executable,
    sourceEditExpected: process.env.REVIEWER_EXPECT_SOURCE_EDIT || null, runtimePathIsolated: Boolean(process.env.REVIEWER_RUNTIME_PATH),
    checked: ['real Windows EXE', 'WebView2', 'settings and key persistence', 'module creation and save', 'module fields survive restart', 'module edit invalidates scan', 'filename keyword maps DOC and TXT to one module', 'current input/review/output instructions reach gateway', 'local scan sends no model requests', 'stream connection', 'Chinese directory', 'legacy DOC Chinese body and table preview', 'DOC and TXT content sent to gateway', 'review', 'overlapping review/scan rejection', 'details', 'four report exports', 'restart history', 'GUI cancellation and retry'],
    limitation: 'Save dialog response was stubbed; other Tauri IPC and file writes used the real Windows application.' }, null, 2));
  console.log('Windows application UI smoke passed');
} catch (error) {
  if (page) await page.screenshot({ path: path.join(artifacts, 'failure.png') }).catch(() => {});
  await writeFile(path.join(artifacts, 'failure.txt'), [String(error), ...gatewayErrors, ...errors].join('\n'));
  throw error;
} finally { await stop(); server.close(); }
