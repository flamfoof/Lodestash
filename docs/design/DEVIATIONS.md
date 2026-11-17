# Deliberate deviations from the prototype

> Created 2026-11-16 (ADR-0023). Empty until a screen is rebuilt against the contract.

The app must match [`Lodestash Prototype.dc.html`](Lodestash%20Prototype.dc.html) as measured by
the design-contract gate ([ADR-0023](../decisions/ADR-0023-one-visual-authority-and-a-contract-gate.md)).
A difference is allowed only if it has a row here. The gate reads this table: an anchor listed
here is exempt for the properties named in its row, and nothing else is.

Add a row when behaviour owned by `FEATURES.md` / `UI-SPEC.md` requires the app to draw something
the prototype does not, or when the owner decides against the prototype. Remove the row when the
prototype is updated to agree.

| Screen / state | Anchor | Properties | Prototype | App | Reason | Decided |
|---|---|---|---|---|---|---|
