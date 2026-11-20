# T25 — CLI & Diagnostics Virtual-Cable Completeness

| Field | Value |
| :--- | :--- |
| Status | done (`cli-cable-ux`) |
| Lane | Z — Post-Integration UX |
| Owns | `crates/tine-app/src/cli.rs`, `crates/tine-app/src/diag.rs`, `docs/tasks/done/T25-cli-cable-ux.md` |

## Goal
Ensure `tine --help` documents every supported CLI subcommand (`mute` and `ptt`) and that `tine diag` reports the virtual cable's paired application microphone target (`app_microphone_target`) alongside the user's `denoise_engine` and `park_when_unused` (PTT Keep Warm) settings.

## Owned Files
- `crates/tine-app/src/cli.rs`
- `crates/tine-app/src/diag.rs`
- `docs/tasks/done/T25-cli-cable-ux.md`

## Changes Made
1. **`crates/tine-app/src/cli.rs`**:
   - Added `tine mute [on|off|toggle]` and `tine ptt on|off` usage lines to `usage()`.
   - Added unit test `cli::tests::usage_lists_all_cli_subcommands` verifying `usage()` lists all 14 CLI subcommands (`tine status`, `tine diag`, `tine set`, `tine macro`, `tine archetype`, `tine bypass`, `tine mute`, `tine denoise`, `tine ptt`, `tine monitor`, `tine calibrate`, `tine meters`, `tine apo`, `tine soak`).
2. **`crates/tine-app/src/diag.rs`**:
   - Enriched `virtual_cable_sink` in `collect()` when `find_virtual_cable_sink()` returns `Some(sink)` to include `"app_microphone_target": tine_engine::listeners::capture_name_for_sink(&sink.name)` alongside `"name"` and `"available"`.
   - Added `"denoise_engine": settings.denoise_engine.as_str()` and `"park_when_unused": settings.park_when_unused` to `hash_endpoint_ids()`.
   - Added unit test `diag::tests::hash_endpoint_ids_includes_denoise_and_ptt` verifying both keys are emitted with the expected values.

## Verification
Commands executed:
- `cargo fmt -p tine-app` — exited with code 0.
- `cargo clippy -p tine-app --all-targets -- -D warnings` — exited with code 0 (no warnings/errors).
- `cargo nextest run -p tine-app --lib` — exited with code 0: `65 tests run: 65 passed, 1 skipped` (including `cli::tests::usage_lists_all_cli_subcommands` and `diag::tests::hash_endpoint_ids_includes_denoise_and_ptt`).
