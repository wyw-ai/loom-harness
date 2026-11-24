import test from 'node:test'
import assert from 'node:assert/strict'
import {Composer} from '../examples/composer.mjs'

test('C09 acceptance and edits leave delivery to the send action', async () => {
  let view
  const c = new Composer(async () => ({based_on_version: 18, suggestion: '推到远端'}), v => { view = v })
  c.select('S', 'R', 18)
  await c.refresh()
  c.accept()
  assert.equal(view.draft, '推到远端')
  c.edit('先看差异')
  assert.equal(c.takeForSend(), '先看差异')
  assert.deepEqual(view, {draft: '', suggestion: null})
})

test('C10 late response never populates an obsolete or edited field', async () => {
  for (const action of ['send', 'switch', 'disable', 'edit', 'version']) {
    let resolve
    let view
    const c = new Composer(() => new Promise(r => { resolve = r }), v => { view = v })
    c.select('S', 'R', 18)
    const inFlight = c.refresh()
    if (action === 'send') { c.edit('先别推'); assert.equal(c.takeForSend(), '先别推') }
    if (action === 'switch') c.select('S2', 'R2', 18)
    if (action === 'disable') c.setEnabled(false)
    if (action === 'edit') c.edit('现有草稿')
    resolve({based_on_version: action === 'version' ? 17 : 18, suggestion: '推到远端'})
    await inFlight
    assert.deepEqual(view, {draft: action === 'edit' ? '现有草稿' : '', suggestion: null}, action)
  }
})
