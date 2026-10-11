#!/usr/bin/env node
// Generate synthetic release-review inputs, never connect to XXL-JOB or an LLM.
// Run using the bundled Node/Python runtime. XLS conversion needs xlwt + xlrd.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import assert from 'node:assert/strict';

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const outputDir = path.resolve(process.argv[2] || path.join(projectRoot, 'artifacts/acceptance-0.1.4'));
const runtime = path.join(os.homedir(), '.cache/codex-runtimes/codex-primary-runtime/dependencies');
const modules = process.env.REVIEWER_ARTIFACT_MODULES || path.join(runtime, 'node/node_modules');
const python = process.env.REVIEWER_ARTIFACT_PYTHON || path.join(runtime, 'python/bin/python3');
const scratch = await fs.mkdtemp(path.join(os.tmpdir(), 'reviewer-xxl-author-'));
await fs.symlink(modules, path.join(scratch, 'node_modules'), 'dir');
const requireRuntime = createRequire(path.join(scratch, 'runtime.js'));
const { Workbook, SpreadsheetFile } = await import(pathToFileURL(requireRuntime.resolve('@oai/artifact-tool')).href);
await fs.mkdir(path.join(outputDir, '模拟发布包'), { recursive: true });
await fs.mkdir(path.join(outputDir, 'fixture-previews'), { recursive: true });

const sourceCommit = '555b48f2edf0e51cd77971565342d80c15c9f18d';
const sourceBase = `https://github.com/xuxueli/xxl-job/blob/${sourceCommit}/`;
const sourceFiles = [
  ['官方文档：任务配置、执行器、超时中断、重复执行', 'doc/XXL-JOB官方文档.md'],
  ['执行器管理表单', 'xxl-job-admin/src/main/resources/templates/biz/group.list.ftl'],
  ['任务管理表单', 'xxl-job-admin/src/main/resources/templates/biz/job.list.ftl'],
  ['中文字段与策略名称', 'xxl-job-admin/src/main/resources/i18n/message_zh_CN.properties'],
  ['调度类型枚举', 'xxl-job-admin/src/main/java/com/xxl/job/admin/scheduler/type/ScheduleTypeEnum.java'],
  ['路由策略枚举', 'xxl-job-admin/src/main/java/com/xxl/job/admin/scheduler/route/ExecutorRouteStrategyEnum.java'],
  ['过期策略枚举', 'xxl-job-admin/src/main/java/com/xxl/job/admin/scheduler/misfire/MisfireStrategyEnum.java'],
  ['阻塞策略枚举', 'xxl-job-core/src/main/java/com/xxl/job/core/constant/ExecutorBlockStrategyEnum.java'],
];
const contracts = [
  ['发布范围', '模拟商城订单批处理', '变更单 DEMO-20261012；拟于 2026-10-12 22:00 在 prod 启用；调度中心时区 Asia/Shanghai。', '所有名称、地址、邮箱和业务数值均为虚构样例。'],
  ['主机清单', '资产归属', 'order-prod-01/02.example.invalid、stock-prod-01.example.invalid 属 prod；ledger-uat.example.invalid 属 uat。', '域名均使用 .invalid，不可用于真实网络连接。'],
  ['发布步骤', '启停与回退', '当前六个任务均为停止；保存配置后核对节点、执行一次并检查业务结果，再启用调度。回退时先停任务，恢复旧配置和版本。', '已产生的业务记录按 bizDate + batchNo 查询；撤销类补偿经业务负责人确认后执行。'],
  ['orderReconcileJob', '参数与处理约定', '必填 bizDate、batchNo、dryRun。bizDate=T-1 时，Handler 每次启动按 Asia/Shanghai 解析前一业务日；batchNo=AUTO_DAILY 时，根据解析后的 bizDate 构造实际唯一批号 DEMO-RyyyyMMdd。', '以实际批号 + orderId 执行 UPSERT，数据库唯一键相同；同一业务日重跑的实际批号一致。典型耗时 180 秒，P95 240 秒；支持线程中断。重复批次演练结果一致。'],
  ['stockSnapshotJob', '参数与处理约定', '必填 warehouse、pageSize；只读库存后覆盖同一快照版本；不要求自动重试，失败由当班处理。', 'P95 45 秒；允许跳过一轮；支持线程中断。'],
  ['cashbackSettleJob', '参数与处理约定', '必填 bizDate、batchNo、dryRun；逐条调用发放接口；当前接口无幂等键，处理中断后不会撤回已发放记录。', 'P95 150 秒；支持线程中断；处理完成后才记录批次完成状态。'],
  ['orderDeltaSyncJob', '参数与处理约定', '必填 batchSize、targetEnv；分页增量读取并按 orderId UPSERT；每五分钟同步一次即可满足时效。', '单次典型耗时 60 秒，P95 90 秒；支持线程中断。'],
  ['ledgerPatchJob', '参数与处理约定', '必填 bizDate、targetEnv；仅允许 prod 发布任务调用 prod 执行器；按 voucherId UPSERT。', 'P95 30 秒；支持线程中断。'],
  ['paymentCompensateJob', '参数与处理约定', '必填 bizDate、dryRun；每次查询当天全部待补偿订单；未读取分片序号；外部接口无幂等键。', 'P95 80 秒；两台实例部署相同 Handler；处理完成后更新订单状态。'],
];
const executors = [
  [101, 'demo-order-prod', '订单生产执行器', '手动录入（1）', 'http://order-prod-01.example.invalid:9999/,http://order-prod-02.example.invalid:9999/', 'prod'],
  [102, 'demo-stock-prod', '库存生产执行器', '自动注册（0）', 'http://stock-prod-01.example.invalid:9999/', 'prod；地址为在线注册快照'],
  [103, 'demo-ledger-prod', '账务生产执行器', '手动录入（1）', 'http://ledger-uat.example.invalid:9999/', 'prod'],
];
const jobs = [
  { id: 9001, group: 101, desc: '订单日终对账', author: '模拟订单组', email: 'orders-oncall@example.invalid', type: 'CRON', schedule: '0 10 2 * * ?', handler: 'orderReconcileJob', param: { bizDate: 'T-1', batchNo: 'AUTO_DAILY', dryRun: false }, route: 'FIRST（第一个）', misfire: 'DO_NOTHING（忽略）', block: 'SERIAL_EXECUTION（单机串行）', timeout: 600, retries: 1 },
  { id: 9002, group: 102, desc: '库存快照刷新', author: '模拟库存组', email: 'stock-oncall@example.invalid', type: 'FIX_RATE（固定速度）', schedule: 300, handler: 'stockSnapshotJob', param: { warehouse: 'DEMO-WH01', pageSize: 500 }, route: 'FIRST（第一个）', misfire: 'DO_NOTHING（忽略）', block: 'DISCARD_LATER（丢弃后续调度）', timeout: 120, retries: 0 },
  { id: 9003, group: 101, desc: '返现结算批次', author: '模拟营销组', email: 'marketing-oncall@example.invalid', type: 'CRON', schedule: '0 0 3 * * ?', handler: 'cashbackSettleJob', param: { bizDate: '2026-10-11', dryRun: false }, route: 'ROUND（轮询）', misfire: 'DO_NOTHING（忽略）', block: 'SERIAL_EXECUTION（单机串行）', timeout: 300, retries: 2 },
  { id: 9004, group: 101, desc: '订单增量同步（每五分钟）', author: '模拟订单组', email: '', type: 'CRON', schedule: '*/10 * * * * ?', handler: 'orderDeltaSyncJob', param: { batchSize: 1000, targetEnv: 'prod' }, route: 'FIRST（第一个）', misfire: 'FIRE_ONCE_NOW（立即执行一次）', block: 'SERIAL_EXECUTION（单机串行）', timeout: 300, retries: 3 },
  { id: 9005, group: 103, desc: '生产账务凭证补录', author: '模拟账务组', email: 'ledger-oncall@example.invalid', type: 'CRON', schedule: '0 30 2 * * ?', handler: 'ledgerPatchJob', param: { bizDate: '2026-10-11', targetEnv: 'prod' }, route: 'FIRST（第一个）', misfire: 'DO_NOTHING（忽略）', block: 'SERIAL_EXECUTION（单机串行）', timeout: 120, retries: 0 },
  { id: 9006, group: 101, desc: '支付异常订单补偿', author: '模拟支付组', email: 'payments-oncall@example.invalid', type: 'CRON', schedule: '0 0/15 * * * ?', handler: 'paymentCompensateJob', param: { bizDate: '2026-10-11', dryRun: false }, route: 'SHARDING_BROADCAST（分片广播）', misfire: 'DO_NOTHING（忽略）', block: 'SERIAL_EXECUTION（单机串行）', timeout: 180, retries: 2 },
];
const sheets = [];
function define(name, title, context, headers, rows, widths, rowHeight) {
  const size = headers.length;
  const pad = (row) => [...row, ...Array(Math.max(0, size - row.length)).fill(null)];
  sheets.push({ name, title, widths, matrix: [pad([]), pad([title]), pad([context]), pad([]), headers, ...rows.map(pad)],
    heights: [12, 34, 44, 12, 46, ...rows.map(() => rowHeight)] });
}
define('发布约定', 'XXL-JOB 模拟上线资料', '仅模拟数据；业务约定是评审依据，非 XXL-JOB 平台表单字段。',
  ['对象', '说明类别', '业务与发布约定', '运行资料'], contracts, [200, 150, 550, 400], 105);
define('执行器', '执行器登记与地址', '字段参照官方 v3.4.0；目标环境是本次发布的附加信息。',
  ['执行器 ID', 'AppName', '名称', '注册方式', '机器地址', '目标环境 / 地址来源'], executors, [95, 180, 145, 130, 510, 230], 96);
define('任务基础', '任务基础配置', '拟发布任务均为停止状态；上线步骤见发布约定。',
  ['任务 ID', '执行器 ID', '任务描述', '负责人', '报警邮件', '当前状态'],
  jobs.map(j => [j.id, j.group, j.desc, j.author, j.email, '停止（0）']), [95, 105, 310, 145, 355, 120], 56);
define('调度运行', '任务调度与运行配置', 'FIX_RATE 单位为秒；任务参数内容由各 Handler 的业务约定定义。',
  ['任务 ID', '调度类型', '调度配置', '运行模式', 'JobHandler', '任务参数'],
  jobs.map(j => [j.id, j.type, j.schedule, 'BEAN', j.handler, JSON.stringify(j.param, null, 2)]), [95, 170, 180, 110, 245, 480], 120);
define('高级配置', '任务高级配置', '任务超时时间单位为秒；子任务 ID 留空表示未配置子任务。',
  ['任务 ID', '路由策略', '调度过期策略', '阻塞处理策略', '超时（秒）', '失败重试次数', '子任务 ID'],
  jobs.map(j => [j.id, j.route, j.misfire, j.block, j.timeout, j.retries, '']), [95, 275, 290, 285, 135, 125, 100], 72);

const workbook = Workbook.create();
for (const spec of sheets) {
  const sheet = workbook.worksheets.add(spec.name);
  sheet.showGridLines = false;
  const rows = spec.matrix.length; const cols = spec.matrix[0].length;
  const all = sheet.getRangeByIndexes(0, 0, rows, cols);
  all.values = spec.matrix;
  all.format.font = { name: 'Arial', size: 11, color: '#223146' };
  all.format.verticalAlignment = 'center';
  all.format.wrapText = true;
  for (let c = 0; c < cols; c++) sheet.getRangeByIndexes(0, c, rows, 1).format.columnWidthPx = spec.widths[c];
  for (let r = 0; r < rows; r++) sheet.getRangeByIndexes(r, 0, 1, cols).format.rowHeightPx = spec.heights[r];
  sheet.getRangeByIndexes(1, 0, 1, cols).format.font = { name: 'Arial', size: 15, bold: true, color: '#243D60' };
  // The title/context remain simple unmerged text cells; overflow into blank cells.
  sheet.getRange('A2').format.wrapText = false;
  sheet.getRange('A3').format.wrapText = false;
  sheet.getRange('A3').format.font = { name: 'Arial', size: 11, color: '#526478' };
  const header = sheet.getRangeByIndexes(4, 0, 1, cols);
  header.format.fill = '#243D60';
  header.format.font = { name: 'Arial', size: 11, bold: true, color: '#FFFFFF' };
  header.format.horizontalAlignment = 'center';
  header.format.borders = { insideVertical: { style: 'thin', color: '#FFFFFF' } };
  for (let r = 5; r < rows; r++) {
    if (r % 2 === 1) sheet.getRangeByIndexes(r, 0, 1, cols).format.fill = '#F0F4F8';
    spec.matrix[r].forEach((value, c) => {
      if (typeof value === 'number') sheet.getCell(r, c).setNumberFormat('0');
    });
  }
  sheet.freezePanes.freezeRows(5);
  sheet.freezePanes.freezeColumns(1);
}
workbook.recalculate();
const verification = await workbook.inspect({ kind: 'table', range: '调度运行!A5:F11', include: 'values,formulas', tableMaxRows: 7, tableMaxCols: 6, maxChars: 5000 });
await fs.writeFile(path.join(outputDir, 'fixture-inspection.ndjson'), verification.ndjson);
const formulaErrors = await workbook.inspect({ kind: 'match', searchTerm: '#REF!|#DIV/0!|#VALUE!|#NAME\\?|#N/A|#NUM!|#NULL!|#SPILL!|#CALC!', options: { useRegex: true, maxResults: 30 }, summary: 'Fixture formula error scan' });
await fs.writeFile(path.join(outputDir, 'fixture-errors.ndjson'), formulaErrors.ndjson);
const previewSheets = process.env.REVIEWER_FIXTURE_PREVIEW_SHEETS?.split(',');
for (const spec of sheets) {
  if (previewSheets && !previewSheets.includes(spec.name)) continue;
  const lastColumn = String.fromCharCode(64 + spec.matrix[0].length);
  const preview = await workbook.render({ sheetName: spec.name, range: `A1:${lastColumn}${spec.matrix.length}`, scale: 1.4, format: 'png' });
  await fs.writeFile(path.join(outputDir, 'fixture-previews', `${spec.name}.png`), new Uint8Array(await preview.arrayBuffer()));
}
await (await SpreadsheetFile.exportXlsx(workbook)).save(path.join(outputDir, '模拟发布包', 'XXL-JOB配置.xlsx'));
// The exporter may emit an inspection sidecar; keep every support file outside
// the directory selected for model input.
const exportInspection = path.join(outputDir, '模拟发布包', 'XXL-JOB配置.xlsx.inspect.ndjson');
try { await fs.rename(exportInspection, path.join(outputDir, 'fixture-export-inspection.ndjson')); }
catch (error) { if (error.code !== 'ENOENT') throw error; }
const manifestFile = path.join(scratch, 'fixture-data.json');
await fs.writeFile(manifestFile, JSON.stringify({ sourceCommit, sheets }));
execFileSync(python, [path.join(projectRoot, 'scripts/generate-xxl-fixtures.py'), manifestFile, outputDir], { stdio: 'inherit', env: process.env });

const guide = `# XXL-JOB 样例验收说明\n\n本目录的两个 Excel 是相同业务数据的格式对照，均为虚构模拟资料，不对应真实调度平台。扫描时只选择“模拟发布包”文件夹。此说明、图片和验证结果位于扫描目录外，不应作为模型输入。\n\n参照 XXL-JOB v3.4.0，固定源码提交 ${sourceCommit}。项目旧原型只有简化字段示例，因此本次以官方表单、枚举和中文资源为字段事实源；这不是用户实际部署版本的确认。\n\n## 正常与风险场景的人工验收预期\n\n- 9001：bizDate=T-1 由 Handler 每次启动按 Asia/Shanghai 解析前一业务日；batchNo=AUTO_DAILY 根据解析后的 bizDate 构造实际唯一批号，无需人工每日修改。以实际批号 + orderId 做 UPSERT 和数据库唯一键，同一业务日重跑保持相同批号。600 秒超时、240 秒 P95、报警和回退资料一致，不应仅因启用 1 次重试而判为必然重复。\n- 9002：300 秒间隔、45 秒 P95、120 秒超时。只读覆盖快照、允许跳过一轮、0 次重试且有报警是已声明业务选择；不应机械地要求所有任务必须重试。\n- 9003：调度运行第 8 行参数缺少发布约定要求的 batchNo；高级配置第 8 行重试 2 次，但发放接口无幂等键且中断不会撤回已发放记录。应指出缺参和重复发放风险，提出参数校验及幂等/补偿措施。\n- 9004：调度运行第 9 行 cron 每 10 秒触发，与任务描述/发布约定的每五分钟不一致；典型耗时 60 秒、P95 90 秒，FIRST + 单机串行可能持续积压；300 秒超时不等于吞吐保障，重试可能放大负载。任务基础第 9 行报警邮件为空，应要求确认替代告警或补齐。不能把单机串行直接表述为同一节点并行执行。\n- 9005：执行器第 8 行声明 prod，但手工地址指向发布约定已标明属 uat 的主机；任务参数 targetEnv=prod。应要求修正环境/地址绑定并验证后再上线，而不是仅凭名称就猜测地址用途。\n- 9006：执行器 101 有两台节点，SHARDING_BROADCAST 会向两台触发；Handler 读取全量未分片、外部接口无幂等键且重试 2 次，应指出重复补偿风险并要求分片或幂等保护。\n\n整份文件包含多项阻断级风险，合理总评应为阻断；允许模型措辞不同，但需要引用任务 ID、字段值及跨表依据。测试只核对识别能力，不要求模型逐字输出本说明。任务参数 schema、耗时与资产环境映射来自虚构业务约定，不是 XXL-JOB 平台自身的校验规则。\n\n## 字段边界\n\n- 执行器原生字段：AppName、名称、注册方式、机器地址。执行器 ID 为关联标识；目标环境和自动注册快照来源是发布资料附加列。\n- 任务原生字段：执行器、任务描述、负责人、报警邮件、调度类型/配置、运行模式、JobHandler、任务参数、路由策略、子任务 ID、调度过期策略、阻塞处理策略、超时、重试。\n- 表单保留 FIX_DELAY 片段且文档描述固定延迟，但该固定提交 ScheduleTypeEnum 注释掉 FIX_DELAY；样例不把它作为可选类型。\n- 超时依赖业务代码配合线程中断，不代表副作用自动回滚。单机串行并非集群全局互斥。\n\n## 来源\n\n${sourceFiles.map(([label, file]) => `- [${label}](${sourceBase}${file})`).join('\n')}\n\n## 文件与复现\n\nXLSX 使用 @oai/artifact-tool 生成并渲染所有工作表；XLS 使用 xlwt 生成 BIFF8/OLE 二进制文件，非改后缀。fixture-validation.json 记录签名、SHA256 和两种格式逐单元格比对。图片用于检查文字、换行和列宽。此处不证明原生 Excel UI 或真实模型评审已通过。\n\n使用 bundled Node 运行 scripts/generate-xxl-fixtures.mjs，可传输出目录；使用 bundled Python，PYTHONPATH 指向临时安装的 xlwt 1.3.0、xlrd 2.0.1。脚本不读取应用设置、API Key 或用户资料，也不会调用任何网络服务。\n`;
await fs.writeFile(path.join(outputDir, 'XXL-JOB样例验收说明.md'), guide);
const checked = JSON.parse(await fs.readFile(path.join(outputDir, 'fixture-validation.json'), 'utf8'));
assert.equal(checked.passed, true);
await fs.rm(scratch, { recursive: true, force: true });
console.log(`XXL-JOB fixtures exported and compared: ${outputDir}`);
