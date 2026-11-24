(() => {
  'use strict';

  const el = id => document.getElementById(id);
  const pathId = encodeURIComponent;
  const terminal = new Set(['completed', 'cancelled', 'failed']);
  const runLabels = {
    queued: '已排队，等待 Agent 接手…', running: 'Agent 正在处理…',
    waiting: '正在等待工具或外部结果…', waiting_delivery: '正在交付回复…',
  };
  const agentPicker = el('agent-select');
  const prompt = el('prompt');
  const dialog = el('credential-dialog');
  const messages = el('messages');
  const scroll = el('conversation-scroll');
  const mobile = matchMedia('(max-width: 720px)');

  let account = null;
  let workspace = null;
  let thread = null;
  const meter = window.LoomUsage.createSessionMeter(el('session-usage'), () => {
    if (thread) refreshUsage(thread);
  });
  let login = null;
  let disconnectedDrafts = null;
  const jump = element('button', 'chat-jump', '↓ 回到最新');
  jump.id = 'jump-latest';
  jump.type = 'button';
  jump.hidden = true;
  el('status').before(jump);
  const stop = element('button', 'chat-stop', '停止');
  stop.id = 'stop-run';
  stop.type = 'button';
  stop.title = '停止这次运行';
  stop.hidden = true;
  el('send').before(stop);
  const desk = window.createLoomDesk({
    uid,
    request: (path, options) => request(account, path, options),
    signIn: showCredentials,
    closeDrawer: () => drawer(false),
    currentAgent: () => workspace?.agent.id,
    currentSession: () => thread?.session.id,
    hasCreator: () => !!creatorAgent(),
    draftWithAgent: draftWithAgent,
    async draftWithCreator(text) {
      const creator = creatorAgent();
      if (!creator) throw new Error('当前账户没有可用的创造者 Agent');
      return draftWithAgent(creator.id,text);
    },
    async reloadAgents() {
      const who=account;
      const agents=await request(who,'agents');
      if(account!==who)return;
      account.agents=agents;
      renderAgents();
      if(workspace){agentPicker.value=workspace.agent.id;workspace.agent=agents.find(a=>a.id===workspace.agent.id)||workspace.agent;loadHints(workspace);}
    },
    async chat(agentId, sessionId) {
      desk.close();
      await openAgent(agentId,sessionId);
      prompt.focus();
    },
  });
  window.createLoomShell?.({
    listAgents: () => account?.agents || [],
    listHints: () => {
      const w=workspace;
      return w?.hints.map(hint=>({label:hint[1],action:()=>useHint(w,hint)})) || [];
    },
    openAgent: id => { if (account) {desk.close(); drawer(false); return openAgent(id);} showCredentials(); },
    listTools: async () => {
      const w=workspace;
      if (!w) return [];
      const rows=await request(account,'agents/'+pathId(w.agent.id)+'/tools',{signal:w.controller.signal});
      return workspace===w ? rows : [];
    },
    draft: text => { desk.close(); drawer(false); return putDraft(text); },
    newChat: () => { desk.close(); drawer(false); return createSession(); },
    openDesk: section => {
      if (section === 'oi') return openOiLink();
      if (account) desk.open(section);
      else showCredentials();
    },
  });

  function creatorAgent() {
    const matches = agent => agent?.template?.id === 'creator' && agent.template.source === 'loom'
      && agent.owner === account?.identity.owner;
    return matches(workspace?.agent) ? workspace.agent : account?.agents.find(matches);
  }

  async function draftWithAgent(id,text) {
    if (!account?.agents.some(agent=>agent.id===id)) throw new Error('此 Agent 不可用');
    desk.close();
    drawer(false);
    const w=await openAgent(id,null,true);
    if (w && workspace===w) putDraft(text);
  }

  function putDraft(text) {
    if (!account) { showCredentials(); return; }
    if (!workspace?.listed || workspace.creating || (thread && !thread.loaded)) return;
    prompt.value = prompt.value.trim() ? prompt.value.trimEnd() + '\n' + text : text;
    prompt.dispatchEvent(new Event('input'));
    prompt.focus();
  }

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function uid() {
    return Array.from(crypto.getRandomValues(new Uint8Array(16)), n => n.toString(16).padStart(2, '0')).join('');
  }

  function setStatus(text = '', kind = '') {
    if (el('status').textContent !== text) el('status').textContent = text;
    el('status').dataset.kind = kind;
  }

  function extras(create=false) {
    if (!account || !workspace) return;
    const key=thread?'session:'+thread.session.id:'agent:'+workspace.agent.id;
    if (create && !account.extras.has(key)) account.extras.set(key,{});
    return account.extras.get(key);
  }
  function renderAttachment(options){
    const chips=el('compose-chips');
    if(chips.dataset.item===(options?.attachment?.id||''))return;
    chips.dataset.item=options?.attachment?.id||'';
    chips.replaceChildren();
    if(!options?.attachment)return;
    const remove=element('button','desk-button',options.attachment.name+' ×');
    remove.type='button';remove.title='移除附件';
    remove.addEventListener('click',()=>{
      if(remove.disabled||!extras())return;
      delete options.attachment;updateControls();
    });
    chips.append(remove);
  }

  function credentialsError(text = '') {
    el('credential-error').textContent = text;
    el('credential-error').hidden = !text;
  }

  function showCredentials() {
    credentialsError();
    if (!dialog.open) dialog.showModal();
    el('token-input').focus();
  }

  async function request(identity, path, {body, method, key, signal = identity.controller.signal, stream = false} = {}) {
    const headers = {Authorization: 'Bearer ' + identity.token};
    if (body) headers['Content-Type'] = 'application/json';
    if (key) headers['Idempotency-Key'] = key;
    if (stream) headers.Accept = 'text/event-stream';
    const response = await fetch('/api/' + path, {
      method: method || (body ? 'POST' : 'GET'), mode: 'same-origin', credentials: 'omit',
      cache: 'no-store', redirect: 'error', signal, headers,
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!response.ok) {
      const detail = await response.json().catch(() => ({}));
      const error = new Error(response.status === 401 ? '凭据无效，请重新连接。' :
        'HTTP ' + response.status + (detail.error ? ' · ' + detail.error : ''));
      error.status = response.status;
      if (response.status === 401 && identity === account) {
        disconnect(true);
        showCredentials();
        credentialsError(error.message);
      }
      throw error;
    }
    return stream ? response : response.json();
  }

  function drawer(open) {
    const wasOpen = document.body.classList.contains('sidebar-open');
    open = open && mobile.matches;
    document.body.classList.toggle('sidebar-open', open);
    el('sidebar-toggle').setAttribute('aria-expanded', String(open));
    document.querySelector('main').inert = open;
    el('sidebar').inert = mobile.matches && !open;
    if (open) el('sidebar-close').focus();
    else if (wasOpen && mobile.matches) el('sidebar-toggle').focus();
  }

  function stopThread() {
    if (thread) {
      if (thread.loaded && scroll.clientHeight) {
        account.reading.set(thread.session.id, {top: scroll.scrollTop, following: thread.following});
      }
      clearTimeout(thread.timer);
      thread.controller.abort();
      thread.feed?.abort();
      cancelAnimationFrame(thread.paint);
    }
    thread = null;
    meter.clear();
    prompt.value = '';
    messages.replaceChildren();
    el('empty-state').hidden = false;
    el('thread-title').textContent = '新会话';
    jump.hidden = true;
    setStatus();
  }

  function stopWorkspace() {
    stopThread();
    workspace?.controller.abort();
    workspace = null;
    el('session-list').replaceChildren();
    el('capability-prompts').replaceChildren();
    el('capability-prompts').hidden = true;
  }

  function disconnect(keepDrafts = false) {
    desk.reset();
    stopWorkspace();
    account?.controller.abort();
    if (keepDrafts && account) {
      const {identity, drafts, emptyDrafts, extras, reading, titles, lastSessions, pending} = account;
      disconnectedDrafts = {identity, drafts, emptyDrafts, extras, reading, titles, lastSessions, pending};
    } else if (!keepDrafts) disconnectedDrafts = null;
    account = null;
    agentPicker.replaceChildren(new Option('请选择 Agent', ''));
    history.replaceState(null, '', location.pathname + location.search);
    el('token-input').value = '';
    updateControls();
  }

  function updateControls() {
    const pending = account?.pending.get(thread?.session.id);
    const ready = !workspace?.creating && (thread ? thread.loaded && !pending?.busy : workspace?.listed);
    el('session-details').hidden=!thread?.loaded;
    el('attach-image').disabled=!ready || !!thread?.active;
    el('goal-toggle').disabled=!ready || !!thread?.active;
    const options = extras();
    el('goal-toggle').setAttribute('aria-pressed',String(!!options?.goal));
    el('suggest-next').hidden=!thread?.reply || !ready || !!thread?.active || !!prompt.value.trim();
    el('suggested-draft').hidden=!thread?.suggestion || !ready || !!prompt.value.trim();
    el('suggested-draft').textContent=thread?.suggestion||'';
    renderAttachment(options);
    agentPicker.disabled = !account?.agents.length;
    el('new-session').disabled = !!account && (!workspace?.listed || workspace.creating);
    el('create-agent').hidden = !account || !!account.agents.length;
    el('create-agent').disabled = !!account?.creating;
    el('logout').hidden = !account;
    el('connection-state').textContent = account ? account.identity.owner : '连接账户';
    prompt.disabled = !ready;
    const retry = pending?.error && pending.text === prompt.value.trim();
    const active = thread?.active;
    el('send').disabled = !ready || !!options?.uploading || !prompt.value.trim() || (active?.status === 'waiting_delivery' && !retry);
    stop.hidden = !active || active.status === 'waiting_delivery';
    stop.disabled = !!thread?.stopping || !!pending?.busy;
    prompt.placeholder = thread?.question ? '回答 Agent 的问题…' : active && active.status !== 'waiting_delivery' ? '补充说明，发给这次运行…' : '发消息，/ 选操作，@ 选工具';
    const sendLabel = thread?.question ? '发送回答' : active && active.status !== 'waiting_delivery' ? '补充输入' : '发送消息';
    el('send').title = sendLabel;
    el('send').setAttribute('aria-label', sendLabel);
    el('capability-prompts').hidden = !ready || !workspace?.hints.length || !!thread?.historyLength || !!thread?.active;
    const action = el('empty-action');
    action.hidden = !!thread || (!!workspace && !workspace.listFailed);
    action.textContent = !account ? '连接账户' : !workspace ? '创建标准 Agent' :
      workspace.listFailed ? '重新读取会话' : '新建会话';
    action.disabled = !!account?.creating || !!workspace?.creating;
    if (!thread) {
      el('empty-title').textContent = '想一起处理什么？';
      el('empty-description').textContent = !account ? '连接账户，选择你的 Agent。' :
        !workspace ? '创建一个标准 Agent，开始第一段对话。' : '';
    } else {
      el('empty-title').textContent = '想一起处理什么？';
      el('empty-description').textContent = '';
    }
  }

  function renderAgents() {
    agentPicker.replaceChildren(...account.agents.map(a => new Option(
      a.name && !['standard', 'creator', a.id].includes(a.name) ? a.name + ' · ' + a.id : a.id, a.id
    )));
    if (!account.agents.length) agentPicker.add(new Option('暂无 Agent', ''));
    updateControls();
  }

  function sessionTitle(s) {
    return s.title || s.first_user_preview || account.titles.get(s.id);
  }

  function titleFor(s) {
    const title = sessionTitle(s);
    if (title) return title;
    const date = new Date(s.created_at * 1000);
    const stamp = Number.isFinite(date.getTime()) ? date.toLocaleString('zh-CN', {
      month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false,
    }) + ' · ' : '';
    return stamp + s.id.slice(-6);
  }

  function renderSessions() {
    el('session-list').replaceChildren(...workspace.sessions.map(s => {
      const item = document.createElement('li');
      const button = element('button', 'session-button', titleFor(s));
      button.type = 'button';
      button.title = titleFor(s);
      if (s.id === thread?.session.id) button.setAttribute('aria-current', 'true');
      button.addEventListener('click', () => { desk.close(); drawer(false); openThread(s); });
      item.append(button);
      return item;
    }));
  }

  async function useHint(w, [id, label, text]) {
    if (prompt.disabled || workspace!==w) return;
    const before=thread, latest=await loadHints(w);
    if (workspace!==w || thread!==before) return;
    if (latest?.some(choice=>choice[0]===id && choice[1]===label && choice[2]===text)) putDraft(text);
    else setStatus('可用工具已更新，请重新选择。');
  }

  async function loadHints(w) {
    const check = ++w.hintCheck;
    try {
      const directory = await request(account, 'agents/' + pathId(w.agent.id) + '/tools', {signal: w.controller.signal});
      if (w !== workspace || w.hintCheck !== check) return;
      const tools = new Set(directory.filter(t => t.source === 'builtin').map(t => t.id));
      const choices = [
        ['sessions.history', '回顾会话', '请回顾这段会话，总结我们已做的决定和待办事项。'],
        ['search_notes', '查找笔记', '请查找这段会话中已有的笔记，列出它们记录了什么。'],
        ['harness.inspect', '查看宿主配置', '请查看当前宿主的版本，以及我可以使用哪些 Agent 模板。'],
        ['agents.update', '调整 Agent 配置', '我想调整你的配置。请先说明你现在能修改哪些设置，并问我想改什么。'],
      ];
      const plugin = w.agent.grants?.plugins?.[0];
      if (plugin) choices.push(['plugins.get', '查看授权插件', '请查看授权插件 ' + plugin + ' 的配置和启用状态。']);
      const contributed = [];
      for (const tool of directory) {
        const suggestion = tool.webui?.suggestion;
        if (typeof suggestion?.label === 'string' && typeof suggestion?.prompt === 'string') {
          const label = suggestion.label.trim(), text = suggestion.prompt.trim();
          if (label && text && label.length <= 64 && text.length <= 1200) contributed.push([tool.id, label, text]);
        }
        if (tool.source === 'oi' && tool.binding?.source?.app === 'loom-oi' && tool.binding.source.action === 'list_hosts') {
          contributed.push([tool.id, '查看在线主机', '请用 ' + tool.binding.member + ' 查看我能访问的主机和在线状态。']);
        }
        if (tool.source === 'oi' && tool.binding?.source?.app === 'skills' && tool.binding.source.action === 'list_skills') {
          contributed.push([tool.id, '浏览主机 Skill', '请用 ' + tool.binding.member + ' 列出目标主机上可用的 Skill，并用一句话介绍每项用途。']);
        }
      }
      const seen = new Set();
      w.hints = [...contributed, ...choices.filter(([id]) => tools.has(id))].filter(([,label,text]) => {
        if (seen.has(label) || seen.has(text)) return false;
        seen.add(label); seen.add(text);
        return true;
      });
      el('capability-prompts').replaceChildren(...w.hints.slice(0,6).map(hint => {
        const label=hint[1];
        const button = element('button', 'capability-prompt', label);
        button.type = 'button';
        button.setAttribute('aria-label',label);
        button.addEventListener('click', async () => {
          if (prompt.disabled || button.disabled) return;
          button.disabled = true;
          await useHint(w,hint);
        });
        return button;
      }));
      updateControls();
      return w.hints;
    } catch {
      if (w === workspace && w.hintCheck === check) {
        w.hints = [];
        el('capability-prompts').replaceChildren();
        updateControls();
      }
    }
  }

  async function openAgent(agentId, preferredSession, blank = false) {
    stopWorkspace();
    const agent = account.agents.find(a => a.id === agentId);
    if (!agent) { updateControls(); return; }
    const w = {agent, sessions: [], hints: [], hintCheck: 0, controller: new AbortController()};
    workspace = w;
    agentPicker.value = agent.id;
    prompt.value = account.emptyDrafts.get(agent.id) || '';
    updateControls();
    loadHints(w);
    try {
      const sessions = await request(account, 'sessions?agent_id=' + pathId(agent.id) + '&include=preview', {signal: w.controller.signal});
      if (w !== workspace) return;
      w.sessions = sessions.reverse();
      w.listed = true;
      renderSessions();
      const selected = preferredSession || account.lastSessions.get(agent.id);
      const initial = !blank && (w.sessions.find(s => s.id === selected) || w.sessions[0]);
      if (initial) await openThread(initial);
      else rememberChatLocation();
      return w;
    } catch (e) {
      if (w === workspace) {
        w.listFailed = true;
        setStatus('无法读取会话。' + e.message, 'error');
      }
    } finally {
      if (w === workspace) updateControls();
    }
  }

  async function createAgent() {
    const identity = account;
    if (!identity || identity.creating) return;
    identity.creating = true;
    updateControls();
    try {
      const id = identity.newAgentId ||= 'assistant-' + uid().slice(0, 12);
      let agent;
      try {
        agent = await request(identity, 'agents', {body: {
          id, name: '工作助手', template: {id: 'standard', source: 'loom'},
        }});
      } catch (e) {
        if (e.status !== 409) throw e;
        agent = await request(identity, 'agents/' + pathId(id));
      }
      if (identity !== account) return;
      identity.agents.push(agent);
      renderAgents();
      await openAgent(agent.id);
    } catch (e) {
      if (identity === account) setStatus('创建 Agent 失败。' + e.message, 'error');
    } finally {
      identity.creating = false;
      if (identity === account) updateControls();
    }
  }

  async function createSession(sendFirstMessage = false) {
    if (!account) { drawer(false); showCredentials(); return; }
    const w = workspace;
    if (!w?.listed || w.creating) return;
    const identity = account;
    const previousThread = thread;
    const draft = previousThread ? '' : prompt.value;
    const id = w.newSessionId ||= 'ses_' + uid();
    w.creating = true;
    updateControls();
    try {
      let session;
      try {
        session = await request(identity, 'sessions', {body: {id, agent_id: w.agent.id}});
      } catch (e) {
        if (e.status !== 409) throw e;
        session = await request(identity, 'sessions/' + pathId(id));
        if (session.agent_id !== w.agent.id) throw e;
      }
      if (w !== workspace) return;
      w.newSessionId = null;
      w.sessions = [session, ...w.sessions.filter(s => s.id !== session.id)];
      if (previousThread !== thread) { renderSessions(); return; }
      if (draft) {
        identity.drafts.set(session.id, draft);
        identity.emptyDrafts.delete(w.agent.id);
      }
      const staged = identity.extras.get('agent:' + w.agent.id);
      if(!previousThread && staged){
        identity.extras.set('session:'+session.id,staged);
        identity.extras.delete('agent:'+w.agent.id);
      }
      drawer(false);
      await openThread(session);
      if (thread?.session.id === session.id) {
        w.creating = false;
        updateControls();
        if (sendFirstMessage && thread.loaded) await send();
        else prompt.focus();
      }
    } catch (e) {
      if (w === workspace) setStatus('创建会话失败。' + e.message, 'error');
    } finally {
      w.creating = false;
      if (w === workspace) updateControls();
    }
  }

  function openThread(session) {
    stopThread();
    const reading = account.reading.get(session.id);
    const t = {session, controller: new AbortController(), historyLength: 0, failures: 0, writes: 0,
      runAnchors: new Map(), runBadges: new Map(),
      following: reading?.following ?? true, restoreTop: reading?.top, work: null};
    thread = t;
    meter.start();
    account.lastSessions.set(workspace.agent.id,session.id);
    rememberChatLocation();
    el('thread-title').textContent = sessionTitle(session) || '新会话';
    prompt.value = account.drafts.get(session.id) || '';
    renderSessions();
    updateControls();
    setStatus('正在读取对话…');
    return refresh(t);
  }

  function message(role, label, content) {
    const row = element('article', 'message message-' + role);
    const body = element('div', 'message-content');
    if (role === 'assistant' && content && window.LoomText) window.LoomText.render(body, content);
    else body.textContent = content;
    row.append(element('div', 'message-label', label), body);
    return row;
  }

  function oiRequested() {
    return new URLSearchParams(location.hash.slice(1)).get('view') === 'oi';
  }

  function rememberChatLocation() {
    if (!el('desk-panel').hidden && oiRequested()) return;
    if (!workspace) {
      history.replaceState(null, '', location.pathname + location.search);
      return;
    }
    const params = new URLSearchParams({agent: workspace.agent.id});
    if (thread) params.set('session', thread.session.id);
    history.replaceState(null, '', '#' + params);
  }

  function openOiLink() {
    if (!account) { showCredentials(); return; }
    if (!account.identity.admin) {
      rememberChatLocation();
      setStatus('OI 接入设置需要管理员身份。', 'error');
      return;
    }
    history.replaceState(null, '', '#view=oi');
    desk.open('oi');
  }

  window.addEventListener('hashchange', () => { if (oiRequested()) openOiLink(); });
  new MutationObserver(() => {
    if (el('desk-panel').hidden && oiRequested() && account) rememberChatLocation();
  }).observe(el('desk-panel'), {attributes:true, attributeFilter:['hidden']});

  async function copyReply(text, button) {
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(text);
    } catch {
      const input = element('textarea', 'visually-hidden');
      input.value = text;
      button.after(input);
      try {
        input.select();
        if (!document.execCommand('copy')) throw new Error('复制失败');
      } finally {
        input.remove();
        button.focus({preventScroll:true});
      }
    }
  }

  function replyActions(t, item, text, cursor) {
    const bar = element('div','reply-actions');
    const copy = element('button','reply-action','复制');
    copy.type='button';
    copy.title='复制回复原文';
    copy.addEventListener('click',async () => {
      try {
        await copyReply(text,copy);
        copy.textContent='已复制';
      } catch {
        copy.textContent='复制失败';
      }
      setTimeout(()=>{copy.textContent='复制';},1500);
    });
    bar.append(copy);
    if (item.run_id) {
      const inspect = element('button','reply-action','查看本轮');
      inspect.type='button';
      inspect.addEventListener('click',()=>desk.open('runs',item.run_id));
      bar.append(inspect);
    }
    const branch = element('button','reply-action','从此处分支');
    branch.type='button';
    branch.title='保留截至这条回复的历史，另开会话';
    let key;
    branch.addEventListener('click',async () => {
      if (thread !== t || branch.disabled) return;
      key ||= uid();
      branch.disabled = true;
      try {
        const session = await request(account,'sessions/'+pathId(t.session.id)+'/branches',{
          body:{at:cursor, idempotency_key:key, title:(sessionTitle(t.session) || '会话')+' · 分支'},
        });
        if (thread !== t) return;
        workspace.sessions = [session,...workspace.sessions.filter(s=>s.id!==session.id)];
        await openThread(session);
        if (thread?.session.id === session.id) prompt.focus();
      } catch(e) {
        if (thread === t) setStatus(e.message,'error');
      } finally {
        branch.disabled=false;
      }
    });
    bar.append(branch);
    return bar;
  }

  function nearBottom() {
    return scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 48;
  }

  function settleViewport(t) {
    if (thread !== t || !scroll.clientHeight) return;
    if (t.following) scroll.scrollTop = scroll.scrollHeight;
    else if (t.restoreTop != null) scroll.scrollTop = t.restoreTop;
    t.restoreTop = null;
    jump.hidden = nearBottom();
  }
  scroll.addEventListener('scroll', () => {
    if (!thread?.loaded || !scroll.clientHeight) return;
    thread.following = nearBottom();
    jump.hidden = thread.following;
  }, {passive:true});
  jump.addEventListener('click', () => {
    if (!thread) return;
    thread.following = true;
    settleViewport(thread);
  });

  function workGroup(runId) {
    const node = element('details', 'work-trace');
    const head = element('summary', 'work-heading');
    const title = element('span','work-title');
    const names = element('span','work-names');
    head.append(title,names);
    const list = element('div','work-steps');
    node.append(head,list);
    if (runId) {
      const inspect = element('button', 'work-inspect', '查看此次运行');
      inspect.type='button';
      inspect.addEventListener('click',()=>desk.open('runs',runId));
      node.append(inspect);
    }
    return {node, title, names, list, runId, entries: new Map()};
  }

  function recordWork(t, fragment, item, text) {
    if (!t.work || t.work.runId !== item.run_id) {
      t.work = workGroup(item.run_id);
      fragment.append(t.work.node);
    }
    const group = t.work;
    const key = item.tool_call_id || item.event_id || String(group.entries.size);
    const name = item.tool_id?.split('.').at(-1) || '工具';
    group.entries.set(key,name);
    group.title.textContent = '调用了 ' + group.entries.size + ' 个工具';
    group.names.textContent = [...new Set(group.entries.values())].join(' · ');
    group.list.append(toolResult(t,item,text));
    if (item.status === 'failed') group.node.dataset.failed = 'true';
  }

  function toolResult(t, item, text) {
    const details = element('details', 'work-step');
    const summary = element('summary', 'message-label');
    const args = element('div', 'message-content');
    const status = {completed: '已完成', failed: '失败', cancelled: '已取消', accepted: '已受理'};
    const describe = (name, state) => {
      summary.textContent = (name || '工具调用 ' + (item.tool_call_id || '').slice(-6))
        + (state ? ' · ' + (status[state] || state) : '');
    };
    describe(item.tool_id, item.status);
    details.append(summary,args);
    if (item.tool_call_id) {
      const inspect = element('button', 'desk-button', '查看工具调用');
      inspect.type = 'button';
      inspect.addEventListener('click', () => desk.open('toolcall', item.tool_call_id));
      details.append(inspect);
    }
    const blocks=item.content?.content||[];
    const images=Array.isArray(blocks)?blocks.map(desk.imageBlock).filter(Boolean):[];
    if (images.length) details.append(...images);
    const payload = item.content?.structuredContent;
    const parts = Array.isArray(blocks) ? blocks.filter(p => p?.type==='text' && typeof p.text==='string') : [];
    if (!images.length || parts.length || typeof payload?.text==='string') {
      const result = element('pre', 'work-result');
      details.append(result);
      details.addEventListener('toggle', () => {
        if (!details.open || result.hasChildNodes()) return;
        if (typeof payload?.text==='string') result.textContent=payload.text;
        else if (payload!=null) result.textContent=JSON.stringify(payload,null,2);
        else if (parts.length) result.textContent=parts.map(p => p.text).join('\n');
        else result.textContent=text;
      });
    }
    if (images.length || parts.length || payload!=null) {
      const raw=element('details','desk-data');
      raw.append(element('summary','','查看原始结果'));
      raw.addEventListener('toggle',()=>{
        if(raw.open && raw.childElementCount===1) raw.append(element('pre','',JSON.stringify(item.content,null,2)));
      });
      details.append(raw);
    }
    let loading = false;
    details.addEventListener('toggle', async () => {
      if (!details.open || loading || !item.tool_call_id || t !== thread) return;
      loading = true;
      args.textContent = '正在读取调用详情…';
      try {
        const call = await request(account, 'tool-calls/' + pathId(item.tool_call_id), {signal: t.controller.signal});
        if (t !== thread) return;
        describe(call.tool_id, call.status);
        args.textContent = '调用参数\n' + JSON.stringify(call.arguments ?? {}, null, 2);
      } catch (e) {
        if (t === thread) args.textContent = '无法读取调用详情。' + e.message;
        loading = false;
      }
    });
    return details;
  }

  function showHistory(t, items) {
    const fragment = document.createDocumentFragment();
    for (let index=t.historyLength; index<items.length; index++) {
      const item=items[index];
      const binary=item.role==='tool' && Array.isArray(item.content?.content) && item.content.content.some(p=>p.type==='image');
      const text = typeof item.content === 'string' ? item.content : binary?'含图像结果':JSON.stringify(item.content ?? '');
      if (item.role === 'tool') {
        recordWork(t,fragment,item,text);
      } else {
        t.work = null;
        const role = item.role === 'user' ? 'user' : item.role === 'assistant' ? 'assistant' : 'system';
        const names = {user: '你', assistant: workspace.agent.name || workspace.agent.id, system: '系统'};
        if(Array.isArray(item.content) && item.content.some(p=>p.type==='attachment')){
          const row=message(role,names[role],item.content.filter(p=>p.type==='text').map(p=>p.text).join('\n'));
          if (role==='user' && item.run_id) t.runAnchors.set(item.run_id,row);
          for(const part of item.content.filter(p=>p.type==='attachment')){
            const load=element('button','desk-button','查看图片'); load.type='button';
            load.addEventListener('click',async()=>{
              load.disabled=true;
              try{
                const image=await request(account,'attachments/'+pathId(part.attachment_id)+'/content',{signal:t.controller.signal});
                if(thread!==t)return;
                if(!/^data:image\/(png|jpeg|webp|gif);base64,/.test(image.image_url?.url||''))throw Error('附件格式不支持');
                const img=element('img','desk-inline-image');img.alt='会话附件';img.src=image.image_url.url;
                load.replaceWith(img);
              }catch(e){if(thread===t){setStatus(e.message,'error');load.disabled=false;}}
            });
            row.append(load);
          }
          fragment.append(row);
        }else {
          const row=message(role,names[role],text);
          if (item.run_id && (role==='user' || (role==='assistant' && item.status!=='progress'))) t.runAnchors.set(item.run_id,row);
          if (role==='assistant' && (!item.status || item.status==='completed')) {
            row.append(replyActions(t,item,text,index+1));
          }
          fragment.append(row);
        }
      }
    }
    if (fragment.childNodes.length) messages.insertBefore(fragment, t.activity || t.preview || null);
    t.historyLength = items.length;
    if (!sessionTitle(t.session)) {
      const first = items.find(i => i.role === 'user');
      const title = typeof first?.content === 'string' ? first.content :
        Array.isArray(first?.content) ? first.content.find(part=>part.type==='text')?.text : '';
      if (title?.trim()) {
        const preview = Array.from(title.trim().replace(/\s+/g, ' ')).slice(0, 48).join('');
        account.titles.set(t.session.id,preview);
        el('thread-title').textContent = preview;
        renderSessions();
      }
    }
    el('empty-state').hidden = !!messages.childElementCount;
    settleViewport(t);
  }

  function clearPreview(t) {
    cancelAnimationFrame(t.paint);
    t.paint = 0;
    t.preview?.remove();
    t.preview = null;
    t.previewText = '';
    t.paintedText = '';
  }

  function schedulePreview(t) {
    if (!t.preview || t.paint || t.paintedText === t.previewText) return;
    t.paint = requestAnimationFrame(() => {
      t.paint = 0;
      if (thread !== t || !t.preview) return;
      const selected=document.getSelection();
      if (selected?.rangeCount && !selected.isCollapsed && selected.getRangeAt(0).intersectsNode(t.preview)) return;
      const body=t.preview.lastChild;
      if (window.LoomText) window.LoomText.render(body,t.previewText);
      else body.textContent=t.previewText;
      t.paintedText=t.previewText;
      settleViewport(t);
    });
  }
  document.addEventListener('selectionchange',()=>{if (thread) schedulePreview(thread);});

  function updateActivity(t, calls) {
    const pending = calls.filter(c => !terminal.has(c.status));
    const signature = JSON.stringify([t.active?.id,t.active?.status,t.question?.question,...pending.map(c=>[c.id,c.status])]);
    if (signature === t.activityKey) return;
    t.activityKey = signature;
    t.activity?.remove();
    t.activity = null;
    if (!t.active) return;
    const block = element('section','work-live');
    const line = element('span','work-live-line');
    const dot = element('span','work-signal');
    dot.setAttribute('aria-hidden','true');
    const current = pending.at(-1);
    const description = t.question ? '等待你的回答'
      : current?.status === 'awaiting_approval' ? '等待审批'
      : current ? '正在调用 ' + current.tool_id
      : t.active.status === 'running' ? '正在处理' : (runLabels[t.active.status] || t.active.status).replace(/…$/, '');
    line.append(dot,element('span','',description));
    block.append(line);
    function action(label, callback) {
      const button = element('button','work-inspect',label);
      button.type='button';
      button.addEventListener('click',callback);
      block.append(button);
    }
    if (t.question) {
      block.append(element('p','work-question',t.question.question || t.question.arguments?.question || '请补充信息。'));
      action('回答问题',()=>prompt.focus());
    } else if (pending.length) {
      for (const call of pending.filter(c=>c.status==='awaiting_approval')) {
        action('查看审批 · '+call.tool_id,()=>desk.open('toolcall',call.id));
      }
    }
    if (current?.status !== 'awaiting_approval' && !t.question) action('查看运行',()=>desk.open('runs',t.active.id));
    t.activity = block;
    messages.insertBefore(block,t.preview || null);
  }

  async function readStream(t, run, controller) {
    const after = t.streamCursor ? '?after='+pathId(t.streamCursor) : '';
    const response = await request(account, 'runs/' + pathId(run.id) + '/stream'+after, {stream: true, signal: controller.signal});
    if (!response.body) return;
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    while (true) {
      const {done, value} = await reader.read();
      if (done || t !== thread || controller.signal.aborted) return;
      buffer += decoder.decode(value, {stream: true});
      let boundary;
      while ((boundary = /\r?\n\r?\n/.exec(buffer))) {
        const block = buffer.slice(0, boundary.index);
        buffer = buffer.slice(boundary.index + boundary[0].length);
        const type = /^event: *([^\r\n]+)/m.exec(block)?.[1];
        const data = /^data: *(.*)$/m.exec(block)?.[1];
        if (!type || !data) continue;
        const event = JSON.parse(data);
        const cursor = /^id: *([^\r\n]+)/m.exec(block)?.[1];
        if (cursor) t.streamCursor = cursor;
        if (type === 'run_end') { refresh(t); return; }
        if (type === 'attempt_started') {
          t.streamAttempt = event.attempt_id;
          clearPreview(t);
        } else if (event.attempt_id === t.streamAttempt && type === 'attempt_discarded') {
          clearPreview(t);
        } else if (event.attempt_id === t.streamAttempt && type === 'text_delta') {
          if (!t.preview) {
            t.preview = message('assistant', '正在回复', '');
            messages.append(t.preview);
            el('empty-state').hidden = true;
          }
          t.previewText += event.text;
          schedulePreview(t);
        }
      }
    }
  }

  function watchRun(t, run) {
    if (t.feedId !== run?.id) {
      t.feed?.abort();
      t.feed = null;
      clearPreview(t);
      t.feedId = run?.id;
      t.streamAttempt = null;
      t.streamCursor = null;
    }
    if (run && !t.feed && t.streamCursor !== 'end') {
      const controller = new AbortController();
      t.feed = controller;
      readStream(t, run, controller).catch(() => {}).finally(() => {
        controller.abort();
        if (t.feed === controller) t.feed = null;
      });
    }
  }

  function displayRunUsage(t, runs) {
    for (const run of runs) {
      const parent=t.active?.id===run.run_id && t.activity ? t.activity : t.runAnchors.get(run.run_id);
      if (!parent) continue;
      let badge=t.runBadges.get(run.run_id);
      if(!badge){badge=document.createElement('span');t.runBadges.set(run.run_id,badge);}
      window.LoomUsage.renderRunBadge(badge,run);
      const bar=parent.querySelector('.reply-actions') || parent;
      if (badge.parentNode!==bar) bar.append(badge);
    }
  }

  async function refreshUsage(t) {
    if(t!==thread) return;
    if(t.usageLoading){t.usageAgain=true;return;}
    t.usageLoading=true;
    try {
      const summary=await request(account,'sessions/'+pathId(t.session.id)+'/usage',{signal:t.controller.signal});
      if(t!==thread || summary.session_id!==t.session.id)return;
      meter.update(summary);
      displayRunUsage(t,summary.runs);
      settleViewport(t);
    } catch {
      if(t===thread) meter.failed();
    } finally {
      t.usageLoading=false;
      if(t===thread && t.usageAgain){t.usageAgain=false;refreshUsage(t);}
    }
  }

  async function refresh(t) {
    if (t !== thread) return;
    if (t.refreshing) { t.again = true; return; }
    t.refreshing = true;
    clearTimeout(t.timer);
    const identity = account;
    const path = 'sessions/' + pathId(t.session.id);
    const options = {signal: t.controller.signal};
    const writes = t.writes;
    try {
      // Read committed history after the run snapshot, including its final answer.
      const runs = await request(identity, path + '/runs', options);
      const items = await request(identity, path + '/context/items', options);
      if (thread !== t || writes !== t.writes) return;
      const active = runs.filter(r => !terminal.has(r.status)).at(-1);
      const calls = active ? await request(identity, 'runs/' + pathId(active.id) + '/tool-calls', options) : [];
      if (thread !== t || writes !== t.writes) return;
      t.active = active;
      t.question = calls.find(c => c.tool_id === 'ask_user' && c.status === 'awaiting_input');
      watchRun(t, active);
      updateActivity(t, calls);
      showHistory(t, items);
      refreshUsage(t);
      const final = items.at(-1);
      const reply = final?.role==='assistant' ? final.run_id : null;
      if(reply!==t.reply){t.reply=reply;t.suggestion='';}
      t.loaded = true;
      t.failures = 0;
      const pending = identity.pending.get(t.session.id);
      const last = runs.at(-1);
      if (last && terminal.has(last.status) && t.finishedRun !== last.id) {
        t.finishedRun = last.id;
        loadHints(workspace);
      }
      if (pending?.error) setStatus(pending.error, 'error');
      else if (pending?.busy) setStatus('正在发送…');
      else if (active) setStatus();
      else if (last?.status === 'failed') setStatus('运行失败。' + (last.error || last.stop_reason || ''), 'error');
      else if (last?.status === 'cancelled') setStatus('运行已取消。');
      else setStatus();
      if (active) t.timer = setTimeout(() => refresh(t), active.status === 'waiting' ? 4000 : 2200);
    } catch (e) {
      if (thread !== t) return;
      t.failures++;
      meter.failed();
      setStatus(t.failures < 4 ? '读取会话失败，正在重试。' + e.message : '读取失败，请重新选择这个会话。' + e.message, 'error');
      if (t.failures < 4) t.timer = setTimeout(() => refresh(t), 1200 * t.failures);
    } finally {
      t.refreshing = false;
      if (thread === t) {
        updateControls();
        if (t.again) { t.again = false; refresh(t); }
      }
    }
  }

  async function send() {
    if (el('send').disabled) return;
    if (!thread) { await createSession(true); return; }
    const identity = account;
    const t = thread;
    const sid = t.session.id;
    const text = prompt.value.trim();
    const options = extras();
    const imageId = options?.attachment?.id || null;
    const objective = options?.goal && !t.active ? text : null;
    let submission = identity.pending.get(sid);
    if (!submission || submission.text !== text || submission.imageId!==imageId || submission.objective!==objective) {
      const action = t.active ? {action: 'add_input', run_id: t.active.id} : {action: 'start_run'};
      if (t.question) action.question_id = t.question.id;
      if(objective)action.goal={objective};
      submission = {text, imageId, objective, key: uid(), delivery: {target: {session_id: sid}, ...action}};
      identity.pending.set(sid, submission);
    }
    submission.busy = true;
    t.following = true;
    submission.error = '';
    updateControls();
    setStatus('正在发送…');
    try {
      if (!submission.eventId) {
        const content=imageId?[{type:'text',text},{type:'attachment',attachment_id:imageId}]:text;
        const event = await request(identity, 'events', {body: {content}, key: 'event-' + submission.key});
        submission.eventId = event.id;
      }
      const delivered = await request(identity, 'events/' + pathId(submission.eventId) + '/deliveries', {body: submission.delivery, key: 'delivery-' + submission.key});
      identity.pending.delete(sid);
      if (terminal.has(delivered.run_status)) {
        if (identity === account && thread?.session.id === sid) {
          await refresh(thread);
          if (identity === account && thread?.session.id === sid) setStatus('该轮已结束，草稿已保留。再次发送可开始新一轮。');
        }
        return;
      }
      identity.drafts.delete(sid);
      identity.extras.delete('session:'+sid);
      if (identity === account && thread?.session.id === sid) {
        prompt.value = '';
        thread.loaded = false;
        thread.writes++;
        await refresh(thread);
      }
    } catch (e) {
      submission.error = '发送失败。可再次发送以重试。' + e.message;
      if (identity === account && thread?.session.id === sid) setStatus(submission.error, 'error');
    } finally {
      submission.busy = false;
      if (identity === account) updateControls();
    }
  }

  stop.addEventListener('click', async () => {
    const t=thread, who=account, run=t?.active?.id;
    if (!run || t.stopping) return;
    const control=t.stopRequest?.run===run ? t.stopRequest : {run,key:uid()};
    t.stopRequest=control;
    t.stopping=true;
    updateControls();
    try {
      if (!control.eventId) {
        const event=await request(who,'events',{body:{content:'停止这次运行。'},key:'event-'+control.key});
        control.eventId=event.id;
      }
      await request(who,'events/'+pathId(control.eventId)+'/deliveries',{
        body:{target:{session_id:t.session.id},run_id:run,action:'request_cancel'},key:'delivery-'+control.key,
      });
      if (thread===t) {t.writes++;await refresh(t);}
    } catch(e) {
      if(thread===t)setStatus('停止失败。'+e.message,'error');
    } finally {
      t.stopping=false;
      if(thread===t)updateControls();
    }
  });

  el('credential-form').addEventListener('submit', async event => {
    event.preventDefault();
    const token = el('token-input').value.trim();
    if (!token) { credentialsError('请输入 Bearer 令牌。'); return; }
    const candidate = {token, controller: new AbortController(), drafts: new Map(), emptyDrafts: new Map(),
      pending: new Map(), extras:new Map(), reading:new Map(), titles:new Map(), lastSessions:new Map()};
    login = candidate;
    credentialsError();
    el('credential-save').disabled = true;
    try {
      const [agents, identity, spec] = await Promise.all([request(candidate, 'agents'), request(candidate, 'identity'), request(candidate, 'openapi.json')]);
      candidate.agents = agents;
      candidate.identity = identity;
      if (login !== candidate) return;
      const preferred = new URLSearchParams(location.hash.slice(1));
      disconnect(true);
      const old = disconnectedDrafts;
      if (old && old.identity.owner === identity.owner && old.identity.subject === identity.subject
          && old.identity.admin === identity.admin) {
        for (const field of ['drafts','emptyDrafts','reading','titles','lastSessions','pending']) candidate[field]=old[field];
        for (const [key,options] of old.extras) {
          candidate.extras.set(key, {attachment:options.attachment,goal:options.goal});
        }
      }
      disconnectedDrafts=null;
      account = candidate;
      desk.connect(identity,spec);
      login = null;
      dialog.close();
      renderAgents();
      const agent = account.agents.find(a => a.id === preferred.get('agent')) || account.agents[0];
      const loading = agent && openAgent(agent.id, preferred.get('session'));
      if (preferred.get('view') === 'oi') {
        if (!identity.admin) await loading;
        if (account === candidate) openOiLink();
      }
    } catch (e) {
      if (login === candidate) credentialsError(e.message);
    } finally {
      if (login === candidate) login = null;
      el('credential-save').disabled = false;
    }
  });

  dialog.addEventListener('close', () => {
    login?.controller.abort();
    login = null;
    el('token-input').value = '';
    credentialsError();
  });
  el('credential-close').addEventListener('click', () => dialog.close());
  el('connection-button').addEventListener('click', showCredentials);
  el('logout').addEventListener('click', () => { disconnect(); dialog.close(); });
  el('new-session').addEventListener('click', () => { desk.close();createSession(); });
  el('create-agent').addEventListener('click', createAgent);
  el('empty-action').addEventListener('click', () => {
    if (!account) showCredentials();
    else if (!workspace) createAgent();
    else if (workspace.listFailed) openAgent(workspace.agent.id);
    else createSession();
  });
  agentPicker.addEventListener('change', () => {desk.close();openAgent(agentPicker.value);});
  el('sidebar-toggle').addEventListener('click', () => drawer(!document.body.classList.contains('sidebar-open')));
  el('sidebar-close').addEventListener('click', () => drawer(false));
  el('sidebar-scrim').addEventListener('click', () => drawer(false));
  mobile.addEventListener('change', () => drawer(false));
  document.addEventListener('keydown', e => { if (e.key === 'Escape') drawer(false); });
  prompt.addEventListener('input', () => {
    if(thread){thread.draftVersion=(thread.draftVersion||0)+1;thread.suggestion='';}
    if (thread) account.drafts.set(thread.session.id, prompt.value);
    else if (workspace) account.emptyDrafts.set(workspace.agent.id, prompt.value);
    updateControls();
  });
  prompt.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.isComposing && e.keyCode !== 229 && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  });
  el('composer').addEventListener('submit', e => { e.preventDefault(); send(); });
  el('session-details').addEventListener('click',()=>thread && desk.open('session',thread.session.id));
  el('goal-toggle').addEventListener('click',()=>{const o=extras(true);if(o){o.goal=!o.goal;updateControls();}});
  el('attach-image').addEventListener('click',()=>el('image-picker').click());
  el('image-picker').addEventListener('change',async event=>{
    const file=event.target.files[0], o=extras(true), who=account;
    event.target.value='';
    if(!file || !o)return;
    if(file.size>8*1024*1024){setStatus('图片超过 8 MiB','error');return;}
    const ticket=(o.uploadTicket||0)+1;
    o.uploadTicket=ticket;
    o.uploading=true;updateControls();
    try{
      setStatus('正在准备图片…');
      const encoded=await new Promise((ok,fail)=>{const r=new FileReader();r.onload=()=>ok(r.result.split(',')[1]);r.onerror=()=>fail(Error('图片读取失败'));r.readAsDataURL(file);});
      const a=await request(who,'attachments',{body:{mime_type:file.type,data:encoded}});
      if(o.uploadTicket===ticket)o.attachment={id:a.id,name:file.name};
      if(extras()===o){setStatus();updateControls();}
    }catch(e){if(extras()===o)setStatus(e.message,'error');}
    finally{if(o.uploadTicket===ticket){o.uploading=false;if(extras()===o)updateControls();}}
  });
  el('suggest-next').addEventListener('click',async()=>{
    const t=thread,reply=t?.reply,version=t?.historyLength, draft=t?.draftVersion||0;
    if(!reply||prompt.value.trim())return;
    try{
      el('suggest-next').disabled=true;
      const r=await request(account,'sessions/'+pathId(t.session.id)+'/suggestions',{body:{expected_version:version,after_reply_id:reply},signal:t.controller.signal});
      if(thread!==t || t.reply!==reply || t.historyLength!==version || (t.draftVersion||0)!==draft || prompt.value.trim())return;
      if(r.based_on_version===version && typeof r.suggestion==='string')t.suggestion=r.suggestion;
      else setStatus('当前没有可用的推荐草稿。');
      updateControls();
    }catch(e){if(thread===t)setStatus(e.message,'error');}
    finally{
      el('suggest-next').disabled=false;
      if(thread===t) refreshUsage(t);
    }
  });
  el('suggested-draft').addEventListener('click', async () => {
    const t = thread, suggestion = t?.suggestion, version = t?.historyLength;
    if (!suggestion || prompt.value.trim()) return;
    try {
      const latest = await request(account, 'sessions/' + pathId(t.session.id), {signal: t.controller.signal});
      if (thread !== t || t.suggestion !== suggestion || prompt.value.trim()) return;
      t.suggestion = '';
      if (latest.version !== version || t.historyLength !== version) {
        await refresh(t);
        if (thread === t) setStatus('会话已更新，请重新生成推荐。');
        return;
      }
      prompt.value = suggestion;
      prompt.dispatchEvent(new Event('input'));
      prompt.focus();
    } catch (e) {
      if (thread === t) setStatus(e.message, 'error');
    }
  });
  drawer(false);
  updateControls();
  if (oiRequested()) openOiLink();
})();
