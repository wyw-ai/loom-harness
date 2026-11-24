// Render model text using DOM nodes. Raw HTML remains text even while streaming.
window.LoomText = (() => {
  'use strict';
  const cache = new WeakMap();
  const tag = (name, text) => {
    const element = document.createElement(name);
    if (text !== undefined) element.textContent = text;
    return element;
  };
  function inline(parent, text) {
    const tokens = /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|\[[^\]\n]+\]\(https?:\/\/[^\s)]+\))/g;
    let start = 0;
    for (const match of text.matchAll(tokens)) {
      parent.append(text.slice(start, match.index));
      const value = match[0];
      if (value.startsWith('`')) parent.append(tag('code', value.slice(1, -1)));
      else if (value.startsWith('**')) parent.append(tag('strong', value.slice(2, -2)));
      else if (value.startsWith('*')) parent.append(tag('em', value.slice(1, -1)));
      else {
        const end = value.indexOf('](');
        const anchor = tag('a', value.slice(1, end));
        anchor.href = value.slice(end + 2, -1);
        anchor.target = '_blank'; anchor.rel = 'noopener noreferrer';
        parent.append(anchor);
      }
      start = match.index + value.length;
    }
    parent.append(text.slice(start));
    return parent;
  }
  const list = /^\s*([-*+]\s+|\d+[.)]\s+)(.*)$/;
  const fence = /^\s*```/;
  const heading = /^(#{1,6})\s+(.*)$/;
  const cells = line => line.trim().replace(/^\||\|$/g, '').split(/(?<!\\)\|/).map(s => s.trim().replace(/\\\|/g, '|'));
  const tableAt = (lines, at) => lines[at]?.includes('|') && lines[at+1]?.includes('|')
    && cells(lines[at+1]).every(s => /^:?-{3,}:?$/.test(s)) && cells(lines[at]).length === cells(lines[at+1]).length;
  function blocks(text) {
    const lines = text.replace(/\r\n?/g, '\n').split('\n'), result = [];
    for (let i = 0; i < lines.length;) {
      if (!lines[i].trim()) { i++; continue; }
      const start = i, line = lines[i++];
      let type = 'p';
      if (fence.test(line)) {
        type = 'code';
        while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) i++;
        if (i < lines.length) i++;
      } else if (tableAt(lines, start)) {
        type = 'table'; i++;
        while (i < lines.length && lines[i].includes('|') && !fence.test(lines[i])) i++;
      } else if (heading.test(line)) type = 'heading';
      else if (list.test(line)) {
        type = /^\s*\d/.test(line) ? 'ol' : 'ul';
        while (i < lines.length && list.test(lines[i]) && /^\s*\d/.test(lines[i]) === (type === 'ol')) i++;
      } else if (/^>\s?/.test(line)) {
        type = 'quote';
        while (i < lines.length && /^>/.test(lines[i])) i++;
      } else {
        while (i < lines.length && lines[i].trim() && !fence.test(lines[i]) && !heading.test(lines[i])
          && !list.test(lines[i]) && !lines[i].startsWith('>') && !tableAt(lines,i)) i++;
      }
      result.push({type, lines:lines.slice(start,i), raw:lines.slice(start,i).join('\n')});
    }
    return result;
  }
  function block({type, lines, raw}) {
    if (type === 'code') {
      const panel = tag('div'), bar=tag('div'), pre=tag('pre'), copy=tag('button','复制');
      panel.className='md-code'; bar.className='md-code-head'; copy.type='button';
      const hasEnd = lines.length > 1 && /^\s*```\s*$/.test(lines.at(-1));
      const value = lines.slice(1,hasEnd ? -1 : undefined).join('\n');
      copy.disabled = !navigator.clipboard;
      copy.addEventListener('click',async () => {
        try { await navigator.clipboard.writeText(value); copy.textContent='已复制'; }
        catch { copy.textContent='复制失败'; }
        setTimeout(() => {copy.textContent='复制';},1500);
      });
      bar.append(tag('span',lines[0].trim().slice(3) || '代码'),copy);
      pre.append(tag('code',value)); panel.append(bar,pre); return panel;
    }
    if (type === 'table') {
      const wrap=tag('div'), table=tag('table'), header=table.createTHead().insertRow(), body=table.createTBody();
      wrap.className='md-table'; wrap.tabIndex=0; wrap.setAttribute('aria-label','表格');
      for (const col of cells(lines[0])) {const th=tag('th'); th.scope='col'; header.append(inline(th,col));}
      for (const line of lines.slice(2)) {const row=body.insertRow(); for(const col of cells(line)) inline(row.insertCell(),col);}
      wrap.append(table); return wrap;
    }
    if (type === 'ul' || type === 'ol') {
      const el=tag(type);
      if(type==='ol') el.start=parseInt(lines[0].trim(),10);
      for(const line of lines) el.append(inline(tag('li'), line.match(list)[2]));
      return el;
    }
    if(type==='heading') {const [,marks,title]=lines[0].match(heading); return inline(tag('h'+Math.min(marks.length+2,6)),title);}
    if(type==='quote') return inline(tag('blockquote'),lines.map(x=>x.replace(/^>\s?/,'')).join('\n'));
    return inline(tag('p'),raw);
  }
  function render(parent, text) {
    const previous=cache.get(parent)||[], next=blocks(text);
    if(!cache.has(parent)) { parent.replaceChildren(); parent.classList.add('loom-prose'); }
    for(let i=0;i<next.length;i++) {
      const b=next[i], old=previous[i];
      if(old && old.type===b.type && old.raw===b.raw && old.node.parentNode===parent) b.node=old.node;
      else {
        b.node=block(b);
        if(old?.node.parentNode===parent) old.node.replaceWith(b.node);
        else parent.append(b.node);
      }
    }
    for(const b of previous.slice(next.length)) b.node.remove();
    cache.set(parent,next);
  }
  return {render};
})();
