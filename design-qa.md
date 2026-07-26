# Design QA

- source: `C:/Users/deyx1/AppData/Local/Temp/codex-clipboard-70828aa8-dbd6-4fb5-aeb6-c0353a53c282.png`
- implementation: `C:/Users/deyx1/AppData/Local/Temp/artforge-custom-prompt-page-full.png`
- source dimensions: 1557 × 1098
- implementation capture: 2582 × 1550 at 144 DPI
- viewport/state: maximized Windows desktop window, dark theme, empty “创建提示词” form

## Comparison

- Full view: passed. The editor is a routed page rather than a modal overlay and keeps the application navigation visible.
- Layout: passed. The left column contains name, category, format, and reference image controls; the right column contains the primary prompt editor and negative prompt editor.
- Hierarchy: passed. The title and close action are visually separated from the two-column body, while cancel/save remain grouped at the bottom-right.
- Spacing and alignment: passed. Panels share consistent margins, borders, radii, label spacing, and aligned field edges.
- Responsive behavior: passed for the maximized production viewport. Width allocation keeps the metadata column compact and gives the content editor the larger share.
- Focused regions: passed for the reference-image controls, format selector, large prompt input, negative prompt input, and footer actions.

## Iteration history

1. Replaced the modal dialog with a dedicated editor page route.
2. Reorganized the form into left metadata/reference and right content columns.
3. Captured the native Slint window at the monitor DPI and verified the complete page, including the footer.

final result: passed
