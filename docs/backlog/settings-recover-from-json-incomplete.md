# recover_settings_from_json silently drops fields

- **Severity:** Low (data safety)
- **Type:** bug / maintainability
- **Area:** `okena-workspace`
- **Location:** `crates/okena-workspace/src/settings.rs:513-608`

## Problem

`recover_settings_from_json` hand-recovers ~17 of ~30 `AppSettings` fields
field-by-field. But every field already has `#[serde(default)]`, so `from_str`
already tolerates missing/unknown fields — the fallback only helps the rare
"one field has a wrong type" case, silently drops the dozen fields it doesn't list,
and forces every new setting to be added in two places.

## Suggested fix

Replace with a per-field-tolerant deserialize (or `serde_json::Value` → fill
defaults generically) so it can't silently lose newly-added settings.
