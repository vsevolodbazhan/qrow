---
name: qrow-ui-conventions
description: Qrow's own UI decisions, consistency rules, known UI bug classes, and the verification steps for UI changes. Read before designing, changing, or reviewing any Qrow screen, toolbar, tab strip, grid, tooltip, empty state, or default (fonts, alignment, density), and before acting on a screenshot note from the owner. Use together with gpui-kit-design-guides; where they conflict, this skill wins for Qrow.
---

# Qrow UI Conventions

These rules come from the owner's decisions and from bugs that came back more
than once. They add Qrow-specific choices to the GPUI Kit Design Guides
(`gpui-kit-design-guides`). When the two disagree, follow this skill.

## Owner decisions

Do not propose changes that undo these. Ask the owner first if a task seems to
need it.

- **Roomy, not dense.** Keep result grid rows tall. Do not reduce row height
  or padding to fit more data.
- **Minimal empty states.** A short title and one line of description. No
  suggested prompts, examples, or filler.
- **Say it once.** Show a fact in one place. The tab's connection lives in the
  status bar, so it does not repeat next to **Run**.
- **Icons, not a second row.** When controls do not fit, collapse them to
  icon-only buttons with a tooltip that names the control and its current
  value. Never wrap a toolbar or composer onto a second row.
- **Default fonts.** IBM Plex Sans for the interface and assistant
  conversations. JetBrains Mono, with ligatures, for the editor, Logs, and code.
- **Grid alignment.** All column headers are left-aligned. Only numeric cell
  values are right-aligned, with tabular digits.
- **Selected tab.** The selected query tab grows to show its full title.

## Consistency rules

Peer surfaces must look and behave the same.

- **Toolbars.** The Logs, Result, and Activity toolbars have the same height.
  Counts are tags (chips). Actions are icon buttons with tooltips.
- **Cut text.** Text cut with an ellipsis shows its full text in a tooltip on
  hover. Use the same tooltip, delay, and "only when cut" check everywhere, and
  measure in the font weight that is drawn. Tooltips that also show state (for
  example a tab's connection status) show on every item, including the
  selected one.
- **Cut content at a scroll edge.** Fade the content into the background of
  each row (stripes, hover, and selection included), not into one flat color.
  Show an edge cue on each side that has hidden content, and only there.
  Lines and borders stay unbroken under the fade.
- **Selection stays visible.** After any change that is not a manual scroll
  (resize, panel toggle, rename, add, close, select), scroll the selected item
  fully into view.
- **Backgrounds reach the edge.** Row backgrounds, tints, and separators run
  the full width of their container. Only content keeps clear of a scrollbar.

## Known UI bug classes

Check each one when you change a layout, a strip, a grid, or a tooltip.

1. **Empty bands from reserved space.** Space reserved for a scrollbar, for
   snapping, or for a cover shows as an empty band when nothing fills it.
   Reserve space only when it is in use.
2. **Orphan controls.** A cut item must not leave a lone control (a close
   button or a status dot) or a blank block. Hide its controls, and pin its
   label to the visible edge.
3. **Stale layout after a size change.** Anything that measures its bounds
   (visible columns, fit checks, badges) must recompute and redraw when the
   size changes, not only on hover.
4. **Stuck tooltips.** Tooltips hide on mouse-out, scroll, resize, key press,
   and when their owner is hidden or removed.
5. **Scroll past the end.** A scroll area stops at its last content. Do not add
   a spacer so the last section can reach the top.
6. **Different path for the active item.** The selected or active state often
   renders through a separate branch. Test the selected item for every hover,
   tooltip, and focus behavior.
7. **Fragments at edges.** No title fragment or partial control at a scroll
   edge. Snap to item boundaries, or fade the partial item.

## Verify a UI change

Follow [Check the native UI](../../../docs/development.md#check-the-native-ui)
in the development docs. In addition:

- Check in the release demo, in the dark and the light theme, at 1280 and 900
  points wide, with the Assistant open and closed.
- For strips and grids, sweep at least five widths, and check the start, the
  middle, and the end of the scroll range.
- Capture the states a static screenshot misses: hover, keyboard focus,
  selection, validation errors, and long non-Latin text (for example
  Cyrillic titles).
- Write a UI test that fails without the fix for each UI regression. Painted
  pixels cannot be read in tests, so check colors and fades on screen and say
  so in the report.
- Before you call a test failure unrelated, compare with the CI history of
  `main`.

## Act on a screenshot note

- Change only what the note names. Plural words in the note cover the whole
  group, not only the item the arrow points to.
- Keep every earlier owner decision intact. Check the list above first.
- Record each change to vendored GPUI Kit code in its `QROW-PATCH.md`.
