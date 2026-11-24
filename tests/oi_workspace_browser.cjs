// Invoked by oi_workspace_e2e.py against the live Harness, OIS and OIC.
const assert = require('node:assert/strict');
const { chromium } = require(process.env.LOOM_PLAYWRIGHT_MODULE || 'playwright');

const group = 'office.workbench';
const members = ['capture_a', 'view_on_a', 'list_skills', 'read_skill'].map(name => group + '.' + name);

function required(name) {
  assert.ok(process.env[name], 'Missing ' + name);
  return process.env[name];
}

async function login(page, token) {
  await page.goto(new URL('/webui/', required('LOOM_BROWSER_BASE')).href);
  await page.locator('#connection-button').click();
  await page.locator('#token-input').fill(token);
  await page.locator('#credential-save').click();
  await page.locator('#credential-dialog').waitFor({ state: 'hidden' });
  await page.locator('#nav-workspace').click();
  await page.locator('#desk-nav').getByRole('button', { name: '运行', exact: true }).waitFor();
}

function nav(page, name) {
  return page.locator('#desk-nav').getByRole('button', { name, exact: true });
}

function row(page, name) {
  return page.locator('#desk-content button.desk-row').filter({
    has: page.getByText(name, { exact: true }),
  });
}

async function details(page, name) {
  const box = page.locator('#desk-content details').filter({
    has: page.getByText(name, { exact: true }),
  });
  await box.waitFor();
  if (!(await box.evaluate(element => element.open))) await box.locator('summary').click();
  const text = box.locator('pre');
  await text.waitFor();
  return JSON.parse(await text.innerText());
}

async function bind(page) {
  await login(page, 'admin-token');
  await nav(page, '插件').click();
  await row(page, 'office').click();
  const service = page.locator('article.oi-service[data-service="' + group + '"]');
  await service.getByText('可用', { exact: true }).waitFor();
  await service.locator('summary').click();
  for (const id of members) await service.getByRole('button', {name:id.slice(group.length + 1), exact:true}).waitFor();
  await service.getByRole('button', { name: '连接到 Agent' }).click();
  const dialog = page.getByRole('dialog', { name: '将 workbench 连接到 Agent' });
  await dialog.getByRole('combobox').selectOption('reader');
  await dialog.getByRole('button', { name: '授权并绑定', exact: true }).click();
  await dialog.waitFor({ state: 'hidden' });
  await page.locator('#desk-refresh').click();
  const tryInChat = service.locator('button[data-chat-agent="reader"]');
  await tryInChat.waitFor();
  await tryInChat.click();
  await page.waitForFunction(() => document.querySelector('#prompt').value.includes('list_skills'));
  await page.locator('#nav-workspace').click();
  await nav(page, 'Agent').click();
  await row(page, 'reader').click();
  const agent = await details(page, 'Agent spec');
  assert.deepEqual(agent.tools, [{ toolset: group }]);
  assert.ok(agent.grants.toolsets.includes(group));
  assert.ok(agent.grants.operations.includes('context.preview'));
  for (const id of members) await row(page, id).waitFor();
  console.log('Browser verified OI discovery and bound the published workbench to reader.');
}

async function trace(page) {
  const runId = required('LOOM_BROWSER_TRACE_RUN');
  const callId = required('LOOM_BROWSER_TRACE_CALL');
  const oiId = required('LOOM_BROWSER_OI_CALL');
  await login(page, 'user-token');
  assert.equal(await nav(page, '插件').count(), 0, 'Plugin administration must be hidden for the consumer');
  await nav(page, '运行').click();
  await row(page, runId).click();
  await row(page, callId).click();
  const call = await details(page, '调用来源与完整记录');
  assert.equal(call.id, callId);
  assert.equal(call.run_id, runId);
  assert.equal(call.oi_call_id, oiId);
  assert.equal(call.tool_id, group + '.view_on_a');
  assert.equal(call.binding.service, 'workbench');
  assert.deepEqual(call.binding.source, { channel: 'host-a', app: 'filesystem', action: 'view_image' });
  assert.ok(call.binding_revision);
  const result = await details(page, '结果');
  assert.ok(result.content.some(item => item.type === 'image'), 'The OI result should contain an image');

  // Check the server boundary too: a hidden menu alone cannot enforce permissions.
  const denied = await page.request.get(new URL('/api/plugins/office/discovery', page.url()).href, {
    headers: { Authorization: 'Bearer user-token' },
  });
  assert.equal(denied.status(), 403, 'The discovery endpoint must reject the consumer credential');
  console.log('Browser verified the consumer view, tool source, image result and OI call ' + oiId + '.');
}

async function main() {
  const phase = required('LOOM_BROWSER_PHASE');
  assert.ok(['bind', 'trace'].includes(phase), 'LOOM_BROWSER_PHASE must be bind or trace');
  const browser = await chromium.launch({ executablePath: process.env.LOOM_BROWSER_EXECUTABLE || undefined });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    page.setDefaultTimeout(12000);
    const pageErrors = [];
    page.on('pageerror', err => pageErrors.push(err.message));
    if (phase === 'bind') await bind(page);
    else await trace(page);
    assert.deepEqual(pageErrors, [], 'Workspace JavaScript errors');
  } finally {
    await browser.close();
  }
}

main().catch(err => {
  console.error(err);
  process.exitCode = 1;
});
