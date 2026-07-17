# 生图精确比例与清晰度尺寸传递 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让客户端保留用户选择的精确比例，由后端按比例和 `1K/2K/4K` 最长边计算尺寸，并把精确 `size` 传给中转站计费。

**Architecture:** 客户端 API 只传服务端允许的比例标识；后端新增无 IO 的尺寸工具，在任务创建时把标准化比例、目标宽高和 `provider_size` 固化进 JSON 快照。Worker 从快照构造图片模型请求并复用同一目标尺寸处理返回图片，旧任务通过 `square/landscape/portrait` 回退路径继续执行。

**Tech Stack:** Rust 2021、Slint、reqwest、Node.js 24、Koa、Joi、Sequelize、node:test、Sharp、OpenAI-compatible image API。

## Global Constraints

- `1K`、`2K`、`4K` 的最长边必须分别为 `1024`、`2048`、`4096`。
- 短边按真实比例计算并四舍五入到最接近的 8 像素倍数，最小 64 像素。
- 支持 `1:1`、`3:2`、`2:3`、`4:3`、`3:4`、`5:4`、`4:5`、`16:9`、`9:16`、`2:1`、`1:2`、`21:9`、`9:21`。
- 兼容旧值 `square`、`landscape`、`portrait`，分别标准化为 `1:1`、`3:2`、`2:3`。
- 上游 `quality: low/medium/high` 和现有积分价格保持不变。
- 只修改开发环境配置与开发示例配置，不修改生产环境配置。
- 不新增数据库列，不执行真实付费图片生成。
- 遵循用户要求：不执行 `git commit`、`git push` 或提交代码；每个任务以测试和 diff 检查作为审阅点。

---

## File Structure

### Backend

- Create `server/artforge-api/src/utils/image_dimensions.js`: 比例标准化、尺寸计算和旧任务快照回退。
- Create `server/artforge-api/src/utils/generation_request.js`: 构造服务端权威任务请求快照。
- Create `server/artforge-api/src/logics/generation_image_request.js`: 从任务快照构造 Worker 的图片模型调用参数。
- Create `server/artforge-api/test/image_dimensions.test.js`: 尺寸算法、比例白名单和旧别名测试。
- Create `server/artforge-api/test/generation_request.test.js`: 任务快照测试。
- Create `server/artforge-api/test/generation_image_request.test.js`: Worker 图片请求参数测试。
- Modify `server/artforge-api/src/routers/v1/generation/filter.js`: 接受精确比例和旧别名。
- Modify `server/artforge-api/src/logics/generation_tasks.js`: 标准化幂等身份并固化目标尺寸快照。
- Modify `server/artforge-api/src/services/openai.js`: 直接传递权威 `size`。
- Modify `server/artforge-api/src/logics/generation_execution.js`: 使用快照尺寸调用中转站并处理输出。
- Modify `server/artforge-api/test/openai.test.js`: 断言 generations 与 edits 的精确 `size`。
- Modify `server/artforge-api/configs/dev.local.yaml`: 提升开发目录版本和图像模型版本，发布完整比例能力。
- Modify `server/artforge-api/configs/dev.example.yaml`: 与开发配置保持一致。

### Client

- Modify `ArtForgeStudio/native-client/src/runtime/configuration.rs`: 增加 API 比例标准化与后端比例恢复函数。
- Modify `ArtForgeStudio/native-client/src/runtime/generation/backend.rs`: 普通提交、恢复提交与服务端任务恢复均保留精确比例。
- Modify `ArtForgeStudio/native-client/src/runtime/tests.rs`: 客户端比例协议单元测试。
- Modify `ArtForgeStudio/native-client/src/runtime/api/cross_stack_tests.rs`: 更新比例参数矩阵与快照断言。

---

### Task 1: Backend authoritative dimension utility and validation

**Files:**
- Create: `server/artforge-api/src/utils/image_dimensions.js`
- Create: `server/artforge-api/test/image_dimensions.test.js`
- Modify: `server/artforge-api/src/routers/v1/generation/filter.js`

**Interfaces:**
- Produces: `SUPPORTED_ASPECT_RATIOS: string[]`
- Produces: `normalizeAspectRatio(value: string): string | null`
- Produces: `targetImageDimensions(aspectRatio: string, maxLongEdge: number): { aspectRatio, width, height, size }`
- Produces: `dimensionsFromSnapshot(snapshot: object, maxLongEdge: number): { aspectRatio, width, height, size }`

- [ ] **Step 1: Write failing dimension tests**

Create `test/image_dimensions.test.js` with executable cases:

```js
const assert = require('node:assert/strict');
const test = require('node:test');
const {
  SUPPORTED_ASPECT_RATIOS,
  dimensionsFromSnapshot,
  normalizeAspectRatio,
  targetImageDimensions,
} = require('../src/utils/image_dimensions');

test('target dimensions preserve exact ratios at the requested longest edge', () => {
  assert.deepEqual(targetImageDimensions('1:1', 1024), {
    aspectRatio: '1:1', width: 1024, height: 1024, size: '1024x1024',
  });
  assert.deepEqual(targetImageDimensions('16:9', 4096), {
    aspectRatio: '16:9', width: 4096, height: 2304, size: '4096x2304',
  });
  assert.deepEqual(targetImageDimensions('9:16', 4096), {
    aspectRatio: '9:16', width: 2304, height: 4096, size: '2304x4096',
  });
  assert.deepEqual(targetImageDimensions('21:9', 4096), {
    aspectRatio: '21:9', width: 4096, height: 1752, size: '4096x1752',
  });
});

test('all supported exact ratios produce step-aligned positive dimensions', () => {
  const exact = SUPPORTED_ASPECT_RATIOS.filter((value) => value.includes(':'));
  assert.equal(exact.length, 13);
  for (const ratio of exact) {
    for (const edge of [1024, 2048, 4096]) {
      const result = targetImageDimensions(ratio, edge);
      assert.equal(Math.max(result.width, result.height), edge);
      assert.equal(result.width % 8, 0);
      assert.equal(result.height % 8, 0);
    }
  }
});

test('legacy aliases normalize and old snapshots retain a deterministic fallback', () => {
  assert.equal(normalizeAspectRatio('square'), '1:1');
  assert.equal(normalizeAspectRatio('landscape'), '3:2');
  assert.equal(normalizeAspectRatio('portrait'), '2:3');
  assert.deepEqual(dimensionsFromSnapshot({ aspect_ratio: 'portrait' }, 2048), {
    aspectRatio: '2:3', width: 1368, height: 2048, size: '1368x2048',
  });
  assert.deepEqual(dimensionsFromSnapshot({
    aspect_ratio: '16:9', target_width: 4096, target_height: 2304,
    provider_size: '4096x2304',
  }, 1024), {
    aspectRatio: '16:9', width: 4096, height: 2304, size: '4096x2304',
  });
});

test('unknown ratios and invalid longest edges are rejected', () => {
  assert.throws(() => targetImageDimensions('7:5', 1024), /Unsupported aspect ratio/);
  assert.throws(() => targetImageDimensions('16:9', 0), /Invalid max long edge/);
});
```

- [ ] **Step 2: Run the new test and verify RED**

Run from `server/artforge-api`:

```bash
node --test test/image_dimensions.test.js
```

Expected: FAIL with `Cannot find module '../src/utils/image_dimensions'`.

- [ ] **Step 3: Implement the minimal pure dimension utility**

Create `src/utils/image_dimensions.js`:

```js
const EXACT_ASPECT_RATIOS = Object.freeze({
  '1:1': [1, 1], '3:2': [3, 2], '2:3': [2, 3], '4:3': [4, 3],
  '3:4': [3, 4], '5:4': [5, 4], '4:5': [4, 5], '16:9': [16, 9],
  '9:16': [9, 16], '2:1': [2, 1], '1:2': [1, 2], '21:9': [21, 9],
  '9:21': [9, 21],
});
const LEGACY_ASPECT_RATIOS = Object.freeze({
  square: '1:1', landscape: '3:2', portrait: '2:3',
});
const SUPPORTED_ASPECT_RATIOS = Object.freeze([
  ...Object.keys(EXACT_ASPECT_RATIOS), ...Object.keys(LEGACY_ASPECT_RATIOS),
]);

function normalizeAspectRatio(value) {
  if (EXACT_ASPECT_RATIOS[value]) return value;
  return LEGACY_ASPECT_RATIOS[value] || null;
}

function roundDimension(value) {
  return Math.max(64, Math.round(Math.max(64, value) / 8) * 8);
}

function targetImageDimensions(aspectRatio, maxLongEdge) {
  const normalized = normalizeAspectRatio(aspectRatio);
  if (!normalized) throw new TypeError(`Unsupported aspect ratio: ${aspectRatio}`);
  if (!Number.isInteger(maxLongEdge) || maxLongEdge <= 0) {
    throw new TypeError(`Invalid max long edge: ${maxLongEdge}`);
  }
  const [ratioWidth, ratioHeight] = EXACT_ASPECT_RATIOS[normalized];
  const width = ratioWidth >= ratioHeight
    ? maxLongEdge : roundDimension((maxLongEdge * ratioWidth) / ratioHeight);
  const height = ratioWidth >= ratioHeight
    ? roundDimension((maxLongEdge * ratioHeight) / ratioWidth) : maxLongEdge;
  return { aspectRatio: normalized, width, height, size: `${width}x${height}` };
}

function dimensionsFromSnapshot(snapshot, maxLongEdge) {
  const width = Number(snapshot?.target_width);
  const height = Number(snapshot?.target_height);
  const aspectRatio = normalizeAspectRatio(snapshot?.aspect_ratio);
  if (aspectRatio && Number.isInteger(width) && width > 0
    && Number.isInteger(height) && height > 0) {
    return { aspectRatio, width, height, size: `${width}x${height}` };
  }
  return targetImageDimensions(snapshot?.aspect_ratio, maxLongEdge);
}

module.exports = {
  SUPPORTED_ASPECT_RATIOS,
  dimensionsFromSnapshot,
  normalizeAspectRatio,
  targetImageDimensions,
};
```

- [ ] **Step 4: Allow exact ratios in the request filter**

In `src/routers/v1/generation/filter.js`, import the shared list and replace the fixed three-value validation:

```js
const { SUPPORTED_ASPECT_RATIOS } = require('../../../utils/image_dimensions');

aspect_ratio: Joi.when('task_type', {
  is: 'image_generation',
  then: Joi.string().valid(...SUPPORTED_ASPECT_RATIOS).default('1:1'),
  otherwise: Joi.forbidden(),
}),
```

- [ ] **Step 5: Run tests and lint for GREEN**

```bash
node --test test/image_dimensions.test.js
npx eslint src/utils/image_dimensions.js src/routers/v1/generation/filter.js test/image_dimensions.test.js
```

Expected: both commands exit 0; four dimension tests pass.

- [ ] **Step 6: Review checkpoint without commit**

```bash
git diff -- src/utils/image_dimensions.js src/routers/v1/generation/filter.js test/image_dimensions.test.js
git status --short
```

Expected: only the planned files plus pre-existing user changes are shown; do not stage or commit.

---

### Task 2: Persist authoritative dimensions in generation snapshots

**Files:**
- Create: `server/artforge-api/src/utils/generation_request.js`
- Create: `server/artforge-api/test/generation_request.test.js`
- Modify: `server/artforge-api/src/logics/generation_tasks.js`
- Modify: `server/artforge-api/configs/dev.local.yaml`
- Modify: `server/artforge-api/configs/dev.example.yaml`

**Interfaces:**
- Consumes: `normalizeAspectRatio()` and `targetImageDimensions()` from Task 1.
- Produces: `buildGenerationRequestSnapshot(entries, referenceFileIds, maxLongEdge): object`.

- [ ] **Step 1: Write failing snapshot tests**

Create `test/generation_request.test.js`:

```js
const assert = require('node:assert/strict');
const test = require('node:test');
const { buildGenerationRequestSnapshot } = require('../src/utils/generation_request');

test('image generation snapshots persist exact provider dimensions', () => {
  assert.deepEqual(buildGenerationRequestSnapshot({
    task_type: 'image_generation', aspect_ratio: '16:9', target_language: null,
  }, ['reference-1'], 4096), {
    aspect_ratio: '16:9',
    target_language: null,
    target_width: 4096,
    target_height: 2304,
    provider_size: '4096x2304',
    reference_file_ids: ['reference-1'],
  });
});

test('legacy input is normalized before being persisted', () => {
  const snapshot = buildGenerationRequestSnapshot({
    task_type: 'image_generation', aspect_ratio: 'portrait', target_language: null,
  }, [], 2048);
  assert.equal(snapshot.aspect_ratio, '2:3');
  assert.equal(snapshot.provider_size, '1368x2048');
});

test('prompt processing snapshots do not invent image dimensions', () => {
  assert.deepEqual(buildGenerationRequestSnapshot({
    task_type: 'prompt_translate', target_language: 'English',
  }, [], null), {
    aspect_ratio: null,
    target_language: 'English',
    reference_file_ids: [],
  });
});
```

- [ ] **Step 2: Run the snapshot test and verify RED**

```bash
node --test test/generation_request.test.js
```

Expected: FAIL with `Cannot find module '../src/utils/generation_request'`.

- [ ] **Step 3: Implement snapshot construction**

Create `src/utils/generation_request.js`:

```js
const { targetImageDimensions } = require('./image_dimensions');

function buildGenerationRequestSnapshot(entries, referenceFileIds, maxLongEdge) {
  const snapshot = {
    aspect_ratio: entries.aspect_ratio || null,
    target_language: entries.target_language || null,
    reference_file_ids: [...referenceFileIds],
  };
  if (entries.task_type !== 'image_generation') return snapshot;
  const dimensions = targetImageDimensions(entries.aspect_ratio, maxLongEdge);
  return {
    ...snapshot,
    aspect_ratio: dimensions.aspectRatio,
    target_width: dimensions.width,
    target_height: dimensions.height,
    provider_size: dimensions.size,
  };
}

module.exports = { buildGenerationRequestSnapshot };
```

- [ ] **Step 4: Wire normalized identity and snapshot creation into generation tasks**

In `src/logics/generation_tasks.js`:

```js
const { normalizeAspectRatio } = require('../utils/image_dimensions');
const { buildGenerationRequestSnapshot } = require('../utils/generation_request');
```

Normalize image request identity so retries from an old client remain idempotent:

```js
aspect_ratio: entries.task_type === imageType
  ? normalizeAspectRatio(entries.aspect_ratio)
  : null,
```

Replace the old `requestSnapshot()` call after references are attached:

```js
task.request_snapshot = buildGenerationRequestSnapshot(
  entries,
  references.map((file) => file.public_id),
  Number(price.max_long_edge),
);
```

Remove the superseded local `requestSnapshot()` helper.

- [ ] **Step 5: Version and update the development catalog**

In both `configs/dev.local.yaml` and `configs/dev.example.yaml`:

```yaml
models:
  catalog_version: "2026-07-15.2"
  items:
    - model_code: openai_image
      version: 3
      capabilities:
        aspect_ratios: ["1:1", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5", "16:9", "9:16", "2:1", "1:2", "21:9", "9:21"]
        supports_references: true
```

Keep display name, provider model ID, prices and prompt model version unchanged. Do not edit `prod.local.yaml` or `prod.example.yaml`.

- [ ] **Step 6: Run snapshot and config tests for GREEN**

```bash
node --test test/image_dimensions.test.js test/generation_request.test.js test/config_schema.test.js
npm run config:check
npx eslint src/utils/generation_request.js src/logics/generation_tasks.js test/generation_request.test.js
```

Expected: all tests pass, configuration check exits 0, lint exits 0.

- [ ] **Step 7: Review checkpoint without commit**

```bash
git diff -- src/utils/generation_request.js src/logics/generation_tasks.js configs/dev.local.yaml configs/dev.example.yaml test/generation_request.test.js
```

Expected: normalized identity, immutable target dimensions and development-only catalog version changes are visible.

---

### Task 3: Pass exact size through the OpenAI-compatible adapter

**Files:**
- Modify: `server/artforge-api/test/openai.test.js`
- Modify: `server/artforge-api/src/services/openai.js`

**Interfaces:**
- Consumes: `options.size: string` supplied by Worker.
- Produces: identical `size` in JSON `/images/generations` and multipart `/images/edits` requests.

- [ ] **Step 1: Change adapter tests to require exact size**

Update the first image adapter test to call:

```js
const result = await OpenAI.generateImage({
  modelId: 'image-model',
  prompt: 'draw a forge',
  size: '4096x2304',
  providerOptions: { quality: 'high' },
  referenceImages: [],
  idempotencyKey: 'task-item-1',
});
assert.equal(requestBody.size, '4096x2304');
assert.equal(requestBody.quality, 'high');
```

Add a multipart edit test:

```js
test('OpenAI image edit adapter preserves the authoritative size', async (context) => {
  const originalFetch = global.fetch;
  context.after(() => { global.fetch = originalFetch; });
  let submitted;
  global.fetch = async (url, options) => {
    submitted = options.body;
    return new Response(JSON.stringify({
      data: [{ b64_json: Buffer.from('image').toString('base64') }],
    }), { status: 200, headers: { 'content-type': 'application/json' } });
  };
  await OpenAI.generateImage({
    modelId: 'image-model', prompt: 'edit', size: '2304x4096',
    providerOptions: { quality: 'high' },
    referenceImages: [{
      buffer: Buffer.from('reference'), mimeType: 'image/png', extension: 'png',
    }],
    idempotencyKey: 'task-item-edit',
  });
  assert.equal(submitted.get('size'), '2304x4096');
  assert.equal(submitted.get('quality'), 'high');
});
```

Update the remaining image adapter calls to pass `size: '1024x1024'` instead of `aspectRatio`.

- [ ] **Step 2: Run adapter tests and verify RED**

```bash
node --test test/openai.test.js
```

Expected: FAIL because the adapter still derives a fixed size from the removed `aspectRatio` input.

- [ ] **Step 3: Use the authoritative size without recomputing it**

In `src/services/openai.js`, remove `imageSize()` and change the common payload:

```js
const common = {
  model: options.modelId,
  prompt: options.prompt,
  n: 1,
  size: options.size,
  quality: providerOptions.quality,
};
```

No other request fields or provider error handling change.

- [ ] **Step 4: Run adapter tests and lint for GREEN**

```bash
node --test test/openai.test.js
npx eslint src/services/openai.js test/openai.test.js
```

Expected: all OpenAI adapter tests pass and lint exits 0.

- [ ] **Step 5: Review checkpoint without commit**

```bash
git diff -- src/services/openai.js test/openai.test.js
```

Expected: fixed `imageSize()` mapping is gone; exact size is preserved for both API endpoints.

---

### Task 4: Wire snapshot dimensions into Worker execution and output processing

**Files:**
- Create: `server/artforge-api/src/logics/generation_image_request.js`
- Create: `server/artforge-api/test/generation_image_request.test.js`
- Modify: `server/artforge-api/src/logics/generation_execution.js`

**Interfaces:**
- Consumes: `dimensionsFromSnapshot()` from Task 1.
- Produces: `buildGenerationImageRequest(args): { dimensions, options }`.
- Passes: `options.size` to `OpenAI.generateImage()` and `dimensions.width/height` to `prepareGenerated()`.

- [ ] **Step 1: Write failing Worker request tests**

Create `test/generation_image_request.test.js`:

```js
const assert = require('node:assert/strict');
const test = require('node:test');
const { buildGenerationImageRequest } = require('../src/logics/generation_image_request');

function args(requestSnapshot, maxLongEdge = 4096) {
  return {
    task: { public_id: 'task-1', request_snapshot: requestSnapshot,
      pricing_snapshot: { max_long_edge: maxLongEdge } },
    itemIndex: 0,
    modelId: 'gpt-image-2',
    prompt: 'draw',
    providerOptions: { quality: 'high' },
    referenceImages: [],
  };
}

test('Worker image requests use snapshotted provider size', () => {
  const request = buildGenerationImageRequest(args({
    aspect_ratio: '16:9', target_width: 4096, target_height: 2304,
    provider_size: '4096x2304',
  }));
  assert.equal(request.options.size, '4096x2304');
  assert.deepEqual(request.dimensions, {
    aspectRatio: '16:9', width: 4096, height: 2304, size: '4096x2304',
  });
  assert.equal(request.options.idempotencyKey, 'task-1-0');
});

test('Worker image requests derive dimensions for legacy snapshots', () => {
  const request = buildGenerationImageRequest(args({ aspect_ratio: 'landscape' }, 1024));
  assert.equal(request.options.size, '1024x680');
  assert.equal(request.dimensions.aspectRatio, '3:2');
});
```

- [ ] **Step 2: Run the Worker request test and verify RED**

```bash
node --test test/generation_image_request.test.js
```

Expected: FAIL with `Cannot find module '../src/logics/generation_image_request'`.

- [ ] **Step 3: Implement the pure Worker request builder**

Create `src/logics/generation_image_request.js`:

```js
const { dimensionsFromSnapshot } = require('../utils/image_dimensions');

function buildGenerationImageRequest({
  task, itemIndex, modelId, prompt, providerOptions, referenceImages,
}) {
  const dimensions = dimensionsFromSnapshot(
    task.request_snapshot,
    Number(task.pricing_snapshot.max_long_edge),
  );
  return {
    dimensions,
    options: {
      modelId,
      prompt,
      size: dimensions.size,
      providerOptions,
      referenceImages,
      idempotencyKey: `${task.public_id}-${itemIndex}`,
    },
  };
}

module.exports = { buildGenerationImageRequest };
```

- [ ] **Step 4: Use the request builder in generation execution**

In `src/logics/generation_execution.js`:

```js
const { buildGenerationImageRequest } = require('./generation_image_request');
```

Remove the old fixed `outputDimensions()` helper. Change image success recording to accept the already resolved dimensions:

```js
async function recordImageSuccess(task, item, result, startedAt, dimensions) {
  const image = await prepareGenerated(
    result.buffer, result.mimeType, dimensions.width, dimensions.height,
  );
```

Replace the image branch in the item loop:

```js
const request = buildGenerationImageRequest({
  task,
  itemIndex: item.item_index,
  modelId: model.provider_model_id,
  prompt,
  providerOptions: providerOptions(task, model),
  referenceImages: references,
});
const result = await OpenAI.generateImage(request.options);
await recordImageSuccess(task, item, result, startedAt, request.dimensions);
```

- [ ] **Step 5: Run focused Worker tests and lint for GREEN**

```bash
node --test test/image_dimensions.test.js test/generation_image_request.test.js test/openai.test.js
npx eslint src/logics/generation_image_request.js src/logics/generation_execution.js test/generation_image_request.test.js
```

Expected: focused tests and lint pass.

- [ ] **Step 6: Review checkpoint without commit**

```bash
git diff -- src/logics/generation_image_request.js src/logics/generation_execution.js test/generation_image_request.test.js
```

Expected: the same resolved dimensions control the billable provider request and stored output dimensions.

---

### Task 5: Preserve exact ratios in client submission and recovery

**Files:**
- Modify: `ArtForgeStudio/native-client/src/runtime/tests.rs`
- Modify: `ArtForgeStudio/native-client/src/runtime/configuration.rs`
- Modify: `ArtForgeStudio/native-client/src/runtime/generation/backend.rs`

**Interfaces:**
- Produces: `api_aspect_ratio(ratio: &str) -> String`.
- Produces: `client_ratio_from_api(ratio: &str) -> String`.

- [ ] **Step 1: Write failing client ratio protocol tests**

Add to `native-client/src/runtime/tests.rs`:

```rust
#[test]
fn generation_api_preserves_exact_aspect_ratios() {
    for ratio in [
        "1:1", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5",
        "16:9", "9:16", "2:1", "1:2", "21:9", "9:21",
    ] {
        assert_eq!(api_aspect_ratio(ratio), ratio);
        assert_eq!(client_ratio_from_api(ratio), ratio);
    }
    assert_eq!(client_ratio_from_api("square"), "1:1");
    assert_eq!(client_ratio_from_api("landscape"), "3:2");
    assert_eq!(client_ratio_from_api("portrait"), "2:3");
    assert_eq!(api_aspect_ratio("unsupported"), "1:1");
}
```

- [ ] **Step 2: Run the focused client test and verify RED**

Run from `ArtForgeStudio`:

```bash
cargo test -p artforge-studio-native generation_api_preserves_exact_aspect_ratios
```

Expected: compilation FAIL because `api_aspect_ratio` and `client_ratio_from_api` do not exist.

- [ ] **Step 3: Implement client ratio protocol helpers**

Add to `native-client/src/runtime/configuration.rs`:

```rust
pub(super) fn api_aspect_ratio(ratio: &str) -> String {
    supported_ratios()
        .iter()
        .find(|(label, _, _)| *label == ratio)
        .map(|(label, _, _)| (*label).to_string())
        .unwrap_or_else(|| "1:1".to_string())
}

pub(super) fn client_ratio_from_api(ratio: &str) -> String {
    match ratio {
        "square" => "1:1".to_string(),
        "landscape" => "3:2".to_string(),
        "portrait" => "2:3".to_string(),
        value => api_aspect_ratio(value),
    }
}
```

- [ ] **Step 4: Replace the three lossy mappings in generation flow**

In `native-client/src/runtime/generation/backend.rs`:

- For a new task, replace the `square/landscape/portrait` match with `let aspect_ratio = api_aspect_ratio(&ratio);`.
- In `run_recovered_generation_worker`, use `let aspect_ratio = api_aspect_ratio(&record.ratio);`.
- In `recover_server_generation_tasks`, replace the old mapping with:

```rust
let ratio = detail.request.get("aspect_ratio")
    .and_then(Value::as_str)
    .map(client_ratio_from_api)
    .unwrap_or_else(|| "1:1".to_string());
```

Pass `Some(aspect_ratio)` in both create requests without any further conversion.

- [ ] **Step 5: Run client tests and formatting for GREEN**

```bash
cargo test -p artforge-studio-native generation_api_preserves_exact_aspect_ratios
cargo test -p artforge-studio-native quality_pixel_size_uses_longest_edge_limits
cargo fmt --all -- --check
```

Expected: both focused tests pass and formatting check exits 0.

- [ ] **Step 6: Review checkpoint without commit**

```bash
git diff -- native-client/src/runtime/configuration.rs native-client/src/runtime/generation/backend.rs native-client/src/runtime/tests.rs
```

Expected: no new task path collapses exact ratios into orientation names.

---

### Task 6: Cross-stack contract coverage

**Files:**
- Modify: `ArtForgeStudio/native-client/src/runtime/api/cross_stack_tests.rs`

**Interfaces:**
- Consumes: exact ratio request contract and task snapshot fields from Tasks 2 and 5.
- Verifies: client request → Koa validation → generation task detail snapshot.

- [ ] **Step 1: Update cross-stack tests before changing their expectations**

In the invalid field matrix, replace the old invalid `16:9` case with:

```rust
request.aspect_ratio = Some("7:5".to_string());
cases.push((request, "aspect_ratio"));
```

Replace the three-ratio success loop with:

```rust
for ratio in [
    "1:1", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5",
    "16:9", "9:16", "2:1", "1:2", "21:9", "9:21",
    "square", "landscape", "portrait",
] {
    let request = CreateGenerationTask {
        client_request_id: format!("ratio_{}", Uuid::new_v4().simple()),
        task_type: "image_generation".to_string(),
        model_code: "openai_image".to_string(),
        prompt: format!("valid {ratio} image"),
        quality: Some("1K".to_string()),
        count: Some(1),
        aspect_ratio: Some(ratio.to_string()),
        reference_file_ids: Some(Vec::new()),
        target_language: None,
    };
    let task = generation.create_task(&request).expect("valid ratio task");
    let expected = match ratio {
        "square" => "1:1", "landscape" => "3:2", "portrait" => "2:3",
        value => value,
    };
    assert_eq!(task.request["aspect_ratio"], expected);
    assert!(task.request["target_width"].as_u64().unwrap() > 0);
    assert!(task.request["target_height"].as_u64().unwrap() > 0);
    assert!(task.request["provider_size"].as_str().unwrap().contains('x'));
    generation.cancel(&task.id).expect("cancel ratio task");
}
```

- [ ] **Step 2: Start the isolated Mock API for contract verification**

Start the isolated Mock API from `server/artforge-api`:

```bash
ARTFORGE_ENABLE_MOCK_API=1 ARTFORGE_MOCK_PORT=39091 NODE_ENV=dev npm run mock:api
```

- [ ] **Step 3: Run the updated cross-stack generation contract tests**

With the Mock API still running:

```bash
ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
cargo test -p artforge-studio-native cross_stack_generation_parameter_matrix_and_idempotency -- --ignored --nocapture

ARTFORGE_CROSS_STACK_BASE_URL=http://127.0.0.1:39091 \
ARTFORGE_MOCK_EMAIL_CODE=654321 \
cargo test -p artforge-studio-native cross_stack_generation_success_variants_and_credit_reservation_limit -- --ignored --nocapture
```

Expected: both ignored cross-stack tests pass. The production behavior was developed from the focused RED tests in Tasks 1–5; these tests verify the assembled HTTP contract. The Mock API cleans up its test users on shutdown.

- [ ] **Step 4: Review checkpoint without commit**

```bash
git diff -- native-client/src/runtime/api/cross_stack_tests.rs
```

Expected: `16:9` is no longer considered invalid and all exact/legacy ratios are covered.

---

### Task 7: Full verification and local development restart

**Files:**
- Verify all files listed above.
- No additional production files.

**Interfaces:**
- Verifies the complete client → backend → provider payload contract without a real paid provider call.

- [ ] **Step 1: Run the complete backend verification suite**

From `server/artforge-api`:

```bash
npm test
npm run lint
npm run config:check
```

Expected: zero test failures, zero lint errors, valid development and example configuration.

- [ ] **Step 2: Run the complete active client verification suite**

From `ArtForgeStudio`:

```bash
cargo fmt --all -- --check
cargo test -p artforge-studio-native
cargo check -p artforge-studio-native
```

Expected: formatting, all non-ignored tests and compile check pass.

- [ ] **Step 3: Confirm no production configuration or database schema changed**

```bash
git diff -- ../server/artforge-api/configs/prod.local.yaml ../server/artforge-api/configs/prod.example.yaml
git diff -- ../server/artforge-api/database ../server/artforge-api/migrations
```

Expected: no output.

- [ ] **Step 4: Sync the development catalog and restart local services**

From `server/artforge-api`:

```bash
NODE_ENV=dev npm run catalog:sync
npm run dev
```

Expected: catalog sync succeeds without `model_catalog_version_conflict`; API and Worker start with the new development model version.

From `ArtForgeStudio`:

```bash
cargo run -p artforge-studio-native --bin ArtForgeStudio
```

Expected: the client starts and loads the model catalog. Do not trigger a real image generation during automated verification.

- [ ] **Step 5: Inspect final diffs without staging or committing**

From both repositories:

```bash
git status --short
git diff --check
```

Expected: no whitespace errors; only planned changes and the user's pre-existing worktree changes remain. Do not run `git add`, `git commit`, or `git push`.
