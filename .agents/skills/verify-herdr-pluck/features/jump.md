# Jump to a Herdr pane or tab

The user invokes **Pluck visible token** on a pane that shows Herdr ids, such as `w1:p1` (pane) or
`w1:t1` (tab). A lowercase hint copies the id. An uppercase hint focuses that pane or tab, closes
the picker tab, and does not run the `[open]` command. If the id is stale, focus returns to the
source tab.

## Sub-features

- `jump-pane` focuses the pane with the selected pane id.
- `jump-tab` focuses the tab with the selected tab id.
- `jump-no-open` neither copies the id nor runs the open command.

## How to get to it (user POV)

- A keybinding bound to `rmarganti.herdr-pluck.pluck` or `rmarganti.herdr-pluck.pluck-all`, then
  an uppercase hint on a Herdr id.

## Driving it with pluck-verify

Preconditions:

- Baseline from the [index](./README.md), on a fresh `$V up`. The first pane is `w1:p1` in tab
  `w1:t1`, and `fixtures/herdr-ids.txt` names both.

- **Split and seed.** Run `$V split herdr-ids.txt`. It prints the new right-hand pane id, which
  becomes the source pane.
- **Hint the ids.** Run `$V invoke pluck`. `picker.txt` shows `a1:p1` and `s1:t1`.
- **Jump to a tab.** Run `$V select S`. `clipboard: <untouched>`, `opened: <none>`, and `focused:`
  is the right-hand pane, because it is the focused pane of tab `w1:t1`.
- **Jump to a pane.** Run `$V invoke pluck` and `$V select A`. `clipboard: <untouched>`,
  `opened: <none>`, and `focused: w1:p1`, the left-hand pane. `$V panes` lists no `Herdr Pluck`
  tab.

## Gotchas

- Run the pane jump last. Focus leaves the recorded source pane, so the next `invoke` stops and
  doctor reports `FAIL source pane ... is focused`. Run `$V down` and `$V up --no-build` to drive
  again.
- The fixture ids are fixed. A run that creates panes or tabs before this journey still works,
  because `w1:p1` and `w1:t1` exist from `up`.
- The helper has no command to create a second tab, so a jump to a different tab or workspace is
  not driven here. The tab and workspace switch is Herdr's `pane.focus` behavior.
