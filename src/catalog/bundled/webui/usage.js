/* Rendering of the server's Session and Run usage snapshots. */
window.LoomUsage = (() => {
  'use strict';
  const unknown = '—';
  const number = new Intl.NumberFormat('zh-CN', {maximumFractionDigits: 1});
  const short = new Intl.NumberFormat('en', {notation: 'compact', maximumFractionDigits: 1});
  const valid = value => Number.isFinite(value) && value >= 0;
  const count = value => valid(value) ? number.format(value) : unknown;
  const compact = value => valid(value) ? (value < 1000 ? String(value) : short.format(value)) : unknown;
  const percent = ratio => valid(ratio) ? number.format(ratio * 100) + '%' : unknown;
  const partial = row => row?.calls?.total > row?.calls?.reported;
  function duration(ms, precision) {
    if (!valid(ms)) return unknown;
    const seconds = ms / 1000;
    if (precision >= 1000 && seconds < 1) return '不足 1 秒';
    const prefix = precision >= 1000 ? '约 ' : '';
    if (ms < 1000) return count(ms) + ' 毫秒';
    if (seconds < 60) return prefix + count(seconds) + ' 秒';
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return prefix + minutes + ' 分 ' + Math.floor(seconds % 60) + ' 秒';
    return prefix + Math.floor(minutes / 60) + ' 小时 ' + minutes % 60 + ' 分';
  }
  function bytes(value) {
    if (!valid(value)) return unknown;
    if (value < 1024) return count(value) + ' B';
    if (value < 1048576) return count(value / 1024) + ' KiB';
    return count(value / 1048576) + ' MiB';
  }
  const text = (name, value, cls) => {
    const node = document.createElement(name);
    node.textContent = value;
    if (cls) node.className = cls;
    return node;
  };
  function total(row, exact = false) {
    const value = row?.tokens?.total;
    if (!valid(value)) return unknown;
    return (partial(row) ? '≥ ' : '') + (exact ? count(value) : compact(value));
  }
  function runLabel(row) {
    return total(row)+' token · '+duration(row.elapsed_ms,row.elapsed_precision_ms);
  }
  function renderRunBadge(badge, row) {
    badge.className = 'run-usage';
    badge.dataset.runId = row.run_id;
    const label = runLabel(row);
    if (badge.textContent !== label) badge.textContent = label;
    badge.title = '本轮用量 ' + total(row,true) + ' token · 从受理到结束或当前时刻的耗时';
    if (partial(row)) badge.title += ' · 部分模型调用尚未上报用量';
    return badge;
  }
  function runDetails(row, id) {
    const group = document.createElement('section');
    group.className = 'run-usage-details desk-group';
    group.dataset.runId = id;
    group.append(text('h3', '本轮用量'));
    if (!row) {group.append(text('p', '暂时无法读取运行统计。')); return group;}
    const showMillis = row.elapsed_precision_ms === 1 && row.elapsed_ms >= 1000;
    const elapsed = duration(row.elapsed_ms,row.elapsed_precision_ms) + (showMillis ? '（'+count(row.elapsed_ms)+' 毫秒）' : '');
    const fields = [['总消耗', total(row, true) + ' token'],
      ['输入 / 输出', count(row.tokens?.input) + ' / ' + count(row.tokens?.output) + ' token'],
      ['总耗时', elapsed]];
    if (partial(row)) fields.push(['上报情况', count(row.calls.reported) + ' / ' + count(row.calls.total) + ' 次模型调用已上报用量']);
    const list = document.createElement('dl'); list.className='desk-facts';
    for (const [label,value] of fields) list.append(text('dt',label),text('dd',value));
    group.append(list);
    return group;
  }
  function contextUsage(ctx) {
    return valid(ctx?.used_bytes) && ctx.limit_bytes > 0 ? ctx.used_bytes / ctx.limit_bytes : null;
  }
  const stateNames = {
    empty:'尚无模型调用', compacted:'已压缩', compacting:'正在压缩', failed:'压缩失败', unavailable:'缺少采样',
  };
  function speedReading(speed) {
    const rate=value => count(value)+' tok/s';
    const full=speed?.output_tokens_per_second, decode=speed?.decode_tokens_per_second;
    if (valid(decode) && speed.decode_sampled_calls>0) {
      const detail='按 '+count(speed.decode_sampled_calls)+' 次调用加权，从首个推理、正文或工具参数片段到输出结束。';
      return {label:'生成速度',value:rate(decode),detail,full:valid(full)?'含首段等待的全程速度 '+rate(full):null};
    }
    if (valid(full)) {
      const measured=speed.sampled_calls>0 ? count(speed.sampled_calls)+' 次模型请求' : '模型请求';
      return {label:'全程速度',value:rate(full),detail:'含首段等待，按'+measured+'的输出 token 和总耗时计算。'};
    }
    return {label:'生成速度',value:unknown,detail:'等待模型回传输出用量及可测耗时。'};
  }
  function sessionFacts(s) {
    if (!s) return text('p','暂时无法读取会话用量。');
    const {context:c,cache}=s, ratio=contextUsage(c), speed=speedReading(s.speed);
    const entries=[
      ['Token 消耗',total(s,true)+' token','输入 '+count(s.tokens?.input)+' · 输出 '+count(s.tokens?.output)],
      [speed.label,speed.value,speed.detail],
      ['自动压缩',c?.state==='observed' && valid(ratio) ? (ratio>=1 ? '已达压缩阈值' : '未达压缩阈值') : (stateNames[c?.state] || unknown),
        '已压缩 '+count(c?.compactions)+' 次 · 输入 '+bytes(c?.used_bytes)+' / 阈值 '+bytes(c?.limit_bytes)],
      ['缓存命中率',percent(cache?.hit_rate),'命中 '+count(cache?.hit_tokens)+' · 未命中 '+count(cache?.miss_tokens)+' 输入 token'],
    ];
    const list=document.createElement('dl'); list.className='usage-rows';
    for(const [label,value,detail] of entries) {
      const dd=document.createElement('dd');
      dd.append(text('strong',value),text('small',detail));
      if(label===speed.label && speed.full) dd.append(text('small',speed.full));
      if(label==='Token 消耗' && s.suggestions?.calls?.total) {
        dd.append(text('small','含输入框推荐 '+total(s.suggestions,true)+' token · '+count(s.suggestions.calls.total)+' 次采样'));
      }
      if(label==='自动压缩' && valid(ratio)) {
        const meter=document.createElement('meter');
        meter.max=1;meter.value=Math.min(1,ratio);meter.setAttribute('aria-label','压缩阈值占用 '+percent(ratio));
        dd.append(meter);
        if(valid(c.before_bytes)) dd.append(text('small','压缩前 '+bytes(c.before_bytes)+' · 阈值占用 '+percent(ratio)));
      }
      if(label==='缓存命中率' && cache?.sampled_calls) dd.append(text('small','来自 '+count(cache.sampled_calls)+' 次有缓存回执的调用'));
      list.append(text('dt',label),dd);
    }
    return list;
  }
  function createSessionMeter(host, refresh) {
    const state=host.querySelector('.usage-state'), rows=host.querySelector('.usage-body');
    const fields=Object.fromEntries(Array.from(host.querySelectorAll('[data-metric]'), n=>[n.dataset.metric,n]));
    let snapshot, signature, loadState;
    function render() {
      const s=snapshot, c=s?.context, speed=s?.speed, cache=s?.cache;
      const ratio=contextUsage(c);
      const rate=speedReading(speed);
      host.querySelector('[data-speed-label]').textContent=rate.label;
      const ctx=c?.state==='compacting' ? '进行中' : c?.state==='failed' ? '失败' : percent(ratio);
      for (const [key,value] of Object.entries({tokens:total(s),speed:rate.value,context:ctx,cache:percent(cache?.hit_rate)})) {
        if(fields[key].textContent!==value) fields[key].textContent=value;
      }
      const status=loadState==='error' ? (s ? '更新失败，显示上次读取的用量。' : '暂时无法读取会话用量。') :
        !s ? '正在读取用量…' : partial(s) ? '已上报用量的模型调用 '+count(s.calls.reported)+' / '+count(s.calls.total)+' 次。' : '';
      state.textContent=status;
      state.hidden=!status;
      host.dataset.state=loadState;
      if(s) rows.replaceChildren(sessionFacts(s));
      else rows.replaceChildren();
    }
    const clear=()=>{
      host.hidden=true;host.open=false;snapshot=undefined;signature=undefined;loadState='loading';render();
    };
    host.querySelector('#usage-refresh').addEventListener('click',refresh);
    document.addEventListener('pointerdown',event=>{if(host.open && !host.contains(event.target))host.open=false;});
    document.addEventListener('keydown',event=>{
      if(event.key==='Escape' && host.open){
        host.open=false;host.querySelector('summary').focus();event.preventDefault();
      }
    });
    return {clear, start(){clear();host.hidden=false;},
      update(s) {
        const next=JSON.stringify([s.tokens,s.calls,s.speed,s.cache,s.context,s.suggestions]);
        if(next===signature && loadState==='ready')return;
        signature=next;snapshot=s;loadState='ready';render();
      },
      failed(){loadState='error';render();},
    };
  }
  return {createSessionMeter,renderRunBadge,runLabel,runDetails,sessionFacts};
})();
