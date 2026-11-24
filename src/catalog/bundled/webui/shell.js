window.createLoomShell = function createLoomShell(client) {
  'use strict';
  const el = id => document.getElementById(id);
  const dialog = el('quick-dialog'), input = el('quick-query'), results = el('quick-results');
  const nav = el('desk-nav');
  let generation = 0, tools = [], actions = [], selected = 0;
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);
  el('quick-open').querySelector('kbd').textContent = mac ? '⌘ K' : 'Ctrl K';

  function workTab(work) {
    document.body.classList.toggle('workspace-nav',work);
    el('nav-workspace').setAttribute('aria-pressed',String(work));
    el('nav-chats').setAttribute('aria-pressed',String(!work));
  }
  el('nav-workspace').onclick = () => workTab(true);
  el('nav-chats').onclick = () => {
    workTab(false);
    if(document.body.classList.contains('in-desk')) el('desk-chat').click();
  };
  el('sidebar-fold').onclick = () => {
    document.body.classList.add('sidebar-folded'); el('sidebar-show').focus();
  };
  el('sidebar-show').onclick = () => {
    document.body.classList.remove('sidebar-folded'); el('sidebar-fold').focus();
  };
  let wasDesk = false;
  new MutationObserver(() => {
    const inDesk = document.body.classList.contains('in-desk');
    if (inDesk !== wasDesk) {workTab(inDesk); wasDesk=inDesk;}
  }).observe(document.body,{attributes:true,attributeFilter:['class']});
  function syncIdentity() {
    const guest = !nav.childElementCount;
    el('nav-workspace').disabled=guest;
    if(guest) { workTab(false); if(dialog.open)dialog.close(); }
  }
  new MutationObserver(syncIdentity).observe(nav,{childList:true});
  syncIdentity();
  const make = (tag, value, cls) => {
    const e=document.createElement(tag);
    if(value) e.textContent=value;
    if(cls) e.className=cls;
    return e;
  };
  function entries(query) {
    const items=[], add=(group,label,detail,run)=>items.push({group,label,detail,run});
    const isAdmin=!!nav.querySelector('[data-section="plugins"]');
    if(!nav.childElementCount) {
      add('账户','连接账户','',()=>el('connection-button').click());
      return items;
    }
    add('操作','新建会话','',client.newChat);
    if(isAdmin) add('操作','接入 OI','连接主机 · 发现 OIS Service 并授权给 Agent',()=>client.openDesk('oi'));
    for(const b of document.querySelectorAll('#session-list .session-button'))
      add('当前 Agent 的会话',b.title || b.textContent,'',()=>b.click());
    for(const a of client.listAgents())
      add('切换 Agent',a.name || a.id,a.id,()=>client.openAgent(a.id));
    for(const b of nav.querySelectorAll('[data-section]:not([data-section="oi"])'))
      add('工作台',b.lastElementChild.textContent,b.dataset.section,()=>client.openDesk(b.dataset.section));
    if(query) for(const t of tools) {
      const name=t.model_name || t.id;
      if(!name) continue;
      add('当前 Agent 的工具',name,t.description || t.id,async ()=>{
        const agent=el('agent-select').value;
        const granted=await client.listTools();
        if (el('agent-select').value!==agent) return;
        const current=granted?.find(item=>item.id===t.id && (item.model_name || item.id)===name);
        if(!current) throw new Error('当前 Agent 的工具已变化，请重新选择。');
        const hint=current.webui?.suggestion;
        const label=typeof hint?.label==='string' && hint.label.trim();
        const prompt=typeof hint?.prompt==='string' && hint.prompt.trim();
        client.draft(label && label.length<=64 && prompt && prompt.length<=1200 ? prompt : '请告诉我 '+name+' 能做什么，以及它需要哪些参数。');
      });
    }
    return items;
  }
  function highlight(i,scroll) {
    selected=i;
    const buttons=results.querySelectorAll('button');
    buttons.forEach((b,n)=>b.setAttribute('aria-selected',String(i===n)));
    if(buttons[i]) {
      input.setAttribute('aria-activedescendant',buttons[i].id);
      if(scroll) buttons[i].scrollIntoView({block:'nearest'});
    } else input.removeAttribute('aria-activedescendant');
  }
  async function pick(i) {
    const action=actions[i];
    if(!action) return;
    dialog.close();
    try { await action.run(); }
    catch(e) {const status=el('status'); status.textContent=e.message || '操作未完成'; status.dataset.kind='error';}
  }
  function render() {
    const query=input.value.trim().toLowerCase();
    const all=entries(query);
    actions=all.filter(a=>(a.label+' '+a.detail+' '+a.group).toLowerCase().includes(query)).slice(0,50);
    results.replaceChildren();
    let prior='';
    for(const [i,a] of actions.entries()) {
      if(a.group!==prior) {results.append(make('div',a.group,'quick-group'));prior=a.group;}
      const b=make('button',null,'quick-item'); b.type='button'; b.id='quick-option-'+i; b.setAttribute('role','option');
      b.append(make('span',a.label)); if(a.detail)b.append(make('small',a.detail));
      b.onclick=()=>pick(i); b.onpointermove=()=>highlight(i,false);
      results.append(b);
    }
    if(!actions.length) results.append(make('p','没有匹配结果','quick-empty'));
    highlight(0,false);
  }
  input.setAttribute('role','combobox'); input.setAttribute('aria-controls','quick-results');
  input.setAttribute('aria-autocomplete','list'); input.setAttribute('aria-expanded','true');
  results.setAttribute('role','listbox');
  input.addEventListener('input',render);
  dialog.addEventListener('keydown',event=>{
    if(event.isComposing || !actions.length)return;
    if(event.key==='ArrowDown'||event.key==='ArrowUp'){
      event.preventDefault();highlight((selected+(event.key==='ArrowDown'?1:-1)+actions.length)%actions.length,true);
    } else if(event.key==='Enter' && event.target===input){event.preventDefault();pick(selected);}
  });
  async function show() {
    if(dialog.open || document.querySelector('dialog[open]'))return;
    const version=++generation;
    input.value='';tools=[];render();dialog.showModal();input.focus();
    if(!nav.childElementCount)return;
    try{
      const available=await client.listTools();
      if(dialog.open && version===generation){tools=available||[];render();}
    }catch{ /* Navigation and local session search still work. */ }
  }
  dialog.addEventListener('close',()=>{generation++;tools=[];actions=[];results.replaceChildren();});
  dialog.addEventListener('click',event=>{if(event.target===dialog)dialog.close();});
  el('quick-close').onclick=()=>dialog.close();
  el('quick-open').onclick=show;
  document.addEventListener('keydown',event=>{
    if((event.metaKey||event.ctrlKey) && !event.altKey && event.key.toLowerCase()==='k'){
      event.preventDefault(); if(dialog.open)dialog.close();else show();
    }
  });

  const prompt=el('prompt'), picker=make('div',null,'compose-picker');
  picker.hidden=true;picker.id='compose-picker';picker.setAttribute('role','listbox');picker.setAttribute('aria-label','输入建议');
  el('composer').append(picker);
  let stamp=0, range=null, options=[], position=0;
  function hidePicker() {
    stamp++;picker.hidden=true;picker.replaceChildren();range=null;options=[];
    prompt.removeAttribute('aria-controls');prompt.removeAttribute('aria-activedescendant');
  }
  function chooseOption(index) {
    if(!range || !options[index]) return;
    const o=options[index], {start,end}=range;
    hidePicker();
    prompt.setRangeText(o.insert || '',start,end,'end');
    prompt.dispatchEvent(new Event('input'));prompt.focus();
    if(o.action) o.action();
  }
  function markOption(index) {
    position=index;
    Array.from(picker.querySelectorAll('button')).forEach((b,i)=>b.setAttribute('aria-selected',String(i===index)));
    const active=picker.querySelector('[aria-selected=true]');
    if(active){prompt.setAttribute('aria-activedescendant',active.id);active.scrollIntoView({block:'nearest'});}
  }
  async function suggest(event) {
    if(event?.isComposing || prompt.disabled || prompt.selectionStart!==prompt.selectionEnd) {hidePicker();return;}
    const before=prompt.value.slice(0,prompt.selectionStart);
    const match=/(?:^|\s)([/@])([^\s/@]{0,80})$/.exec(before);
    if(!match){hidePicker();return;}
    const type=match[1], query=match[2].toLowerCase(), end=before.length;
    const version=++stamp;
    let rows=[];
    if(type==='/'){
      rows.push({label:'添加图片',meta:'附件',action:()=>el('attach-image').click(),enabled:!el('attach-image').disabled},
                {label:'目标模式',meta:'切换',action:()=>el('goal-toggle').click(),enabled:!el('goal-toggle').disabled});
      for(const hint of client.listHints()) rows.push({...hint,meta:'草稿'});
    }
    if(type==='@'){
      let available=[];
      try {available=await client.listTools() || [];} catch { if(version===stamp) hidePicker();return; }
      if(version!==stamp||prompt.value.slice(0,prompt.selectionStart)!==before||document.activeElement!==prompt)return;
      for(const t of available) {
        const name=t.model_name || t.id;
        if(name)rows.push({label:name,meta:t.description||'',insert:name+' '});
      }
    }
    range={start:end-match[2].length-1,end};position=0;
    options=rows.filter(r=>r.enabled!==false && (r.label+' '+r.meta).toLowerCase().includes(query)).slice(0,20);
    picker.replaceChildren(make('div',type==='@'?'当前 Agent 的工具':'对话操作','quick-group'));
    for(const [i,o] of options.entries()){
      const b=make('button',null,'quick-item');b.id='compose-option-'+i;b.type='button';b.setAttribute('role','option');
      b.append(make('span',o.label),make('small',o.meta));b.onpointerdown=e=>e.preventDefault(); b.onclick=()=>chooseOption(i);
      picker.append(b);
    }
    if(!options.length)picker.append(make('div','没有匹配项','quick-empty'));
    picker.hidden=false;prompt.setAttribute('aria-controls',picker.id);markOption(0);
  }
  prompt.addEventListener('input',suggest);
  prompt.addEventListener('compositionend',suggest);
  prompt.addEventListener('blur',hidePicker);
  document.addEventListener('selectionchange',()=>{
    if(!range || picker.hidden)return;
    if(prompt.selectionStart!==range.end || prompt.selectionEnd!==range.end)hidePicker();
  });
  prompt.addEventListener('keydown',event=>{
    if(picker.hidden || event.isComposing || event.shiftKey || event.ctrlKey || event.metaKey)return;
    if(event.key==='Escape')hidePicker();
    else if(options.length && (event.key==='ArrowDown'||event.key==='ArrowUp'))
      markOption((position+(event.key==='ArrowDown'?1:-1)+options.length)%options.length);
    else if(options.length && event.key==='Enter')chooseOption(position);
    else return;
    event.preventDefault();event.stopImmediatePropagation();
  },true);
};
