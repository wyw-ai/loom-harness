import assert from 'node:assert/strict'
import {Composer} from '../examples/composer.mjs'

const [base, session, reply, rawVersion, scenario] = process.argv.slice(2)
const version = Number(rawVersion)
async function request(method, path, body) {
  const response = await fetch(base + '/api/' + path, {
    method, headers: {Authorization: 'Bearer user-token', 'Content-Type': 'application/json'},
    body: body && JSON.stringify(body),
  })
  assert.ok(response.ok, method + ' ' + path + ' returned ' + response.status)
  return response.json()
}
let arrived, release, view
const received = new Promise(resolve => { arrived = resolve })
const deliveryGate = new Promise(resolve => { release = resolve })
const composer = new Composer(async (sid, args) => {
  const response = await request('POST', 'sessions/' + sid + '/suggestions', args)
  assert.equal(response.suggestion, '推到远端')
  arrived()
  if (scenario === 'late') await deliveryGate
  return response
}, next => { view = next })
composer.select(session, reply, version)
const flight = composer.refresh()
await received
if (scenario === 'accept') {
  await flight
  composer.accept()
  assert.equal(view.draft, '推到远端')
  composer.edit('先看差异')
} else {
  composer.edit('先别推')
}
assert.equal((await request('GET', 'sessions/' + session)).version, version)
const event = await request('POST', 'events', {content: composer.takeForSend()})
const delivered = await request('POST', 'events/' + event.id + '/deliveries',
  {target: {session_id: session}, action: 'start_run'})
release()
await flight
assert.deepEqual(view, {draft: '', suggestion: null})
console.log(JSON.stringify({run_id: delivered.run_id, event_id: event.id}))
