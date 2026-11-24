'use strict';
// Launched by workbench_e2e.py with a fresh Harness instance and a local provider.
const assert = require('node:assert/strict');
const { chromium } = require(process.env.LOOM_PLAYWRIGHT_MODULE || 'playwright');
const base = process.env.LOOM_BROWSER_BASE;
const image = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jV90AAAAASUVORK5CYII=';

const button = (p, name) => p.getByRole('button', { name, exact: true });
const nav = (p, id) => p.locator('#desk-nav [data-section="' + id + '"]').click();
const row = (p, text) => p.locator('#desk-content button.desk-row').filter({ hasText: text });
const field = (p, name) => p.locator('#desk-form [name="' + name + '"]');

async function read(path, token = 'admin-token') {
  const res = await fetch(base + '/api/' + path, { headers: { Authorization: 'Bearer ' + token } });
  assert.equal(res.status, 200, path);
  return res.json();
}

async function login(p, token) {
  await p.goto(base + '/webui/');
  await p.locator('#connection-button').click();
  await p.locator('#token-input').fill(token);
  await p.locator('#credential-save').click();
  await p.locator('#credential-dialog').waitFor({ state: 'hidden' });
  await p.locator('#desk-nav [data-section=runs]').waitFor();
}

async function submit(p, name = '保存') {
  const dialog = p.locator('#desk-dialog');
  await button(dialog, name).click();
  await dialog.waitFor({ state: 'hidden' });
}

async function send(p, message) {
  await p.locator('#prompt').fill(message);
  const receipt = p.waitForResponse(
    (r) => r.request().method() === 'POST' && r.url().endsWith('/deliveries'),
  );
  await p.locator('#send').click();
  const result = await receipt;
  assert.ok(result.ok());
  return result.json();
}

async function waitRun(id, status) {
  for (let i = 0; i < 100; i++) {
    const r = await read('runs/' + id);
    if (r.status === status) return r;
    assert.ok(!['failed', 'cancelled'].includes(r.status), JSON.stringify(r));
    await new Promise((resolve) => setTimeout(resolve, 120));
  }
  assert.fail('Run ' + id + ' did not reach ' + status);
}

async function catalog(p) {
  await nav(p, 'tools');
  await button(p, '创建独立工具').click();
  await field(p, 'id').fill('browser-check');
  await field(p, 'description').fill('浏览器测试');
  await field(p, 'code').fill('return function(ctx,args) return true end');
  await submit(p);
  await button(p, '工具集').click();
  await button(p, '创建工具集').click();
  await field(p, 'id').fill('browser-set');
  await field(p, 'members').selectOption('home.browser-check');
  await submit(p);
  const sets = await read('catalog/toolsets');
  assert.deepEqual(sets.find((s) => s.id === 'home.browser-set').members, ['home.browser-check']);

  await nav(p, 'plugins');
  await row(p, 'internal-tools').click();
  assert.equal(await field(p, 'package').inputValue(), 'internal-tools');
  assert.ok(await field(p, 'version').inputValue());
  await field(p, 'id').fill('test-office');
  await submit(p);
  await button(p, '编辑设置').click();
  await field(p, 'base_url').fill('http://127.0.0.1:9');
  await field(p, 'catalog_service').fill('catalog');
  await field(p, 'catalog_credential_ref').fill('test-catalog');
  await field(p, 'consumer_credential_ref').fill('test-consumer');
  await submit(p);
  assert.equal((await read('settings/test-office')).catalog_service, 'catalog');

  await nav(p, 'dispatches');
  await button(p, '创建调度').click();
  await field(p, 'id').fill('preview-check');
  await field(p, 'trigger').fill(JSON.stringify({ type: 'schedule', interval_seconds: 120 }));
  await submit(p);
  await button(p, '预览匹配').click();
  await submit(p, '预览');
  await p.locator('summary').filter({ hasText: '预览结果' }).waitFor();
  assert.equal((await read('dispatches/preview-check')).trigger.interval_seconds, 120);

  await nav(p, 'host');
  await button(p, '应用 HOME 配置').click();
  await p.locator('summary').filter({ hasText: '应用 HOME 配置' }).waitFor();
}

async function creator(p) {
  await nav(p, 'agents');
  await row(p, '创造者').click();
  await button(p, '编辑 Agent').click();
  await field(p, 'description').fill('从工作台修改描述');
  await submit(p);
  assert.equal((await read('agents/author')).description, '从工作台修改描述');

  await button(p, '与 Agent 对话').click();
  const r = await send(p, '修改自己的描述为「聊天已更新描述」');
  await waitRun(r.run_id, 'completed');
  const calls = await read('runs/' + r.run_id + '/tool-calls');
  const update = calls.find((call) => call.tool_id === 'agents.update');
  assert.equal(update?.status, 'completed');
  await p.locator('#messages').getByText('完成', { exact: true }).waitFor();
  // A code-mode operation carries its own related ToolCall. Reach it via the Run.
  await p.locator('#session-details').click();
  await row(p, r.run_id).click();
  await row(p, update.id).click();
  await p.locator('#desk-content').getByText('agents.update', { exact: true }).waitFor();
  await nav(p, 'agents');
  await row(p, '创造者').click();
  await p.locator('#desk-content').getByText('聊天已更新描述', { exact: true }).waitFor();
}

async function conversation(p) {
  await nav(p, 'agents');
  await row(p, '工作助手').click();
  await button(p, '与 Agent 对话').click();
  await p.locator('#new-session').click();
  await p.locator('#attach-image').click({ trial: true });
  await p
    .locator('#image-picker')
    .setInputFiles({ name: 'fixture.png', mimeType: 'image/png', buffer: Buffer.from(image, 'base64') });
  await p.locator('#compose-chips button').waitFor();
  const r = await send(p, '检查图片');
  await waitRun(r.run_id, 'completed');
  await p.locator('#messages').getByText('收到图片', { exact: true }).waitFor();
  await button(p, '查看图片').click();
  await p.locator('#messages img').waitFor();
  await p.locator('#suggest-next').click();
  await p.locator('#suggested-draft').click();
  await p.waitForFunction(() => document.getElementById('prompt').value === '再检查一次记录');

  await p.locator('#session-details').click();
  await button(p, '预览模型上下文').click();
  await p.locator('summary').filter({ hasText: '预览模型上下文' }).waitFor();
  await button(p, '创建分支').click();
  await submit(p, '创建分支');
  await p.locator('#desk-panel').waitFor({ state: 'hidden' });
  const branch = new URL(p.url()).hash;
  assert.ok(!branch.includes(r.session_id));

  await nav(p, 'runs');
  await row(p, r.run_id).click();
  await row(p, 'fixture').click();
  await p.locator('summary').filter({ hasText: '发送的消息' }).waitFor();
  await nav(p, 'events');
  await p.locator('#desk-content button.desk-row').first().click();
  await p.locator('summary').filter({ hasText: '事件正文' }).waitFor();
}

async function approvalAndGoal(p) {
  await login(p, 'approve-token');
  assert.equal(await p.locator('#desk-nav [data-section=plugins]').count(), 0);
  await p.locator('#agent-select').selectOption('desktop');
  await p.locator('#new-session').click();
  const r = await send(p, '审批笔记测试');
  await waitRun(r.run_id, 'waiting');
  await nav(p, 'runs');
  await row(p, r.run_id).click();
  await row(p, 'home.audit').click();
  await button(p, '批准').click();
  await submit(p, '确认批准');
  await waitRun(r.run_id, 'completed');
  await p.locator('#desk-refresh').click();
  await button(p, '读取结果 1 原文').click();
  const result = p
    .locator('#desk-content details')
    .filter({ has: p.locator('summary', { hasText: '读取结果 1 原文' }) });
  assert.ok((await result.locator('pre').textContent()).includes('note audit row'));
  await button(p, '会话详情').click();
  const notes = await read('sessions/' + r.session_id + '/notes', 'approve-token');
  const note = notes.items[0];
  await p.getByLabel('按笔记描述检索').fill(note.description.slice(0, 5));
  await button(p, '检索').click();
  await row(p, note.note_ref).click();
  await field(p, 'start').fill('2');
  await field(p, 'end').fill('3');
  await submit(p, '读取');
  assert.equal((await p.locator('#note-readout pre').textContent()).trim(), 'note audit row\nnote audit row');

  await button(p, '返回对话').click();
  await p.locator('#new-session').click();
  await p.locator('#goal-toggle').click();
  const g = await send(p, '目标验收');
  const run = await waitRun(g.run_id, 'completed');
  assert.equal(run.goal.objective, '目标验收');

  await p.setViewportSize({ width: 390, height: 844 });
  await p.emulateMedia({ reducedMotion: 'reduce' });
  await p.locator('#sidebar-toggle').click();
  await nav(p, 'runs');
  await row(p, g.run_id).click();
  await p.locator('#desk-content').getByText('目标验收', { exact: true }).waitFor();
  assert.equal(await p.locator('#sidebar-toggle').getAttribute('aria-expanded'), 'false');
  assert.ok(await p.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
}

(async () => {
  assert.ok(base, 'Launch with workbench_e2e.py');
  const browser = await chromium.launch({ executablePath: process.env.LOOM_BROWSER_EXECUTABLE || undefined });
  try {
    const p = await browser.newPage({ viewport: { width: 1280, height: 840 } });
    p.setDefaultTimeout(12000);
    const errors = [];
    p.on('pageerror', (err) => errors.push(err.message));
    await login(p, 'admin-token');
    await catalog(p);
    console.log('PASS catalog, installed packages, scheduling, HOME');
    await creator(p);
    console.log('PASS creator edits its own Agent spec via a traced chat ToolCall');
    await conversation(p);
    console.log('PASS attachment, suggestion, branch, context, ModelCall and Event');
    await approvalAndGoal(p);
    console.log('PASS approval, result original, Note line range, Goal and mobile navigation');
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
