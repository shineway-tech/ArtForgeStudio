const assert = require('node:assert/strict');
const test = require('node:test');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const script = fs.readFileSync(path.join(__dirname, '../src/runtime/payment_qr_ready.js'), 'utf8');
function fixture(images = [], canvases = [], subframe = false) {
  const messages = [];
  let poll;
  let cleared = false;
  const window = { ipc: { postMessage: (message) => messages.push(message) } };
  window.top = subframe ? {} : window;
  vm.runInNewContext(script, {
    window, document: { images, querySelectorAll: () => canvases },
    setInterval: (callback) => { poll = callback; return 1; },
    clearInterval: () => { cleared = true; },
  });
  return { messages, tick: () => poll?.(), get cleared() { return cleared; }, get started() { return !!poll; } };
}
test('a navigation error page never claims visual readiness', () => {
  const page = fixture();
  for (let index = 0; index < 120; index += 1) page.tick();
  assert.deepEqual(page.messages, []);
  assert.equal(page.cleared, true);
});
test('waits for an actually loaded and visible square image', () => {
  const image = { complete: false, naturalWidth: 240, naturalHeight: 240,
    getBoundingClientRect: () => ({ width: 240, height: 240 }) };
  const page = fixture([image]);
  page.tick(); assert.deepEqual(page.messages, []);
  image.complete = true;
  page.tick(); assert.deepEqual(page.messages, ['qr-content-ready']);
  assert.equal(page.cleared, true);
});
test('hidden images, small icons and subframes do not announce the QR', () => {
  const hidden = { complete: true, naturalWidth: 240, naturalHeight: 240,
    getBoundingClientRect: () => ({ width: 0, height: 0 }) };
  const icon = { complete: true, naturalWidth: 48, naturalHeight: 48,
    getBoundingClientRect: () => ({ width: 240, height: 240 }) };
  const page = fixture([hidden, icon]);
  page.tick(); assert.deepEqual(page.messages, []);
  assert.equal(fixture([], [], true).started, false);
});
