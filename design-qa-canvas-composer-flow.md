**Comparison Target**

- Source visual truth: `C:\Users\deyx1\AppData\Local\Temp\codex-clipboard-6fc19df5-7959-4141-852b-59664a97c272.png`.
- Rendered implementation: `E:\Elunvi Canvas\artifacts\canvas-composer-flow\implementation-final-expanded.jpg`.
- Focused comparison evidence: `E:\Elunvi Canvas\artifacts\canvas-composer-flow\comparison-inline-reference-passed.png`.
- State: packaged Windows client, plant-generator canvas, composer expanded, one reference image loaded, inline `图片1` mention followed by the saved `水稻` prompt.

**Viewport and Normalization**

- Source pixels: `127 x 127`.
- Implementation capture: `1443 x 931` window-relative logical pixels; the saved JPEG has the same pixel dimensions.
- Focus crop: `700 x 180` from the implementation, scaled to `780 x 201` and vertically padded to `780 x 255`.
- Source was scaled to `255 x 255` and horizontally padded to `780 x 255`.
- The normalized panels were combined side by side into `1560 x 255`; no browser or device frame was included.

**Findings**

- No actionable P0, P1, or P2 differences remain in the requested reference-image input region.
- [P3] The production composer includes its existing model, size, enhancement, clear, and credit controls beyond the small reference crop. These are intentional product controls and do not change the requested upload order or inline-reference behavior.

**Required Fidelity Surfaces**

- Fonts and typography: `图片1`, `图片`, the saved prompt, and the numbered badge remain legible at the production client's regular-weight type scale. The source and implementation use slightly different raster antialiasing, but hierarchy and optical weight are equivalent.
- Spacing and layout rhythm: the add button is first, the numbered thumbnail follows, and the inline reference mention begins at the prompt's left edge before user text. Button size, gap, badge placement, and dark composer spacing preserve the reference hierarchy.
- Colors and visual tokens: dark surfaces, muted labels, white number badge, and purple canvas border use existing application tokens while retaining the source's contrast and grouping.
- Image quality and asset fidelity: the real uploaded rice reference is visible both in the mention token and thumbnail. No placeholder, fake image, or generated substitute is used.
- Copy and content: the visible labels are `图片`, `图片1`, and the user's saved `水稻` prompt; no implementation instructions leak into the interface.
- States and interactions: entering the plant canvas, asynchronous thumbnail loading, expanded-to-collapsed transition, collapsed-to-expanded transition, down chevron while expanded, and up chevron while collapsed were exercised in the packaged client.
- Accessibility and resilience: the clickable add and thumbnail targets remain 42 px square, content is not clipped at the captured desktop viewport, and the prompt retains keyboard focus logic after a new reference is added. Keyboard-only traversal was not separately captured.

**Comparison History**

- Iteration 1: [P2] the inline mention was not visibly anchored at the prompt start. Fixed by explicitly pinning the mention container to `x: 0px; y: 0px`, then recaptured.
- Iteration 2: [P1] the persisted canvas reference produced an empty thumbnail because the asynchronous preview callback only accepted the ordinary generation page. Fixed by accepting both `generation` and `canvas`, covered by a regression test, then recaptured.
- Final evidence: `comparison-inline-reference-passed.png` shows the real reference image, inline `图片1` token, add-first ordering, and numbered thumbnail with no remaining P0/P1/P2 mismatch.

**Implementation Checklist**

- [x] Reference token rendered before prompt text.
- [x] Prompt input focus/caret is moved to the end after a new reference is added.
- [x] Add button appears before uploaded thumbnails.
- [x] Uploaded thumbnail and number badge are visible.
- [x] Remove badge is hidden until hover.
- [x] Canvas-page asynchronous reference previews render.
- [x] Expanded and collapsed chevrons point in the correct directions.
- [x] Generation placeholder replaces itself in place without opening the media editor automatically.
- [x] Plant generator defaults to natural soil unless the user explicitly requests a pot or container.

**Residual Test Gaps**

- The paid external image-generation request was not triggered during visual QA. Loading-placeholder creation, in-place replacement, error cleanup, and result deselection are covered by automated tests instead.

final result: passed
