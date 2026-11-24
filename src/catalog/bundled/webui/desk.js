/* Workspace views use the same authenticated client as the conversation. */
window.createLoomDesk = function createLoomDesk(client) {
  'use strict';
  const byId = (id) => document.getElementById(id);
  const panel = byId('desk-panel');
  const nav = byId('desk-nav');
  const editor = byId('desk-dialog');
  const eid = encodeURIComponent;
  const label = {
    agents: 'Agent',
    plugins: '插件',
    tools: '工具目录',
    sets: '工具集',
    connections: '连接',
    runs: '运行',
    events: '事件',
    dispatches: '调度',
    host: '宿主',
    session: '会话详情',
    toolcall: '工具调用',
    modelcall: '模型调用',
    eventDelivery: '投递',
    reply: '回复',
    agentTool: 'Agent 工具',
  };
  const symbol = {
    agents: '◉',
    plugins: '⊞',
    tools: '⌘',
    runs: '▷',
    events: '≋',
    dispatches: '◷',
    host: '▤',
  };
  let identity = null;
  let paths = {};
  let current = null;
  let revision = 0;
  let reader = null;
  let sequence = [];
  let lastFocus = null;
  const node = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined && text !== null) n.textContent = String(text);
    return n;
  };
  const print = (value) => (typeof value === 'string' ? value : JSON.stringify(value, null, 2));
  function button(title, action, cls = 'desk-button') {
    const b = node('button', cls, title);
    b.type = 'button';
    b.addEventListener('click', async () => {
      if (b.disabled) return;
      b.disabled = true;
      try {
        await action();
      } catch (e) {
        if (b.isConnected) notice(e.message, true);
      } finally {
        b.disabled = false;
      }
    });
    return b;
  }
  function notice(text, error = false) {
    const out = byId('desk-status');
    out.textContent = text;
    out.classList.toggle('error', error);
  }
  const permitted = (path, method = 'get') => !!paths['/api/' + path]?.[method];
  function time(value) {
    if (!value) return '—';
    const d = new Date(typeof value === 'number' && value < 1e12 ? value * 1000 : value);
    return Number.isNaN(d.getTime()) ? String(value) : d.toLocaleString('zh-CN', { hour12: false });
  }
  function group(title, ...children) {
    const box = node('section', 'desk-group');
    if (title) box.append(node('h3', '', title));
    box.append(...children.filter(Boolean));
    return box;
  }
  function data(title, value, opened = false) {
    const box = node('details', 'desk-data');
    box.open = opened;
    box.append(node('summary', '', title));
    const render = () => {
      if (box.open && box.childElementCount === 1) box.append(node('pre', '', print(value) ?? '—'));
    };
    box.addEventListener('toggle', render);
    render();
    return box;
  }
  function imageBlock(part) {
    const mime = part?.mimeType || part?.mime_type;
    if (
      part?.type !== 'image' ||
      !['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(mime) ||
      typeof part.data !== 'string' ||
      !part.data
    )
      return null;
    const img = node('img', 'desk-inline-image');
    img.loading = 'lazy';
    img.alt = '工具返回的图片';
    img.src = 'data:' + mime + ';base64,' + part.data;
    return img;
  }
  function facts(entries) {
    const dl = node('dl', 'desk-facts');
    entries
      .filter(([, v]) => v !== undefined && v !== null && v !== '')
      .forEach(([k, v]) => {
        dl.append(node('dt', '', k), node('dd', '', typeof v === 'object' ? print(v) : v));
      });
    return dl;
  }
  function controls(...buttons) {
    const bar = node('div', 'desk-controls');
    bar.append(...buttons.filter(Boolean));
    return bar;
  }
  function link(title, section, id) {
    return button(title, () => open(section, id), 'desk-link');
  }
  const stateNames = {
    completed: '已完成',
    running: '运行中',
    failed: '失败',
    queued: '排队中',
    waiting: '等待中',
    awaiting_approval: '等待审批',
    pending_approval: '等待审批',
    cancelled: '已取消',
    accepted: '已受理',
    waiting_delivery: '等待交付',
  };
  const stateTitle = (s) => stateNames[s] || s || '';
  function list(rows, describe, empty = '暂无记录') {
    const box = node('div', 'desk-collection');
    if (!rows.length) {
      box.append(node('p', 'desk-empty', empty));
      return box;
    }
    const search = node('input', 'desk-search');
    search.type = 'search';
    search.placeholder = '筛选当前列表';
    search.setAttribute('aria-label', '筛选当前列表');
    const all = rows.map(describe);
    const results = node('div', 'desk-rows');
    function show() {
      results.replaceChildren();
      const q = search.value.trim().toLowerCase();
      for (const item of all) {
        const [title, sub, tag, action] = item;
        if (![title, sub, tag].filter(Boolean).join(' ').toLowerCase().includes(q)) continue;
        const row = action ? button('', action, 'desk-row') : node('div', 'desk-row');
        const copy = node('span', 'desk-row-copy');
        copy.append(node('strong', '', title || '—'));
        if (sub) copy.append(node('small', '', sub));
        row.append(copy);
        if (tag) row.append(node('span', 'desk-tag', tag));
        if (action) row.append(node('span', 'desk-chevron', '›'));
        results.append(row);
      }
      if (!results.childElementCount) results.append(node('p', 'desk-empty', '未找到匹配项'));
    }
    search.addEventListener('input', show);
    show();
    if (rows.length > 3) box.append(search);
    box.append(results);
    return box;
  }
  function reset() {
    identity = null;
    paths = {};
    sequence = [];
    close();
    dismiss();
    nav.replaceChildren();
  }
  function close() {
    ++revision;
    reader?.abort();
    current = null;
    panel.hidden = true;
    document.body.classList.remove('in-desk');
    byId('desk-content').replaceChildren();
    nav.querySelector('[aria-current]')?.removeAttribute('aria-current');
    if (lastFocus?.isConnected && !document.body.classList.contains('sidebar-open'))
      lastFocus.focus({ preventScroll: true });
    lastFocus = null;
  }
  function dismiss() {
    if (editor.open) editor.close();
  }
  function connect(id, api) {
    identity = id;
    paths = api.paths || {};
    nav.replaceChildren();
    const sections = [
      'agents',
      ...(id.admin ? ['plugins'] : []),
      'tools',
      'runs',
      'events',
      'dispatches',
      'host',
    ];
    for (const section of sections) {
      const b = button('', () => open(section), 'desk-nav-item');
      const glyph = node('span', 'desk-glyph', symbol[section]);
      glyph.setAttribute('aria-hidden', 'true');
      b.append(glyph, node('span', '', label[section]));
      b.dataset.section = section;
      nav.append(b);
    }
  }
  function field(name, title, value = '', type = 'text', options = {}) {
    return { name, title, value, type, ...options };
  }
  function edit(title, fields, commit, submit = '保存') {
    const form = byId('desk-form');
    form.replaceChildren();
    editor.setAttribute('aria-label', title);
    const head = node('div', 'desk-dialog-head');
    head.append(node('h2', '', title), button('关闭', dismiss));
    form.append(head);
    const inputs = [];
    for (const f of fields) {
      const wrap = node('label', 'desk-field');
      wrap.append(node('span', '', f.title));
      const wide = ['json', 'textarea', 'code', 'multiple'].includes(f.type);
      const inp = node(f.type === 'select' || f.type === 'multiple' ? 'select' : wide ? 'textarea' : 'input');
      if (f.type === 'select' || f.type === 'multiple') {
        if (f.type === 'multiple') inp.multiple = true;
        (f.options || []).forEach((o) => {
          const [v, t] = Array.isArray(o) ? o : [o, o];
          inp.add(
            new Option(
              t,
              v,
              false,
              f.type === 'multiple' ? f.value.includes(v) : String(f.value) === String(v),
            ),
          );
        });
      } else if (f.type === 'checkbox') {
        inp.type = 'checkbox';
        inp.checked = !!f.value;
      } else {
        if (inp.tagName === 'INPUT') inp.type = f.type;
        inp.value = f.type === 'json' ? JSON.stringify(f.value, null, 2) : (f.value ?? '');
        if (f.type === 'json' || f.type === 'code') inp.className = 'desk-code';
      }
      if (f.placeholder) inp.placeholder = f.placeholder;
      inp.name = f.name;
      inp.required = !!f.required;
      inp.disabled = !!f.disabled;
      wrap.append(inp);
      if (f.hint) wrap.append(node('small', '', f.hint));
      form.append(wrap);
      inputs.push([f, inp]);
    }
    const err = node('p', 'desk-form-error');
    err.setAttribute('role', 'alert');
    const save = node('button', 'desk-button primary', submit);
    save.type = 'submit';
    form.append(err, controls(button('取消', dismiss), save));
    const owner = identity;
    form.onsubmit = async (ev) => {
      ev.preventDefault();
      if (save.disabled || identity !== owner) return;
      const values = {};
      err.textContent = '';
      try {
        for (const [f, i] of inputs) {
          if (f.type === 'json') {
            try {
              values[f.name] = JSON.parse(i.value);
            } catch {
              throw Error(f.title + ' 的 JSON 格式有误');
            }
          } else
            values[f.name] =
              f.type === 'checkbox'
                ? i.checked
                : f.type === 'multiple'
                  ? Array.from(i.selectedOptions, (x) => x.value)
                  : i.value;
        }
        save.disabled = true;
        await commit(values);
        if (identity === owner && save.isConnected) dismiss();
      } catch (e) {
        if (save.isConnected) err.textContent = e.message;
      } finally {
        save.disabled = false;
      }
    };
    if (!editor.open) editor.showModal();
    form.querySelector('input:not(:disabled),select:not(:disabled),textarea:not(:disabled)')?.focus();
  }
  const call = (path, method = 'GET', body, key) => client.request(path, { method, body, key });
  const activeRead = (path, opts = {}) => client.request(path, { signal: reader.signal, ...opts });
  const move = (section, id) => () => open(section, id);
  function packagesMenu() {
    return controls(
      link('工具', 'tools'),
      identity.admin && link('工具集', 'sets'),
      identity.admin && link('连接', 'connections'),
    );
  }
  function editThen(section, id) {
    return async () => open(section, id, { replace: true });
  }
  async function open(section, id, options = {}) {
    if (!identity) {
      client.signIn();
      return;
    }
    if (!label[section]) return;
    const before = current;
    if (before && !options.replace && (before.section !== section || before.id !== id))
      sequence.push({ ...before, position: panel.scrollTop });
    if (!before) {
      sequence = [];
      lastFocus = document.activeElement;
    }
    reader?.abort();
    reader = new AbortController();
    const turn = ++revision;
    current = { section, id };
    client.closeDrawer();
    document.body.classList.add('in-desk');
    panel.hidden = false;
    nav.querySelector('[aria-current]')?.removeAttribute('aria-current');
    const parent =
      {
        sets: 'tools',
        connections: 'tools',
        toolcall: 'runs',
        modelcall: 'runs',
        eventDelivery: 'events',
        reply: 'events',
        session: 'runs',
      }[section] || section;
    nav.querySelector('[data-section="' + parent + '"]')?.setAttribute('aria-current', 'page');
    byId('desk-back').hidden = !sequence.length && !id;
    byId('desk-title').textContent = label[section];
    byId('desk-heading').textContent = id || label[section];
    byId('desk-content').replaceChildren();
    notice('正在读取…');
    panel.scrollTop = 0;
    try {
      const parts = await views[section](id);
      if (turn !== revision) return;
      byId('desk-content').replaceChildren(...parts.filter(Boolean));
      notice('');
      if (options.position) panel.scrollTop = options.position;
      if (!options.replace) byId('desk-heading').focus({ preventScroll: true });
    } catch (e) {
      if (turn === revision) {
        notice(e.message, true);
        byId('desk-content').append(button('重新读取', editThen(section, id)));
      }
    }
  }
  byId('desk-back').addEventListener('click', () => {
    const prev = sequence.pop();
    if (prev) open(prev.section, prev.id, { replace: true, position: prev.position });
    else if (current?.id) open(current.section, undefined, { replace: true });
    else close();
  });
  byId('desk-refresh').addEventListener(
    'click',
    () => current && open(current.section, current.id, { replace: true, position: panel.scrollTop }),
  );
  byId('desk-chat').addEventListener('click', close);

  const ident = (name = 'id', title = '标识', value = '') =>
    field(name, title, value, 'text', { required: true });
  function rev(item) {
    return item.revision ? { expected_revision: item.revision } : {};
  }
  function bindingKeys(items = []) {
    return items
      .map((i) => (typeof i === 'string' ? 'tool:' + i : i.toolset ? 'set:' + i.toolset : 'tool:' + i.tool))
      .filter(Boolean);
  }
  function agentToolRows(tools, agent) {
    return list(
      tools,
      (t) => [
        t.id,
        t.description,
        t.source,
        identity.admin ? move('tools', t.id) : move('agentTool', JSON.stringify([agent, t.id])),
      ],
      '这个 Agent 当前没有获授的工具',
    );
  }
  async function editAgent(agent = null) {
    const [templates, models] = await Promise.all([activeRead('agent-templates'), activeRead('models')]);
    const model = agent?.model;
    const currentModel = (typeof model === 'string' ? model : model?.default) || '';
    const form = [
      ...(!agent
        ? [
            ident(),
            field('template', '模板', '0', 'select', {
              options: templates.map((t, i) => [String(i), t.id + ' · ' + t.source]),
            }),
          ]
        : []),
      field('name', '名称', agent?.name || ''),
      field('description', '描述', agent?.description || '', 'textarea'),
      field('model', '默认模型', currentModel, 'select', {
        options: [
          ['', '继承宿主默认值'],
          ...Array.from(new Set([...models.map((x) => x.id), ...(currentModel ? [currentModel] : [])])),
        ],
      }),
      field('fallbacks', '备用模型（逗号分隔）', model?.fallbacks?.join(', ') || ''),
      field('system_prompt', '系统指令', agent?.system_prompt || '', 'textarea'),
    ];
    if (identity.admin) form.push(field('grants', '授权范围', agent?.grants || {}, 'json'));
    edit(agent ? '编辑 Agent' : '创建 Agent', form, async (v) => {
      const patch = { name: v.name, description: v.description, system_prompt: v.system_prompt };
      if (v.model || agent?.model || v.fallbacks.trim())
        patch.model = {
          ...(typeof model === 'object' ? model : {}),
          default: v.model || null,
          fallbacks: v.fallbacks
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean),
        };
      if (identity.admin) patch.grants = v.grants;
      if (!agent) {
        const t = templates[Number(v.template)];
        patch.id = v.id;
        patch.template = { id: t.id, source: t.source, version: t.version };
        if (!v.name) delete patch.name;
        if (!v.system_prompt) delete patch.system_prompt;
        if (!v.description) delete patch.description;
      }
      const saved = await call(agent ? 'agents/' + eid(agent.id) : 'agents', agent ? 'PATCH' : 'POST', {
        ...patch,
        ...(agent ? rev(agent) : {}),
      });
      await client.reloadAgents();
      await open('agents', saved.id, { replace: true });
    });
  }
  async function bind(a) {
    const existing = bindingKeys(a.tools);
    const [ts, tools] = identity.admin
      ? await Promise.all([activeRead('catalog/toolsets'), activeRead('catalog/tools')])
      : [[], await activeRead('agents/' + eid(a.id) + '/tools')];
    const options = Array.from(
      new Set([...existing, ...ts.map((x) => 'set:' + x.id), ...tools.map((x) => 'tool:' + x.id)]),
    );
    const fields = [
      field('choices', '工具集与单个工具', existing, 'multiple', {
        options: options.map((x) => [
          x,
          (x.startsWith('set:') ? '工具集 · ' : '工具 · ') + x.slice(x.indexOf(':') + 1),
        ]),
        hint: 'Ctrl 或 ⌘ 可选择多项。授权范围决定生效成员。',
      }),
      field('extra', '其他已知工具集 ID（逗号分隔）', ''),
    ];
    edit('绑定工具', fields, async (v) => {
      const keys = new Set([
        ...v.choices,
        ...v.extra
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean)
          .map((s) => 'set:' + s),
      ]);
      const bindings = Array.from(keys, (x) =>
        x.startsWith('set:') ? { toolset: x.slice(4) } : { tool: x.slice(5) },
      );
      await call('agents/' + eid(a.id), 'PATCH', { ...rev(a), tools: bindings });
      await client.reloadAgents();
      await open('agents', a.id, { replace: true });
    });
  }
  async function agentView(id) {
    if (!id) {
      const rows = await activeRead('agents');
      return [
        controls(button('创建 Agent', () => editAgent(), 'desk-button primary')),
        list(
          rows,
          (a) => [
            a.name || a.id,
            [a.id, a.description].filter(Boolean).join(' · '),
            a.template?.id,
            move('agents', a.id),
          ],
          '当前账户还没有 Agent',
        ),
      ];
    }
    const [a, tools] = await Promise.all([
      activeRead('agents/' + eid(id)),
      activeRead('agents/' + eid(id) + '/tools'),
    ]);
    return [
      controls(
        button('与 Agent 对话', () => client.chat(id), 'desk-button primary'),
        button('编辑 Agent', () => editAgent(a)),
        button('绑定工具', () => bind(a)),
      ),
      facts([
        ['名称', a.name],
        ['标识', id],
        ['描述', a.description || '—'],
        ['模型', typeof a.model === 'string' ? a.model : a.model?.default || '宿主默认'],
        ['模板', a.template?.id],
        ['所有者', a.owner],
      ]),
      data('系统指令', a.system_prompt || '使用默认指令'),
      data('工具绑定与授权', { tools: a.tools, grants: a.grants }),
      group('实际可用工具', agentToolRows(tools, id)),
      data('Agent spec', a),
    ];
  }
  function installVersion(defaultId = '') {
    edit(
      '安装插件版本',
      [
        ident('id', '包标识', defaultId),
        ident('version', '版本', '1.0.0'),
        field('manifest', '包清单', {}, 'json'),
        field('code', 'plugin.lua', '', 'code', { required: true }),
        field('templates', '模板', [], 'json'),
      ],
      async (v) => {
        const { id, ...bundle } = v;
        await call('plugins/' + eid(id) + '/versions', 'POST', bundle);
        if (current?.section === 'plugins') await open('plugins', current.id, { replace: true });
      },
      '安装',
    );
  }
  function instanceForm(p, pkg) {
    const fields = [
      ...(!p ? [ident()] : []),
      ident('package', '包标识', p?.package || pkg?.id || ''),
      ident('version', '启用版本', p?.version || pkg?.version || '1.0.0'),
      field('enabled', '启用实例', p?.enabled || false, 'checkbox'),
      field('allowed_owners', '可访问的所有者（留空沿用默认范围）', p?.allowed_owners?.join(', ') || ''),
    ];
    edit(p ? '编辑插件实例' : '创建插件实例', fields, async (v) => {
      const payload = { ...v, ...(p ? rev(p) : {}) };
      if (v.allowed_owners.trim() || p?.allowed_owners)
        payload.allowed_owners = v.allowed_owners
          .split(',')
          .map((x) => x.trim())
          .filter(Boolean);
      else delete payload.allowed_owners;
      const saved = await call(p ? 'plugins/' + eid(p.id) : 'plugins', p ? 'PATCH' : 'POST', payload);
      await open('plugins', saved.id, { replace: true });
    });
  }
  function externalAction(title, run) {
    const output = node('div', 'desk-result');
    const b = button(title, async () => {
      const value = await run();
      output.replaceChildren(data(title, value, true));
    });
    return group('', controls(b), output);
  }
  function pluginSettings(id, settings, schema) {
    const config = { ...settings };
    delete config.revision;
    const properties = Object.entries(schema?.properties || {});
    const titles = {
      base_url: 'OIS 地址',
      catalog_service: '目录 Service',
      catalog_credential_ref: '目录凭据引用',
      consumer_credential_ref: '业务消费凭据引用',
      service_credentials: '按 Service 指定凭据引用',
      refresh_interval_secs: '刷新间隔（秒）',
      principals: '各身份的采样设置',
    };
    const fs = properties.map(([key, s]) => {
      const options = { required: schema.required?.includes(key), hint: s.description };
      if (s.enum)
        return field(key, titles[key] || key, config[key] ?? s.default ?? s.enum[0], 'select', {
          ...options,
          options: s.enum,
        });
      if (s.type === 'object' || s.type === 'array')
        return field(
          key,
          titles[key] || key,
          config[key] ?? s.default ?? (s.type === 'array' ? [] : {}),
          'json',
          options,
        );
      if (s.type === 'boolean')
        return field(key, titles[key] || key, config[key] ?? s.default ?? false, 'checkbox', options);
      return field(
        key,
        titles[key] || key,
        config[key] ?? s.default ?? '',
        s.type === 'number' || s.type === 'integer' ? 'number' : 'text',
        options,
      );
    });
    if (!fs.length) fs.push(field('settings', '实例设置', config, 'json'));
    edit('编辑插件设置', fs, async (v) => {
      const changed = properties.length ? { ...config } : v.settings;
      for (const [k, s] of properties) {
        if (v[k] === '' && !(k in config)) continue;
        changed[k] = s.type === 'integer' || s.type === 'number' ? Number(v[k]) : v[k];
      }
      await call('settings/' + eid(id), 'PATCH', { ...changed, ...rev(settings) });
      await open('plugins', id, { replace: true });
    });
  }
  async function pluginsView(id) {
    if (!id) {
      const [rows, packages] = await Promise.all([activeRead('plugins'), activeRead('catalog/packages')]);
      return [
        controls(
          button('创建实例', () => instanceForm(null), 'desk-button primary'),
          button('安装插件版本', () => installVersion()),
        ),
        group(
          '实例',
          list(rows, (p) => [
            p.id,
            p.package + ' · ' + p.version,
            p.enabled ? '已启用' : '已停用',
            move('plugins', p.id),
          ]),
        ),
        group(
          '从已安装包创建实例',
          list(packages, (p) => [
            p.id,
            p.description || p.id,
            (p.official ? '官方 · ' : '') + p.version,
            () => instanceForm(null, p),
          ]),
        ),
      ];
    }
    const p = await activeRead('plugins/' + eid(id));
    const [settings, versions, schema] = await Promise.all([
      activeRead('settings/' + eid(id)),
      activeRead('plugins/' + eid(p.package) + '/versions'),
      activeRead('settings/' + eid(id) + '/schema'),
    ]);
    const installed = versions.find((v) => v.version === p.version) || {};
    const sections = [
      controls(
        button('编辑实例', () => instanceForm(p)),
        button('编辑设置', () => pluginSettings(id, settings, schema)),
        button('安装版本', () => installVersion(p.package)),
      ),
      facts([
        ['状态', p.enabled ? '已启用' : '已停用'],
        ['包', p.package],
        ['生效版本', p.version],
        ['说明', installed.manifest?.description],
      ]),
      data('配置与设置约束', { settings, schema }),
      group(
        '已安装的版本',
        list(versions, (v) => [
          v.version,
          v.manifest?.description || p.package,
          v.version === p.version ? '当前版本' : '',
        ]),
      ),
      data('插件贡献', {
        tools: installed.tools,
        toolsets: installed.toolsets,
        templates: installed.templates,
        routes: installed.routes,
        manifest: installed.manifest,
      }),
    ];
    if (p.package === 'internal-tools') {
      sections.splice(
        1,
        0,
        externalAction('刷新 OI 服务目录', () => call('plugins/' + eid(id) + '/refresh', 'POST', {})),
        externalAction('查看 OI 发现结果', () => activeRead('plugins/' + eid(id) + '/discovery')),
      );
    }
    return sections;
  }
  function toolForm(t) {
    edit(
      t ? '编辑独立工具' : '创建独立工具',
      [
        ...(!t ? [ident()] : []),
        field('description', '说明', t?.description || ''),
        field('input_schema', '输入 Schema', t?.input_schema || { type: 'object', properties: {} }, 'json'),
        field('exposure', '调用方式', t?.direct ? 'direct' : t?.exposure || 'code', 'select', {
          options: [
            ['code', '代码调用'],
            ['direct', '直接调用'],
          ],
        }),
        field('requires_approval', '调用前需要审批', t?.requires_approval || false, 'checkbox'),
        field('allowed_agents', '允许的 Agent ID（逗号分隔）', t?.allowed_agents?.join(', ') || ''),
        field('code', 'Lua 处理器', t?.code || '', 'code', {
          required: !t,
          hint: t ? '留空保留原实现。' : '',
        }),
      ],
      async (v) => {
        v.allowed_agents = v.allowed_agents
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean);
        if (t && !v.code) delete v.code;
        const saved = await call(t ? 'catalog/tools/' + eid(t.id) : 'catalog/tools', t ? 'PATCH' : 'POST', {
          ...v,
          ...(t ? rev(t) : {}),
        });
        await open('tools', saved.id, { replace: true });
      },
    );
  }
  async function toolsView(id) {
    if (!identity.admin) {
      const a = client.currentAgent();
      return a
        ? [group('当前 Agent 的实际工具', agentToolRows(await activeRead('agents/' + eid(a) + '/tools'), a))]
        : [node('p', 'desk-empty', '请先选择 Agent。')];
    }
    const rows = await activeRead('catalog/tools');
    if (!id)
      return [
        packagesMenu(),
        controls(button('创建独立工具', () => toolForm(null), 'desk-button primary')),
        list(rows, (t) => [t.id, t.description, t.source, move('tools', t.id)], '目录中还没有已登记的工具'),
      ];
    const t = rows.find((t) => t.id === id);
    if (!t)
      return [packagesMenu(), node('p', 'desk-empty', '此工具由宿主内置，可从 Agent 的实际工具目录查看。')];
    return [
      packagesMenu(),
      controls((t.source === 'home' || id.startsWith('home.')) && button('编辑工具', () => toolForm(t))),
      facts([
        ['标识', id],
        ['说明', t.description],
        ['来源', t.source],
        ['实例', t.instance],
        ['版本', t.revision],
      ]),
      data('输入定义', t.input_schema, true),
      data('执行配置', t),
      t.revision &&
        externalAction('读取已保存的修订', () =>
          activeRead('catalog/tools/' + eid(id) + '/versions/' + eid(t.revision)),
        ),
    ];
  }
  async function setForm(s) {
    const tools = await activeRead('catalog/tools');
    const members = s?.members || [];
    edit(
      s ? '编辑工具集' : '创建工具集',
      [
        ...(!s ? [ident()] : []),
        field('members', '成员', members, 'multiple', {
          options: Array.from(new Set([...members, ...tools.map((t) => t.id)])),
        }),
        field('more', '其他成员标识（逗号分隔）', ''),
      ],
      async (v) => {
        v.members = Array.from(
          new Set([
            ...v.members,
            ...v.more
              .split(',')
              .map((x) => x.trim())
              .filter(Boolean),
          ]),
        );
        delete v.more;
        const saved = await call(
          s ? 'catalog/toolsets/' + eid(s.id) : 'catalog/toolsets',
          s ? 'PATCH' : 'POST',
          { ...v, ...(s ? rev(s) : {}) },
        );
        await open('sets', saved.id, { replace: true });
      },
    );
  }
  async function setsView(id) {
    const rows = await activeRead('catalog/toolsets');
    if (!id)
      return [
        packagesMenu(),
        controls(button('创建工具集', () => setForm(null), 'desk-button primary')),
        list(rows, (s) => [s.id, String((s.members || []).length) + ' 个工具', s.source, move('sets', s.id)]),
      ];
    const s = rows.find((x) => x.id === id);
    if (!s) throw Error('找不到该工具集');
    return [
      packagesMenu(),
      s.editable && controls(button('编辑成员', () => setForm(s))),
      facts([
        ['来源', s.source],
        ['状态', s.status],
        ['说明', s.reason],
      ]),
      group(
        '成员',
        list(s.members || [], (m) => [m, '', '', move('tools', m)]),
      ),
      data('工具集记录', s),
    ];
  }
  function connectionForm(c) {
    edit(
      c ? '编辑 MCP 连接' : '新建 MCP 连接',
      [
        ...(!c ? [ident(), ident('instance', '插件实例')] : []),
        ident('url', 'MCP 端点', c?.url || ''),
        field('credential_ref', '凭据引用', c?.credential_ref || ''),
        field('receipt_source', '回执来源', c?.receipt_source || ''),
      ],
      async (v) => {
        const target = c ? 'mcp/connections/' + eid(c.instance) + '/' + eid(c.id) : 'mcp/connections';
        const saved = await call(target, c ? 'PATCH' : 'POST', { ...v, ...(c ? rev(c) : {}) });
        await open('connections', JSON.stringify([saved.instance, saved.id]), { replace: true });
      },
    );
  }
  async function connectionsView(id) {
    if (!id)
      return [
        packagesMenu(),
        controls(button('新建连接', () => connectionForm(null), 'desk-button primary')),
        list(await activeRead('mcp/connections'), (c) => [
          c.id,
          c.url,
          c.instance,
          move('connections', JSON.stringify([c.instance, c.id])),
        ]),
      ];
    const [instance, key] = JSON.parse(id),
      path = 'mcp/connections/' + eid(instance) + '/' + eid(key);
    const c = await activeRead(path);
    return [
      packagesMenu(),
      facts([
        ['连接', key],
        ['实例', instance],
        ['地址', c.url],
        ['凭据引用', c.credential_ref],
      ]),
      controls(button('编辑连接', () => connectionForm(c))),
      externalAction('刷新工具发现', () => call(path + '/refresh', 'POST', {})),
      externalAction('上次发现结果', () => activeRead(path + '/discovery')),
    ];
  }

  const runRows = (rows) =>
    list(rows.slice().reverse(), (r) => [
      r.id,
      [r.agent_id, time(r.created_at)].filter(Boolean).join(' · '),
      stateTitle(r.status),
      move('runs', r.id),
    ]);
  function supplement(r, cancel = false) {
    const op = client.uid();
    edit(
      cancel ? '取消运行' : '补充输入',
      [
        field('text', cancel ? '取消说明' : '消息', cancel ? '请求取消本次运行。' : '', 'textarea', {
          required: true,
        }),
      ],
      async (v) => {
        const ev = await call('events', 'POST', { content: v.text }, 'event-' + op);
        await call(
          'events/' + eid(ev.id) + '/deliveries',
          'POST',
          {
            target: { session_id: r.session_id },
            run_id: r.id,
            action: cancel ? 'request_cancel' : 'add_input',
          },
          'delivery-' + op,
        );
        await open('runs', r.id, { replace: true });
      },
      cancel ? '确认取消' : '发送',
    );
  }
  async function runsView(id) {
    if (!id) return [runRows(await activeRead('runs'))];
    const path = 'runs/' + eid(id);
    const [r, calls, models, steps] = await Promise.all([
      activeRead(path),
      activeRead(path + '/tool-calls'),
      activeRead(path + '/model-calls'),
      activeRead(path + '/steps'),
    ]);
    const finished = ['completed', 'cancelled', 'failed'].includes(r.status);
    return [
      facts([
        ['状态', stateTitle(r.status)],
        ['Agent', r.agent_id],
        ['受理时间', time(r.created_at)],
        ['目标', r.goal?.objective],
        ['等待', r.waiting_for],
        ['唤醒原因', r.wake_reason],
        ['停止原因', r.stop_reason || r.error],
      ]),
      controls(
        link('会话详情', 'session', r.session_id),
        link('Agent 详情', 'agents', r.agent_id),
        !finished && button('补充输入', () => supplement(r)),
        !finished && button('取消运行', () => supplement(r, true)),
        r.reply_id && link('交付的回复', 'reply', r.reply_id),
      ),
      group(
        '模型调用',
        list(
          models,
          (m) => [m.model || m.id, m.id, stateTitle(m.status), move('modelcall', m.id)],
          '暂无模型调用',
        ),
      ),
      group(
        '工具调用',
        list(
          calls,
          (c) => [c.tool_id || c.id, c.id, stateTitle(c.status), move('toolcall', c.id)],
          '暂无工具调用',
        ),
      ),
      r.system_prompt_event_id && controls(link('基础系统指令事件', 'events', r.system_prompt_event_id)),
      data('执行时间线', steps),
      data('运行记录', r),
    ];
  }
  async function modelView(id) {
    const m = await activeRead('model-calls/' + eid(id)),
      t = m.timing || {},
      usage = m.usage || {};
    const latency = (v) =>
      v && t.request_started_us ? ((v - t.request_started_us) / 1000).toFixed(1) + ' ms' : null;
    const messages = m.messages || [];
    const hasImage = messages.some(
      (message) =>
        Array.isArray(message.content) && message.content.some((part) => part.type === 'image_url'),
    );
    return [
      facts([
        ['模型', m.model],
        ['状态', stateTitle(m.status)],
        ['调用时间', time(t.request_started_us ? Math.floor(t.request_started_us / 1000) : m.created_at)],
        ['首个推理片段', latency(t.first_reasoning_us)],
        ['首个正文片段', latency(t.first_content_us)],
        ['输入 token', usage.prompt_tokens],
        ['输出 token', usage.completion_tokens],
        ['总 token', usage.total_tokens],
      ]),
      controls(
        m.run_id && link('运行记录', 'runs', m.run_id),
        m.system_prompt_event_id && link('实际系统指令', 'events', m.system_prompt_event_id),
      ),
      data('发送的消息', messages, !hasImage),
      data('可用工具', m.tools || []),
      data('完整模型调用', m),
    ];
  }
  async function toolView(id) {
    const c = await activeRead('tool-calls/' + eid(id));
    const blocks = c.result?.content || [c.result];
    const images = Array.isArray(blocks) ? blocks.map(imageBlock).filter(Boolean) : [];
    const resultButtons = Array.isArray(blocks)
      ? blocks
          .map((part, index) => {
            if (!part) return null;
            return externalAction('读取结果 ' + (index + 1) + ' 原文', () =>
              activeRead('tool-calls/' + eid(id) + '/results/' + index + '/content'),
            );
          })
          .filter(Boolean)
      : [];
    function approval(allow) {
      const op = client.uid();
      edit(
        allow ? '批准工具调用' : '拒绝工具调用',
        [field('arguments', '本次调用参数', c.arguments || {}, 'json', { disabled: true })],
        async () => {
          await call(
            'tool-calls/' + eid(id) + '/approval',
            'POST',
            { allow, arguments: c.arguments },
            'approval-' + op,
          );
          await open('toolcall', id, { replace: true });
        },
        allow ? '确认批准' : '确认拒绝',
      );
    }
    const binding = c.binding || {};
    return [
      facts([
        ['工具', c.tool_id],
        ['状态', stateTitle(c.status)],
        ['时间', time(c.created_at)],
        ['来源', c.source || c.external_dispatch?.service],
        ['OI 调用号', c.oi_call_id],
        ['Service', binding.service],
        ['成员发布名', binding.member],
        ['通道', binding.source?.channel],
        ['App', binding.source?.app],
        ['Action', binding.source?.action],
        ['绑定修订', c.binding_revision],
        ['作业', c.job_id],
      ]),
      controls(
        c.run_id && link('运行记录', 'runs', c.run_id),
        c.session_id && link('会话详情', 'session', c.session_id),
      ),
      identity.approve &&
        ['awaiting_approval', 'pending_approval'].includes(c.status) &&
        controls(
          button('批准', () => approval(true), 'desk-button primary'),
          button('拒绝', () => approval(false)),
        ),
      identity.source &&
        ['accepted', 'dispatched', 'unknown'].includes(c.status) &&
        controls(
          button('提交来源回执', () => {
            const key = client.uid();
            edit(
              '提交工具回执',
              [field('receipt', '回执', { status: 'completed', job_id: c.job_id, result: '' }, 'json')],
              async (v) => {
                await call('tool-calls/' + eid(id) + '/results', 'POST', v.receipt, key);
                await open('toolcall', id, { replace: true });
              },
              '提交',
            );
          }),
        ),
      data('调用参数', c.arguments, true),
      ...images,
      data('结果', c.result || c.error || c.status, !images.length),
      ...resultButtons,
      data('调用来源与完整记录', c),
    ];
  }
  async function eventView(id) {
    if (!id)
      return [
        controls(
          button('创建事件', () => {
            const key = client.uid();
            edit(
              '创建事件',
              [
                field('type', '类型', 'user.message', 'text', { required: true }),
                field('content', '正文', '', 'textarea', { required: true }),
              ],
              async (v) => {
                const e = await call('events', 'POST', v, key);
                await open('events', e.id, { replace: true });
              },
              '创建',
            );
          }),
        ),
        list((await activeRead('events')).reverse(), (e) => [
          e.type || '消息',
          e.id + ' · ' + time(e.created_at),
          e.source,
          move('events', e.id),
        ]),
      ];
    const [ev, ds] = await Promise.all([
      activeRead('events/' + eid(id)),
      activeRead('events/' + eid(id) + '/deliveries'),
    ]);
    const sub = node('div', '');
    return [
      facts([
        ['事件类型', ev.type],
        ['来源', ev.source],
        ['时间', time(ev.created_at)],
      ]),
      controls(
        ev.run_id && link('运行记录', 'runs', ev.run_id),
        ev.tool_call_id && link('工具原文', 'toolcall', ev.tool_call_id),
        ev.session_id && link('会话', 'session', ev.session_id),
      ),
      controls(
        button('投递事件', () => {
          const key = client.uid();
          edit(
            '投递事件',
            [
              field(
                'intent',
                '投递设置',
                { target: { agent_id: client.currentAgent() || '' }, action: 'start_run' },
                'json',
              ),
            ],
            async (v) => {
              const d = await call('events/' + eid(id) + '/deliveries', 'POST', v.intent, key);
              await open('eventDelivery', d.id);
            },
            '投递',
          );
        }),
      ),
      data('事件正文', ev.content ?? ev.result ?? ev, true),
      group(
        '投递',
        list(ds, (d) => [d.id, d.action, stateTitle(d.status), move('eventDelivery', d.id)]),
      ),
      controls(
        button('查看订阅处理', () =>
          edit(
            '查询订阅处理',
            [ident('instance', '插件实例'), ident('subscription', '订阅标识')],
            async (v) => {
              const result = await activeRead(
                'plugins/' + eid(v.instance) + '/subscriptions/' + eid(v.subscription) + '/events/' + eid(id),
              );
              sub.replaceChildren(data('订阅处理结果', result ?? '暂无处理记录', true));
            },
            '查询',
          ),
        ),
      ),
      sub,
      data('事件记录', ev),
    ];
  }
  async function deliveryView(id) {
    const d = await activeRead('event-deliveries/' + eid(id));
    return [
      facts([
        ['动作', d.action],
        ['状态', stateTitle(d.status || d.run_status)],
        ['受理时间', time(d.created_at)],
      ]),
      controls(
        d.run_id && link('查看 Run', 'runs', d.run_id),
        d.event_id && link('原始事件', 'events', d.event_id),
      ),
      data('投递记录', d, true),
    ];
  }
  async function noteReader(s, ref) {
    const key = ref.split(':').at(-1);
    const path = 'sessions/' + eid(s) + '/notes/' + eid(key);
    edit(
      '读取 Note',
      [
        field('start', '从第几行开始（可留空）', '', 'number'),
        field('end', '读到第几行（可留空）', '', 'number'),
      ],
      async (v) => {
        const q = new URLSearchParams();
        if (v.start) q.set('start_line', v.start);
        if (v.end) q.set('end_line', v.end);
        const n = await activeRead(path + '?' + q);
        byId('note-readout')?.replaceChildren(data(ref, n.body ?? n, true));
      },
      '读取',
    );
  }
  async function sessionView(id) {
    const path = 'sessions/' + eid(id);
    const [s, items, runs] = await Promise.all([
      activeRead(path),
      activeRead(path + '/context/items'),
      activeRead(path + '/runs'),
    ]);
    const search = node('form', 'desk-controls'),
      input = node('input', 'desk-search');
    input.type = 'search';
    input.placeholder = '按笔记描述检索';
    input.setAttribute('aria-label', '按笔记描述检索');
    const submit = node('button', 'desk-button', '检索');
    submit.type = 'submit';
    search.append(input, submit);
    const results = node('div', ''),
      readout = node('div', '');
    readout.id = 'note-readout';
    let stamp = 0;
    async function loadNotes(cursor) {
      const requestId = ++stamp;
      const q = new URLSearchParams({ keyword: input.value.trim() });
      if (cursor) q.set('cursor', cursor);
      const out = await activeRead(path + '/notes?' + q);
      if (requestId !== stamp) return;
      const rows = list(
        out.items || [],
        (n) => [n.description, n.note_ref, n.line_count + ' 行', () => noteReader(id, n.note_ref)],
        '没有匹配的笔记',
      );
      if (!cursor) results.replaceChildren();
      else results.querySelector('.more-notes')?.remove();
      results.append(rows);
      if (out.next_cursor)
        results.append(button('更多笔记', () => loadNotes(out.next_cursor), 'desk-button more-notes'));
    }
    search.addEventListener('submit', (e) => {
      e.preventDefault();
      loadNotes().catch((err) => notice(err.message, true));
    });
    await loadNotes();
    function branch() {
      const op = client.uid();
      edit(
        '从历史位置创建分支',
        [field('at', '包含到第几条历史记录', items.length, 'number', { required: true })],
        async (v) => {
          const b = await call(path + '/branches', 'POST', { at: Number(v.at), idempotency_key: op });
          await client.chat(b.agent_id, b.id);
        },
        '创建分支',
      );
    }
    return [
      facts([
        ['Agent', s.agent_id],
        ['会话', id],
        ['历史记录', items.length],
        ['分支来源', s.parent_session_id],
      ]),
      controls(
        button('打开对话', () => client.chat(s.agent_id, s.id), 'desk-button primary'),
        items.length && button('创建分支', branch),
        s.parent_session_id && link('父会话', 'session', s.parent_session_id),
      ),
      externalAction('预览模型上下文', () => call(path + '/context/previews', 'POST', {})),
      group('运行', runRows(runs)),
      group('会话笔记', search, results, readout),
      data('历史及事件引用', items),
    ];
  }
  function dispatchForm(d) {
    const a = client.currentAgent();
    edit(
      d ? '编辑调度' : '创建调度',
      [
        ...(!d ? [ident('id', '规则 ID')] : []),
        field('enabled', '启用规则', d?.enabled ?? false, 'checkbox'),
        field(
          'trigger',
          '触发条件',
          d?.trigger || { type: 'event', source: 'user', event_type: 'message.created' },
          'json',
          {
            hint: '事件使用 type=event 和 source；定时使用 type=schedule 和 interval_seconds 或 cron、timezone。',
          },
        ),
        field('rule', '投递规则', d?.rule || { action: 'start_run', target: { agent_id: a || '' } }, 'json'),
      ],
      async (v) => {
        const r = await call(
          d ? 'dispatches/' + eid(d.id) : 'dispatches',
          d ? 'PATCH' : 'POST',
          { ...v, ...(d ? rev(d) : {}) },
          client.uid(),
        );
        await open('dispatches', r.id, { replace: true });
      },
    );
  }
  async function dispatchView(id) {
    if (!id)
      return [
        controls(button('创建调度', () => dispatchForm(null), 'desk-button primary')),
        list(await activeRead('dispatches'), (d) => [
          d.id,
          d.trigger?.type,
          d.enabled === false ? '已停用' : '已启用',
          move('dispatches', d.id),
        ]),
      ];
    const path = 'dispatches/' + eid(id);
    const [d, evals, ds] = await Promise.all([
      activeRead(path),
      activeRead(path + '/evaluations'),
      activeRead(path + '/deliveries'),
    ]);
    const out = node('div', '');
    const preview = () =>
      edit(
        '预览规则',
        [field('event', '样例事件', { source: 'user', type: 'message.created', content: '' }, 'json')],
        async (v) => {
          out.replaceChildren(data('预览结果', await call(path + '/previews', 'POST', v), true));
        },
        '预览',
      );
    return [
      controls(
        button('编辑规则', () => dispatchForm(d)),
        button('预览匹配', preview),
      ),
      out,
      facts([
        ['状态', d.enabled === false ? '已停用' : '已启用'],
        ['触发', d.trigger?.type],
        ['下次执行', time(d.next_due)],
      ]),
      data('触发条件与规则', { trigger: d.trigger, rule: d.rule }, true),
      data('评估记录', evals),
      group(
        '投递记录',
        list(ds, (d) => [d.id, d.action, stateTitle(d.status), move('eventDelivery', d.id)]),
      ),
    ];
  }
  async function hostView() {
    const [models, docs, templates] = await Promise.all([
      activeRead('models'),
      activeRead('docs'),
      activeRead('agent-templates'),
    ]);
    const results = [
      facts([
        ['账户', identity.subject],
        ['所属用户', identity.owner],
        ['角色', identity.admin ? '管理员' : '用户'],
        ['审批权限', identity.approve ? '已授权' : '未授权'],
      ]),
      group(
        '模型',
        list(models, (m) => [m.id, m.model, m.images ? '支持图像' : '文本']),
      ),
      data('Agent 模板', templates),
    ];
    if (identity.admin)
      results.push(
        group(
          'HOME 配置',
          data('目录诊断', await activeRead('catalog/diagnostics'), true),
          externalAction('应用 HOME 配置', () => call('catalog/apply', 'POST', {})),
        ),
      );
    const ext = Object.entries(paths).flatMap(([route, ops]) => {
      if (!route.startsWith('/api/extensions/') || new URL(route, location.origin).pathname !== route)
        return [];
      return Object.keys(ops)
        .filter((method) => ['get', 'post', 'patch', 'put', 'delete'].includes(method))
        .map((method) => ({ route, method: method.toUpperCase() }));
    });
    const out = node('div', '');
    results.push(
      group(
        '插件扩展入口',
        list(ext, (x) => [
          x.route,
          x.method,
          '',
          () =>
            edit(
              '调用 ' + x.method + ' ' + x.route,
              x.method === 'GET' ? [] : [field('body', '请求参数', {}, 'json')],
              async (v) => {
                const value = await call(x.route.slice('/api/'.length), x.method, v.body);
                out.replaceChildren(data('扩展结果', value, true));
              },
              '调用',
            ),
        ]),
        out,
      ),
      group(
        '宿主参考',
        ...docs.map((doc) => externalAction(String(doc), () => activeRead('docs/' + eid(doc)))),
      ),
    );
    return results;
  }
  async function agentToolView(ref) {
    const [agent, id] = JSON.parse(ref);
    const tools = await activeRead('agents/' + eid(agent) + '/tools');
    const t = tools.find((x) => x.id === id);
    if (!t) throw Error('工具已不可用');
    return [
      facts([
        ['工具', id],
        ['Agent', agent],
        ['说明', t.description],
        ['来源', t.source],
      ]),
      data('输入定义', t.input_schema, true),
      data('实际工具记录', t),
    ];
  }
  const views = {
    agents: agentView,
    plugins: pluginsView,
    tools: toolsView,
    sets: setsView,
    connections: connectionsView,
    agentTool: agentToolView,
    runs: runsView,
    modelcall: modelView,
    toolcall: toolView,
    events: eventView,
    eventDelivery: deliveryView,
    session: sessionView,
    dispatches: dispatchView,
    host: hostView,
    reply: async (id) => [data('回复与交付状态', await activeRead('replies/' + eid(id)), true)],
  };
  return { reset, connect, open, close, imageBlock };
};
