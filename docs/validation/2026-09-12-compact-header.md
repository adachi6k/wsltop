# Compact-header validation — 2026-09-12

Retained from the Issue #29 implementation record on
`feat/compact-resource-summary`. This is historical evidence from implementation
and review, not a current specification or a claim about later builds.
Current behavior is described in the [README](../../README.md#interactive-tui).

## Automated coverage recorded

- `cargo test --locked --all-targets`: 160 tests passed at the recorded stage.
- Clippy with warnings denied, Windows GNU target checking, locked release build
  and diff whitespace checks passed.
- CPU/RAM history events use the same snapshot capture time. Regression tests
  cover queue delays, slot boundaries and failures without sleeps.
- History tests cover chronological ordering, 0/100%, missing versus failed
  observations, held values, shared-clock advancement and bounded retention.
  Other collector updates, redraws and filter changes do not append samples.
- Rendering tests at 40/79/80/119/120/160 columns cover summary/table/footer layout,
  environment colors and required footer fields. At four or more rows, two summary
  lines and one footer line remain available.
- CPU/RAM graph and environment columns stay aligned across N/A, digit-count and
  memory-unit changes. Flat/tree resource output was compared with the existing
  renderer to preserve columns and container grouping.
- Help scrolling uses Ratatui's actual wrapped line count. Tests at 19/40/80/120
  columns include long status text, Japanese and page navigation to the end.
- Classic-header and long-tree-row regressions cover restored classic output and
  exclusion of resource rows from separator-width calculations.
- Summary and table separators share Dim styling without changing numeric,
  history, environment, heading or resource styles; monochrome has no decoration.
  Separator width is bounded by the terminal and summary/table widths, independent
  of command length and scrolling. Tested at 0/60/80/120/240 columns and startup.
- A Windows command-test startup allowance was increased from 5 to 30 seconds;
  product timeouts and dedicated timeout tests were retained.

## Real-host checks recorded

- WSL TUI collected Windows host CPU/physical RAM and Windows/WSL observations.
  Docker/WSLC were empty in that run; it did not validate loaded containers.
- At 120/80/60 columns, checked alignment, spacing, separators, footer shortening,
  narrow-width history hiding and `stty -g` equality before/after `q` exit.
- Checked tree/memory/ascending toggles, classic and WSL-only views, help open/close
  and `q` exit. Earlier captures used intermediate 20/12-point history layouts;
  those were not the final 23/15-point layout.
- Approximately 20 seconds of observation after the cadence fix showed CPU
  updates around every three seconds, held history without artificial gaps and
  stable column positions.
- With NO_COLOR set, checked auto/never monochrome and explicit always-color
  output, including terminal ANSI attributes.
- The Windows RAM P/Invoke path was checked on Windows 11 Pro and WSL2;
  see the [dedicated measurement record](2026-09-12-summary-memory.md).

## Limits of this record

Temporary SVG previews were not committed and are not required to repeat checks.
At this stage Windows executable generation was blocked by missing
`x86_64-w64-mingw32-dlltool`; target checking passed, but native execution/MSVC,
loaded-container validation and both light/dark themes still needed separate
checks. See later [v0.5.0 validation](2026-09-12-v0.5.0.md) and the
[live demo capture](../assets/README.md) for subsequent evidence.
