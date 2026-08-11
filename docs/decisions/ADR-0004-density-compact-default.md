# ADR-0004 — Density default is compact; tokens own the row/control stops

- **Status:** accepted
- **Date:** 2026-08-07
- **Supersedes:** the cozy default and the comfortable density block (both dead code)

## Context

The design token file (`tokens.css`) defined only 32/36 px row stops but the shell shipped a
40 px stop, a `data-density="comfortable"` block nothing ever set, an inline `--row-h`
bypassing the tokens, and a `--fs-2xs` token that didn't exist (a typo hidden by a fallback).
DESIGN-GUIDELINES §4 says the default is **compact**.

## Decision

- Default density = `compact` (32 px rows); `cozy`/`large` remain as opt-ins.
- `--row-h` / `--control-h` map from `data-density` in tokens.css; the inline bypass and the
  dead `comfortable` block were removed; `--fs-2xs` → `--fs-xs`.

## Consequences

- The grid fits more rows by default, per spec; the executable token file and the shell agree
  again (verified by the visual-conformance lane of the board).
