// Bind onChange to the draft and suggestion displayed in one input box.
export class Composer {
  #sequence = 0
  #session
  #reply
  #version
  #enabled = true
  #draft = ''
  #suggestion = null

  constructor(sample, onChange) {
    this.sample = sample
    this.onChange = onChange
  }

  #changed() {
    this.#sequence += 1
    this.#suggestion = null
    this.#publish()
  }

  #publish() {
    this.onChange({draft: this.#draft, suggestion: this.#suggestion})
  }

  select(session, reply, version, draft = '') {
    this.#session = session
    this.#reply = reply
    this.#version = version
    this.#draft = draft
    this.#changed()
  }

  setEnabled(enabled) {
    this.#enabled = enabled
    this.#changed()
  }

  edit(text) {
    this.#draft = text
    this.#changed()
  }

  accept() {
    if (this.#suggestion) this.edit(this.#suggestion)
  }

  // Call at send intent, before awaiting the HTTP acknowledgement.
  takeForSend() {
    const message = this.#draft
    this.edit('')
    return message
  }

  async refresh() {
    if (!this.#enabled || this.#draft || !this.#reply) return
    const generation = ++this.#sequence
    const response = await this.sample(this.#session, {after_reply_id: this.#reply, expected_version: this.#version})
    if (this.#sequence !== generation || response.based_on_version !== this.#version) return
    this.#suggestion = typeof response.suggestion === 'string' ? response.suggestion : null
    this.#publish()
  }
}
