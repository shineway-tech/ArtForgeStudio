# Design QA

- source visual truth: `C:/Users/deyx1/AppData/Local/Temp/codex-clipboard-bb78ba2e-46e2-401b-88fd-cd296a64957a.png`
- implementation screenshot: `C:/Users/deyx1/AppData/Local/Temp/artforge-usage-tip.png`
- combined comparison: `C:/Users/deyx1/AppData/Local/Temp/artforge-usage-tip-comparison.png`
- source pixels: 755 × 93
- implementation pixels: 2582 × 1550 at 144 DPI
- implementation viewport: maximized Windows desktop window
- density normalization: the bottom generation-control region was cropped from the 144-DPI native capture and placed beside the source at comparable scale
- state: dark theme, character workspace, idle generation state, second usage tip visible

## Findings

- No actionable P0, P1, or P2 findings remain.
- The requested usage-tip line occupies the previously empty area below the generation button without moving or covering persistent controls.

## Required fidelity surfaces

- Fonts and typography: passed. The tip uses 12 px regular-weight text and remains subordinate to the generation button.
- Spacing and layout rhythm: passed. The tip is centered in the existing status slot with clear separation from the button.
- Colors and visual tokens: passed. The text uses the existing weak foreground token and preserves dark-theme contrast.
- Image quality and assets: not applicable; the feature introduces no image assets.
- Copy and content: passed. Both `/` recent-history and `//` custom-prompt instructions are included, with localized English equivalents.

## Comparison evidence

- Full view: the complete native application shows the tip inside the work panel and no overflow at the bottom edge.
- Focused region: the combined comparison shows the former empty region on the left and the added tip below the unchanged generation button on the right.
- Interaction: the 4.2-second timer rotates two vertically animated lines; active generation, optimization, translation, or status text hides the tips.

## Comparison history

1. Initial state: the area below the generation button was empty while idle.
2. Fix: added a clipped vertical carousel that reuses the status-text slot.
3. Post-fix evidence: the second tip is visible below the button; source-level tests verify both messages and both slide animations.

final result: passed
