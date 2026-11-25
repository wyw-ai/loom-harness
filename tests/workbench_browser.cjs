'use strict';
// Launched by workbench_e2e.py with a fresh Harness instance and a local provider.
const assert = require('node:assert/strict');
const { mkdir } = require('node:fs/promises');
const path = require('node:path');
const { chromium } = require(process.env.LOOM_PLAYWRIGHT_MODULE || 'playwright');
const base = process.env.LOOM_BROWSER_BASE;
const image = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jV90AAAAASUVORK5CYII=';

const button = (p, name) => p.getByRole('button', { name, exact: true });
const nav = async (p, id) => {
  const drawer = p.locator('#sidebar-toggle');
  if (await drawer.isVisible() && await drawer.getAttribute('aria-expanded') !== 'true') await drawer.click();
  await p.locator('#desk-nav [data-section="' + id + '"]').click();
};
const row = (p, text) => p.locator('#desk-content button.desk-row').filter({ hasText: text });
const field = (p, name) => p.locator('#desk-form [name="' + name + '"]');

async function read(path, token = 'admin-token') {
  const res = await fetch(base + '/api/' + path, { headers: { Authorization: 'Bearer ' + token } });
  assert.equal(res.status, 200, path);
  return res.json();
}

async function login(p, token, location = '') {
  await p.goto(base + '/webui/' + location);
  await p.locator('#connection-button').click();
  await p.locator('#token-input').fill(token);
  await p.locator('#credential-save').click();
  await p.locator('#credential-dialog').waitFor({ state: 'hidden' });
  await p.locator('#desk-nav [data-section=runs]').waitFor({state:'attached'});
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

async function capture(page, name) {
  const dir = process.env.LOOM_WORKBENCH_SCREENSHOTS;
  if (!dir) return;
  await mkdir(dir, {recursive:true});
  await page.screenshot({path:path.join(dir, name + '.png'), animations:'disabled'});
}

async function consoleWorkflow(p) {
  const recentRuns = p.locator('#overview-runs');
  const recentSessions = p.locator('#overview-sessions');
  await p.getByRole('heading', {name:'总览', exact:true}).waitFor();
  await recentRuns.getByText('暂无运行记录', {exact:true}).waitFor();
  assert.equal(await p.locator('#desk-nav [data-section=overview]').getAttribute('aria-current'), 'page');
  const overviewApi = '**/api/console/overview';
  await p.route(overviewApi, route => route.fulfill({status:503, contentType:'application/json', body:'{}'}));
  try {
    await p.locator('#desk-refresh').click();
    await p.locator('#desk-content').getByText(/读取失败/).first().waitFor();
    await button(p.locator('#overview-agents'), '查看 Agent 配置 · 工作助手').waitFor();
  } finally {
    await p.unroute(overviewApi);
  }
  await button(p.locator('#desk-content'), '重试').click();
  await recentRuns.getByText('暂无运行记录', {exact:true}).waitFor();

  await button(p.locator('#overview-agents'), '与 工作助手 对话').click();
  await p.locator('#new-session').click();
  const handoff = await send(p, '整理本次发布的交接事项');
  await waitRun(handoff.run_id, 'completed');
  await p.locator('#messages').getByRole('cell', {name:'值班', exact:true}).waitFor();
  await p.locator('#session-usage [data-metric=tokens]').filter({hasText:/^1K$/}).waitFor();
  await capture(p, 'conversation-desktop');
  await p.locator('#new-session').click();
  const pending = await send(p, '核对生产发布的审计记录');
  await waitRun(pending.run_id, 'waiting');

  await p.locator('#agent-select').selectOption('monitor');
  await p.locator('#new-session').click();
  const live = await send(p, '检查当前发布队列的执行进度');
  await p.locator('#messages').getByText('已读取发布队列。', {exact:true}).waitFor();
  const draft = '发布前再确认一次值班安排。';
  await p.locator('#prompt').fill(draft);
  await nav(p, 'overview');
  const active = recentRuns.locator('[data-run-id="' + live.run_id + '"]');
  await active.waitFor();
  assert.match(await active.innerText(), /运行中/);
  assert.match(await active.innerText(), /检查当前发布队列的执行进度/);
  assert.match(await recentRuns.locator('[data-run-id="' + pending.run_id + '"]').innerText(), /等待/);
  await recentSessions.locator('[data-session-id="' + handoff.session_id + '"]').waitFor();
  await capture(p, 'overview-desktop');
  await active.click();
  await p.locator('#desk-content .run-usage-details[data-run-id="' + live.run_id + '"]').waitFor();
  await p.goBack();
  await recentRuns.locator('[data-run-id="' + live.run_id + '"]').waitFor();
  await p.goForward();
  await p.locator('#desk-content .run-usage-details[data-run-id="' + live.run_id + '"]').waitFor();

  await p.setViewportSize({width:390, height:844});
  await p.locator('#sidebar-toggle').click();
  await p.locator('#desk-nav [data-section=agents]').waitFor();
  await capture(p, 'navigation-mobile');
  await nav(p, 'agents');
  assert.equal(await p.locator('#sidebar-toggle').getAttribute('aria-expanded'), 'false');
  assert.ok(await p.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  await row(p, '巡检助手').click();
  await button(p, '编辑 Agent').click();
  await field(p, 'description').fill('用于核对云端发布与巡检记录。');
  await submit(p);
  assert.equal((await read('agents/monitor')).description, '用于核对云端发布与巡检记录。');

  // The model emits its final chunk only after the real UI has switched to management.
  assert.equal((await read('runs/' + live.run_id)).status, 'running');
  assert.equal((await fetch(process.env.LOOM_BROWSER_RELEASE, {method:'POST'})).status, 204);
  await waitRun(live.run_id, 'completed');
  const finalText = p.locator('#messages').getByText('当前批次的检查均已结束，可以开始发布。', {exact:true});
  await finalText.waitFor({state:'attached'});
  await p.setViewportSize({width:1440, height:900});
  await p.locator('#nav-chats').click();
  await finalText.waitFor();
  assert.equal(await p.locator('#prompt').inputValue(), draft);
  assert.equal(await p.locator('#messages .message-assistant').count(), 1);
  assert.equal(new URLSearchParams(new URL(p.url()).hash.slice(1)).get('session'), live.session_id);

  await nav(p, 'overview');
  await recentSessions.locator('[data-session-id="' + handoff.session_id + '"]').click();
  await p.locator('#messages').getByRole('cell', {name:'值班', exact:true}).waitFor();
  assert.equal(new URLSearchParams(new URL(p.url()).hash.slice(1)).get('session'), handoff.session_id);
}

async function usageFigures(page) {
  const created = await page.request.post(base + '/api/sessions', {
    headers: {Authorization:'Bearer admin-token'}, data:{agent_id:'desktop'},
  });
  assert.ok(created.ok());
  const session = await created.json();
  const link = '#agent=desktop&session=' + session.id;
  await login(page, 'admin-token', link);
  const meter = page.locator('#session-usage');
  const metric = name => meter.locator('[data-metric="' + name + '"]');
  for (const [prompt, runTokens, sessionTokens] of [['统计第一轮',100,100], ['统计第二轮',150,250]]) {
    const job = await send(page,prompt);
    assert.equal(job.session_id, session.id, 'The requested session deep link must own the new Run');
    await waitRun(job.run_id,'completed');
    const receipt = page.locator('#messages .run-usage[data-run-id="' + job.run_id + '"]');
    await receipt.filter({hasText:runTokens + ' token'}).first().waitFor();
    await metric('tokens').filter({hasText:new RegExp('^'+sessionTokens+'$')}).waitFor();
    assert.match(await receipt.first().textContent(), /秒/);
  }
  await meter.locator('summary').click();
  assert.match(await meter.innerText(), /50%/);
  assert.match(await metric('speed').textContent(), /tok\/s/);
  const snapshot = await read('sessions/' + session.id + '/usage');
  assert.equal(snapshot.runs.length,2);
  assert.deepEqual([snapshot.tokens.input,snapshot.tokens.output,snapshot.tokens.total],[200,50,250]);
  assert.equal(snapshot.cache.hit_rate,0.5);
  assert.ok(snapshot.speed.output_tokens_per_second > 0);
  const last = snapshot.runs.find(run => run.tokens.total===150);
  assert.ok(last.elapsed_ms > 0);
  await page.keyboard.press('Escape');
  await button(page.locator('.message-assistant').last(),'查看本轮').click();
  const detail = page.locator('#desk-content [data-run-id="' + last.run_id + '"]');
  await detail.waitFor();
  assert.match(await detail.innerText(), /150 token/);
  assert.match(await detail.innerText(), /总耗时/);
  const forbidden = await page.request.get(base + '/api/sessions/' + session.id + '/usage',{
    headers:{Authorization:'Bearer other-token'},
  });
  assert.equal(forbidden.status(),404);
  await login(page,'admin-token',link);
  await metric('tokens').filter({hasText:/^250$/}).waitFor();
  await page.locator('#suggest-next').click();
  await metric('tokens').filter({hasText:/^280$/}).waitFor();
  const afterSuggestion = await read('sessions/' + session.id + '/usage');
  assert.deepEqual(afterSuggestion.runs.map(r=>r.tokens.total).sort((a,b)=>a-b), [100,150]);
  await page.setViewportSize({width:390,height:844});
  await meter.locator('summary').click();
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  await page.keyboard.press('Escape');
  await page.setViewportSize({width:1280,height:840});
}

async function pluginSuggestion(p) {
  const suggestion = {label: '查看 <em>状态</em>', prompt: '请列出 <strong>当前状态</strong>。'};
  const tools = await read('agents/desktop/tools','user-token');
  assert.deepEqual(tools.find(t => t.id === 'inspection.status').webui.suggestion,suggestion);
  const reader = await read('agents/author/tools','user-token');
  assert.ok(reader.every(t => t.id !== 'inspection.status'));
  await p.locator('#agent-select').selectOption('author');
  await p.locator('#new-session:not(:disabled)').waitFor();
  assert.equal(await button(p, suggestion.label).count(),0);
  await p.locator('#agent-select').selectOption('desktop');
  const card = button(p, suggestion.label);
  await card.waitFor();
  assert.equal(await card.textContent(),suggestion.label);
  assert.equal(await card.locator('*').count(),0);
  const before = await read('events', 'user-token');
  await card.click();
  await p.waitForFunction(text => document.querySelector('#prompt').value === text,suggestion.prompt);
  assert.deepEqual(await read('events', 'user-token'),before);
  const disabled = await p.request.patch(base + '/api/plugins/inspection',{
    headers: {Authorization:'Bearer admin-token'},data:{enabled:false},
  });
  assert.ok(disabled.ok());
  await p.locator('#prompt').fill('');
  await card.click();
  await p.getByText('可用工具已更新，请重新选择。',{exact:true}).waitFor();
  assert.equal(await card.count(),0);
  assert.equal(await p.locator('#prompt').inputValue(),'');
}

async function keyboardAndRendering(p) {
  await p.locator('#agent-select').selectOption('desktop');
  await p.locator('#new-session').click();
  const prompt = p.locator('#prompt');
  await prompt.fill('@home.au');
  await p.locator('#compose-picker [role=option]').filter({hasText:'home.audit'}).waitFor();
  await prompt.press('Enter');
  assert.equal(await prompt.inputValue(), 'home.audit ');
  await prompt.fill('/目标');
  await prompt.press('Enter');
  assert.equal(await p.locator('#goal-toggle').getAttribute('aria-pressed'), 'true');
  await p.locator('#goal-toggle').click();
  await p.keyboard.press('Control+k');
  await p.locator('#quick-query').fill('接入 OI');
  await p.locator('#quick-query').press('Enter');
  await p.getByRole('button', {name:'连接 Loom OI', exact:true}).waitFor();
  await p.locator('#desk-chat').click();
  const job=await send(p,'测试表格渲染');
  const firstRun = await waitRun(job.run_id,'completed');
  await p.locator('.message-assistant td strong').getByText('记录',{exact:true}).waitFor();
  assert.equal(await p.locator('.md-code pre code').textContent(),'<b>原文</b>');
  assert.equal(await p.locator('.md-code pre b').count(),0);
  const answer = p.locator('.message-assistant').first();
  const initial = await read('sessions/' + firstRun.session_id + '/context/items');
  const later = await send(p, '继续本轮验收');
  await waitRun(later.run_id, 'completed');
  await p.locator('.message-assistant').nth(1).waitFor();
  const inspected = p.waitForResponse(r => r.url().endsWith('/api/runs/' + job.run_id));
  await button(answer, '查看本轮').click();
  assert.equal((await (await inspected).json()).id, job.run_id);
  await p.locator('#desk-content .run-usage-details').waitFor();
  await p.locator('#desk-chat').click();
  const created = p.waitForResponse(r => r.request().method()==='POST' && r.url().endsWith('/branches'));
  await button(answer,'从此处分支').click();
  const branch = await (await created).json();
  assert.equal(branch.parent_session_id, firstRun.session_id);
  assert.deepEqual(await read('sessions/' + branch.id + '/context/items'), initial);
}

async function catalog(p) {
  await nav(p, 'tools');
  await button(p, '创建独立工具').click();
  await field(p, 'id').fill('browser-check');
  await field(p, 'description').fill('浏览器测试');
  await field(p, 'code').fill('return function(ctx,args) return true end');
  await p.locator('#desk-form summary').filter({hasText: '输入框建议'}).click();
  await field(p, 'suggestion_label').fill('查看工作状态');
  await field(p, 'suggestion_prompt').fill('请检查当前工作状态。');
  await submit(p);
  let entry = (await read('catalog/tools')).find((t) => t.id === 'home.browser-check');
  assert.deepEqual(entry.webui.suggestion, {label: '查看工作状态', prompt: '请检查当前工作状态。'});
  await button(p, '编辑工具').click();
  await p.locator('#desk-form summary').filter({hasText: '输入框建议'}).click();
  await field(p, 'suggestion_label').fill('');
  await field(p, 'suggestion_prompt').fill('');
  await submit(p);
  entry = (await read('catalog/tools')).find((t) => t.id === 'home.browser-check');
  assert.equal(entry.webui?.suggestion, undefined);
  await button(p, '工具集').click();
  await button(p, '创建工具集').click();
  await field(p, 'id').fill('browser-set');
  await field(p, 'members').selectOption('home.browser-check');
  await submit(p);
  const sets = await read('catalog/toolsets');
  assert.deepEqual(sets.find((s) => s.id === 'home.browser-set').members, ['home.browser-check']);

  await nav(p, 'plugins');
  await button(p, '连接 Loom OI').click();
  await field(p, 'instance').fill('test-office');
  await field(p, 'base_url').fill('http://127.0.0.1:9');
  await field(p, 'catalog_service').fill('catalog');
  await field(p, 'catalog_credential_ref').fill('test-catalog');
  await button(p.locator('#desk-dialog'), '连接并发现服务').click();
  await p.locator('.desk-form-error').filter({hasText: /请设置默认调用凭据/}).waitFor();
  await button(p, '添加专用凭据').click();
  await p.getByLabel('Service 名称', {exact: true}).fill('sample');
  await p.getByLabel('凭据引用', {exact: true}).fill('test-consumer');
  await submit(p, '连接并发现服务');
  await p.locator('.oi-notice').filter({hasText: /LOOM_MCP_CREDENTIAL_TEST_CATALOG|暂时无法连接 OIS/}).waitFor();
  assert.equal((await read('settings/test-office')).catalog_service, 'catalog');
  assert.deepEqual((await read('settings/test-office')).service_credentials, {sample: 'test-consumer'});
  assert.equal((await read('plugins/test-office')).enabled, true);

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
  await p.locator('#nav-chats').click();
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

async function inaccessibleLinks(p) {
  const headers = {Authorization:'Bearer other-token'};
  const agent = await p.request.post(base + '/api/agents', {headers, data:{id:'private-ops'}});
  assert.ok(agent.ok());
  const session = await p.request.post(base + '/api/sessions', {headers, data:{agent_id:'private-ops'}});
  assert.ok(session.ok());
  const foreign = await session.json();
  await p.setViewportSize({width:1440, height:900});
  await login(p, 'user-token');
  for (const [link, notice] of [
    ['view=plugins', /管理员身份/],
    ['agent=private-ops', /无法打开指定的 Agent/],
    ['agent=desktop&session=' + foreign.id, /无法打开指定的会话/],
  ]) {
    await p.goto(base + '/webui/#' + link);
    await p.waitForURL(url => url.hash === '#view=overview');
    await p.locator('#desk-status').filter({hasText:notice}).waitFor();
    assert.equal(await p.locator('#prompt').isVisible(), false);
  }
}

(async () => {
  assert.ok(base, 'Launch with workbench_e2e.py');
  const browser = await chromium.launch({ executablePath: process.env.LOOM_BROWSER_EXECUTABLE || undefined });
  try {
    const p = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    p.setDefaultTimeout(12000);
    const errors = [];
    p.on('pageerror', (err) => errors.push(err.message));
    await login(p, 'user-token');
    await p.locator('#overview-runs').waitFor();
    assert.equal(await p.locator('#desk-nav [data-section=plugins]').count(), 0);
    await p.locator('#nav-chats').click();
    await pluginSuggestion(p);
    console.log('PASS Lua plugin Toolset exposes a plain-text draft only to the authorized Agent and expires on disable');
    await login(p, 'admin-token');
    await consoleWorkflow(p);
    console.log('PASS console overview, Run drill-down, mobile management, and conversation draft across a live stream');
    await keyboardAndRendering(p);
    console.log('PASS keyboard navigation, scoped tools, slash actions and Markdown');
    await usageFigures(p);
    console.log('PASS two-run session usage, speed, cache, mobile and reload');
    await catalog(p);
    console.log('PASS catalog, installed packages, scheduling, HOME');
    await creator(p);
    console.log('PASS creator edits its own Agent spec via a traced chat ToolCall');
    await conversation(p);
    console.log('PASS attachment, suggestion, branch, context, ModelCall and Event');
    await approvalAndGoal(p);
    console.log('PASS approval, result original, Note line range, Goal and mobile navigation');
    await inaccessibleLinks(p);
    console.log('PASS restricted management, Agent and Session links return to overview');
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
