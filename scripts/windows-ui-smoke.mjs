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
const root = await mkdtemp(path.join(process.env.RUNNER_TEMP || tmpdir(), 'reviewer-ui-'));
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
const scheduledGatewayInputs = new Set();
let scheduleEvidence = {};
let largeScanEvidence = {};
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
      for (const marker of ['QA_SCHEDULE_GIT_V2', 'QA_SCHEDULE_GIT_V3']) if (prompt.includes(marker)) scheduledGatewayInputs.add(marker);
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
const errors = [];
async function launch() {
  app = spawn(executable, [], { env: { ...process.env, PATH: runtimePath, APPDATA: path.join(root, 'appdata'),
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
// Git is a documented optional prerequisite for scheduled pull. These fixtures
// touch only this run's temp directory and never contact an external remote.
const gitExecutable = execFileSync(process.platform === 'win32' ? 'where.exe' : 'which', ['git'], { encoding: 'utf8', windowsHide: true }).trim().split(/\r?\n/)[0];
const runtimePath = [process.env.REVIEWER_RUNTIME_PATH || process.env.PATH, path.dirname(gitExecutable)].join(path.delimiter);
const gitEnv = { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: path.join(root, 'empty-git-config'), GIT_TERMINAL_PROMPT: '0' };
await writeFile(gitEnv.GIT_CONFIG_GLOBAL, '');
function git(cwd, args) {
  return execFileSync(gitExecutable, args, { cwd, env: gitEnv, encoding: 'utf8', windowsHide: true, timeout: 30000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
const scheduledName = `${moduleName}定时拉取`;
const scheduledFilename = `${moduleName}定时.txt`;
const remote = path.join(root, 'schedule-remote.git');
const seed = path.join(root, 'schedule-seed');
const checkout = path.join(root, '定时任务仓库');
git(root, ['init', '--bare', '--initial-branch=main', remote]);
git(root, ['clone', remote, seed]);
async function publishFixture(version) {
  await writeFile(path.join(seed, scheduledFilename), `QA_SCHEDULE_GIT_V${version}：订单同步 orderSyncJob 配置版本 ${version}，3 次重试。`);
  git(seed, ['add', scheduledFilename]);
  git(seed, ['-c', 'user.name=Reviewer UI Test', '-c', 'user.email=qa@example.invalid', 'commit', '-m', `fixture ${version}`]);
  git(seed, ['push', 'origin', 'HEAD']);
}
await publishFixture(1);
git(root, ['clone', remote, checkout]);
const initialScheduledCommit = git(checkout, ['rev-parse', 'HEAD']);
await publishFixture(2);
const updatedScheduledCommit = git(seed, ['rev-parse', 'HEAD']);
const largeInputs = path.join(root, '超过800个文件');
await mkdir(largeInputs);
await Promise.all(Array.from({ length: 805 }, (_, i) => writeFile(path.join(largeInputs, `${moduleName}-${String(i + 1).padStart(4, '0')}.txt`), `订单同步 orderSyncJob 第 ${i + 1} 个配置文件。`)));
function scheduleCard(name = scheduledName) {
  return page.getByTestId('schedule-card').filter({ has: page.getByRole('heading', { name, exact: true }) });
}
async function schedules() { return page.evaluate(() => window.__TAURI_INTERNALS__.invoke('list_schedules')); }
async function waitForSchedule(id, previousTaskId, state, timeout = 45000) {
  const deadline = Date.now() + timeout;
  let job;
  do {
    // Await each IPC response in Node; do not serialize a browser-side async
    // polling handle through WebView2's CDP bridge.
    job = (await schedules()).find(item => item.config.id === id);
    const run = job?.last_run;
    if (run && run.task_id !== previousTaskId && run.state !== 'running') {
      assert.equal(run.state, state, run.message);
      return job;
    }
    await new Promise(resolve => setTimeout(resolve, 250));
  } while (Date.now() < deadline);
  throw new Error(`Scheduled run did not finish: ${JSON.stringify(job?.last_run ?? null)}`);
}
function nativeWindow(action, hwnd) {
  const command = `Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class ReviewerWindow { [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l); [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h); [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n); }'; `;
  if (action === 'close') {
    const result = execFileSync('powershell.exe', ['-NoProfile', '-Command', command + `$reviewWindow=(Get-Process -Id ${app.pid}).MainWindowHandle; if ($reviewWindow -eq 0) { throw 'No visible app window'; }; [ReviewerWindow]::PostMessage($reviewWindow,16,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null; Start-Sleep -Milliseconds 400; @{ handle=$reviewWindow.ToInt64(); visible=[ReviewerWindow]::IsWindowVisible($reviewWindow); running=[bool](Get-Process -Id ${app.pid} -ErrorAction SilentlyContinue) } | ConvertTo-Json -Compress`], { encoding: 'utf8', windowsHide: true });
    return JSON.parse(result.trim());
  }
  execFileSync('powershell.exe', ['-NoProfile', '-Command', command + `[ReviewerWindow]::ShowWindow([IntPtr]${hwnd},5) | Out-Null`], { windowsHide: true });
}
async function verifyLargeScan() {
  const baseline = requestCount;
  await page.getByRole('button', { name: /新建评审/ }).click();
  await page.getByRole('button', { name: '扫描本地目录', exact: true }).click();
  await page.locator('input[placeholder*="2026Q3-mixed"]').fill(largeInputs);
  await page.getByRole('button', { name: '下一步：扫描解析 →', exact: true }).click();
  await page.getByText('命中 805 / 805', { exact: true }).waitFor({ timeout: 60000 });
  await page.getByText('第 1 / 9 页 · 共 805 个文件', { exact: true }).waitFor();
  for (let i = 0; i < 8; i++) await page.getByRole('button', { name: '下一页', exact: true }).click();
  await page.getByText('第 9 / 9 页 · 共 805 个文件', { exact: true }).waitFor();
  await page.getByText(`${moduleName}-0805.txt`, { exact: true }).first().waitFor();
  await page.getByText(`${moduleName}-0805.txt`, { exact: true }).first().scrollIntoViewIfNeeded();
  assert.equal(await page.getByRole('row').count(), 6, 'Last page must show five files plus header');
  assert.equal(requestCount, baseline, '805-file local scan must not send model requests');
  await page.screenshot({ path: path.join(artifacts, 'scan-over-800.png'), fullPage: true });
  largeScanEvidence = { discovered: 805, matched: 805, pages: 9, finalPageFiles: 5, gatewayRequests: requestCount - baseline };
}
async function verifySchedules() {
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  await page.getByRole('button', { name: '新建定时任务', exact: true }).click();
  await page.getByLabel('任务名称', { exact: true }).fill(scheduledName);
  await page.getByLabel('资料目录', { exact: true }).fill(checkout);
  await page.getByLabel('执行间隔（分钟）', { exact: true }).fill('60');
  for (const label of ['执行前 Git 拉取', '仅内容或评审配置变化时评审', '启用定时执行']) await page.getByLabel(label, { exact: true }).check();
  await page.getByRole('button', { name: '保存任务', exact: true }).click();
  await scheduleCard().getByText('定时已启用', { exact: true }).waitFor();
  let job = (await schedules()).find(item => item.config.name === scheduledName);
  assert.ok(job && job.config.git_pull && job.config.only_changed && job.config.enabled);
  const id = job.config.id;
  const beforeReview = requestCount;
  await scheduleCard().getByRole('button', { name: '立即执行', exact: true }).click();
  job = await waitForSchedule(id, undefined, 'succeeded');
  assert.equal(job.last_run.before_commit, initialScheduledCommit);
  assert.equal(job.last_run.after_commit, updatedScheduledCommit);
  assert.equal(job.last_run.total, 1); assert.equal(job.last_run.processed, 1);
  assert.ok(job.last_run.history_id, 'Scheduled review must save a real history report');
  assert.ok(scheduledGatewayInputs.has('QA_SCHEDULE_GIT_V2'), 'Pulled file contents must reach the real gateway request');
  assert.equal(requestCount, beforeReview + 1);
  await scheduleCard().getByText('评审完成', { exact: true }).waitFor();
  await page.screenshot({ path: path.join(artifacts, 'scheduled-git-review.png'), fullPage: true });
  const firstRun = job.last_run;
  await scheduleCard().getByRole('button', { name: '查看本次报告', exact: true }).click();
  await page.getByText(scheduledFilename, { exact: true }).first().waitFor();
  await page.getByRole('button', { name: /评审历史/ }).click();
  await page.getByRole('row').filter({ hasText: checkout }).first().waitFor();
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  const beforeSkip = requestCount;
  await scheduleCard().getByRole('button', { name: '立即执行', exact: true }).click();
  job = await waitForSchedule(id, firstRun.task_id, 'unchanged');
  assert.equal(requestCount, beforeSkip, 'Unchanged schedule must skip model requests');
  await scheduleCard().getByText('无变化，已跳过', { exact: true }).waitFor();
  await scheduleCard().getByRole('button', { name: '暂停', exact: true }).click();
  await scheduleCard().getByText('定时已暂停', { exact: true }).waitFor();
  job = (await schedules()).find(item => item.config.id === id);
  assert.equal(job.config.enabled, false); assert.equal(job.next_run_at, null);
  await stop(); await launch();
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  await scheduleCard().getByText('定时已暂停', { exact: true }).waitFor();
  const persisted = (await schedules()).find(item => item.config.id === id);
  assert.equal(persisted.last_run.task_id, job.last_run.task_id);
  assert.equal(persisted.config.directory, checkout);
  await scheduleCard().getByRole('button', { name: '启用', exact: true }).click();
  await scheduleCard().getByText('定时已启用', { exact: true }).waitFor();
  await verifyLargeScan();
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  await scheduleCard().getByRole('button', { name: '编辑', exact: true }).click();
  await page.getByLabel('执行间隔（分钟）', { exact: true }).fill('1');
  await page.getByRole('button', { name: '保存任务', exact: true }).click();
  await scheduleCard().getByText('每 1 分钟', { exact: true }).waitFor();
  // Close the native window and let a real timer tick trigger Git + review
  // in the tray. No run_schedule_now call is made for this last run.
  await publishFixture(3);
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  const windowState = nativeWindow('close');
  assert.equal(windowState.visible, false, 'Native close must hide the main window');
  assert.equal(windowState.running, true, 'Closing the window must keep the scheduler process running');
  const beforeTimer = requestCount;
  const timed = await waitForSchedule(id, persisted.last_run.task_id, 'succeeded', 85000);
  assert.ok(timed.last_run.started_at >= persisted.last_run.finished_at);
  assert.ok(scheduledGatewayInputs.has('QA_SCHEDULE_GIT_V3'), 'Hidden-window timer must pull and review newly committed content');
  assert.equal(requestCount, beforeTimer + 1);
  nativeWindow('show', windowState.handle);
  await scheduleCard().getByText('评审完成', { exact: true }).waitFor();
  await page.screenshot({ path: path.join(artifacts, 'scheduled-background-timer.png'), fullPage: true });
  scheduleEvidence = { firstRun, unchangedRun: persisted.last_run, persistedAfterRestart: true, pausedAndEnabledInUi: true, editedIntervalMinutes: 1, hiddenWindow: windowState, timedRun: timed.last_run, gatewayMarkers: [...scheduledGatewayInputs], gitVersion: git(root, ['--version']) };
  const exitingPid = app.pid;
  await stop();
  let stillRunning = true; try { process.kill(exitingPid, 0); } catch { stillRunning = false; }
  assert.equal(stillRunning, false, 'Fully terminated application must have no scheduler process');
  scheduleEvidence.fullyExitedProcessAbsent = true;
  await launch();
  await page.locator('aside.sidebar').getByRole('button', { name: /定时任务/ }).click();
  await scheduleCard().getByRole('button', { name: '删除', exact: true }).click();
  await scheduleCard().getByRole('button', { name: '确认删除', exact: true }).click();
  await scheduleCard().waitFor({ state: 'detached' });
  assert.equal((await schedules()).some(item => item.config.id === id), false);
  const history = await page.evaluate(id => window.__TAURI_INTERNALS__.invoke('get_history', { id }), firstRun.history_id);
  assert.ok(history, 'Deleting schedule must preserve existing history');
  scheduleEvidence.deletedInUiAndHistoryPreserved = true;
}
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
  await verifySchedules();
  assert.equal(gatewayErrors.length, 0, gatewayErrors.join("\n"));
  assert.equal(errors.length, 0, errors.join('\n')); assert.ok(requestCount >= 5);
  await writeFile(path.join(artifacts, 'result.json'), JSON.stringify({ passed: true, requestCount, scheduleEvidence, largeScanEvidence,
    checked: ['installed EXE', 'WebView2', 'settings and key persistence', 'module creation and save', 'module fields survive restart', 'module edit invalidates scan', 'filename keyword maps DOC and TXT to one module', 'current input/review/output instructions reach gateway', 'local scan sends no model requests', 'stream connection', 'Chinese directory', 'legacy DOC Chinese body and table preview', 'DOC and TXT content sent to gateway', 'review', 'overlapping review/scan rejection', 'details', 'four report exports', 'restart history', 'GUI cancellation and retry', '805-file UI scan without truncation', 'scheduled job GUI create/edit/pause/enable/delete', 'real local Git pull reaches gateway', 'scheduled report and history', 'unchanged schedule skips gateway', 'schedule persistence across restart', 'one-minute automatic trigger while native window is hidden', 'fully exited scheduler process absent'],
    limitation: 'Save dialog response was stubbed; model responses came from a local mock HTTP server, while all Tauri IPC, Git, timer, history and file writes used the real installed application. Native WM_CLOSE was tested; full termination used taskkill, not the native tray Exit menu.' }, null, 2));
  console.log('Windows installed application UI smoke passed');
} catch (error) {
  if (page) await page.screenshot({ path: path.join(artifacts, 'failure.png') }).catch(() => {});
  await writeFile(path.join(artifacts, 'failure.txt'), [String(error), ...gatewayErrors, ...errors].join('\n'));
  throw error;
} finally { await stop(); server.close(); }
