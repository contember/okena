# ui/ — Shared UI Utilities

Design tokens, reusable selection state, and click detection utilities used across views.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | Module re-exports. |
| `tokens.rs` | Design tokens — constants for spacing, text sizes, border radii, icon sizes. Central source of truth for UI dimensions. |
| `selection.rs` | `SelectionState<P>` — generic selection tracking with `Selectable` trait. Used for list selection in overlays. |
| `click_detector.rs` | `ClickDetector<K>` — generic click detector with 400ms double-click threshold. Distinguishes single vs double clicks. |

## Key Patterns

- **Design tokens**: All magic numbers for UI dimensions are centralized in `tokens.rs`. Views import these instead of using inline constants.
- **Generic utilities**: Both `SelectionState<P>` and `ClickDetector<K>` are generic over their key type, making them reusable across different list/item contexts.
