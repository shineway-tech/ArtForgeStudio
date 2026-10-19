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
  const styles = () => ({ setProperty(name, value) { this[name] = value; } });
  const body = { style: styles() };
  const root = { style: styles(), clientWidth: 320, clientHeight: 320 };
  let resize;
  const window = { ipc: { postMessage: (message) => messages.push(message) },
    innerWidth: 320, innerHeight: 320,
    getComputedStyle: () => ({ transform: 'none' }),
    addEventListener: (event, callback) => { if (event === 'resize') resize = callback; },
  };
  window.top = subframe ? {} : window;
  vm.runInNewContext(script, {
    window, document: { images, body, documentElement: root, querySelectorAll: () => canvases },
    setInterval: (callback) => { poll = callback; return 1; },
    clearInterval: () => { cleared = true; },
  });
  return { messages, body, root, window, resize: () => resize?.(), tick: () => poll?.(), get cleared() { return cleared; }, get started() { return !!poll; } };
}
test('a navigation error page never claims visual readiness', () => {
  const page = fixture();
  for (let index = 0; index < 120; index += 1) page.tick();
  assert.deepEqual(page.messages, []);
  assert.equal(page.cleared, true);
});
test('waits for an actually loaded and visible square image', () => {
  const image = { complete: false, naturalWidth: 240, naturalHeight: 240,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 240, height: 240 }) };
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

test('centers the complete provider page before revealing the QR and after resize', () => {
  let page;
  const canvas = { width: 240, height: 240, getBoundingClientRect: () => {
    const translation = page.body.style.transform?.match(/translate\(([-\d.]+)px, ([-\d.]+)px\)/);
    return { left: 8 + Number(translation?.[1] || 0), top: 12 + Number(translation?.[2] || 0), width: 240, height: 240 };
  } };
  page = fixture([], [canvas]);
  page.tick();
  assert.deepEqual(page.messages, ['qr-content-ready']);
  assert.equal(canvas.getBoundingClientRect().left, 40);
  assert.equal(canvas.getBoundingClientRect().top, 40);
  assert.equal(page.root.style.overflow, 'hidden');
  page.window.innerWidth = page.root.clientWidth = 360;
  page.window.innerHeight = page.root.clientHeight = 300;
  page.resize();
  assert.equal(canvas.getBoundingClientRect().left, 60);
  assert.equal(canvas.getBoundingClientRect().top, 30);
  page.resize();
  assert.equal(canvas.getBoundingClientRect().left, 60, 'repeated layout must not accumulate offsets');
  assert.deepEqual(page.messages, ['qr-content-ready'], 'layout changes must not report payment success');
});

test('centers against the viewport even when the provider uses quirks layout', () => {
  let page;
  const image = { complete: true, naturalWidth: 240, naturalHeight: 240,
    getBoundingClientRect: () => {
      const translation = page.body.style.transform?.match(/translate\(([-\d.]+)px, ([-\d.]+)px\)/);
      return { left: 8 + Number(translation?.[1] || 0), top: 8 + Number(translation?.[2] || 0), width: 240, height: 240 };
    } };
  page = fixture([image]);
  page.root.clientHeight = 256;
  page.tick();
  assert.equal(image.getBoundingClientRect().left, 40);
  assert.equal(image.getBoundingClientRect().top, 40);
});
