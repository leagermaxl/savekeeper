# LLM eval results (SPEC-08 §6.3)

## The set

- `*.json` here: one file per expected category, each a JSON array of cases
  `{ id, expected_category, acceptable_categories, expected_app?, must_not?, trap?, hint?, summary }`;
  `summary` is a `FolderSummary` (SPEC-02 §4) as `sk-scan` produces it after `redact`.
- Written by hand from real folder layouts of known programs and games. Names, paths, sizes and
  dates are synthetic; no data of a real user. Long ids and `@` names are already in redacted form
  (`<redacted>`), as the scanner would send them.
- `acceptable_categories` lists other categories that also count as correct (the expected one is
  implied). `must_not` lists serious mistakes (e.g. a save classified as `cache`). `trap` marks
  hard cases: games in `Documents`, Electron apps, large caches with `.json`, Unreal `Saved`
  folders with logs/web cache, install folders with configs, etc.
- 150 cases: game_save 15, game_config 14, app_config 16, app_data 15, user_files 14,
  dev_environment 14, credentials 14, system_settings 13, reinstallable 16, cache 16, unknown 3;
  42 traps. Structure is checked by `cargo test -p sk-llm --test eval_dataset` (no network).

Run: `cargo run -p sk-llm --example eval -- --provider ollama --model <model> [--verbose]`.
The example calls the classifier directly (no cache), in batches like `ClassifyRunner`
(`max_batch`, ≤ 12 000 input chars), in a fixed order that mixes categories; ids sent to the model
are opaque (`item-NNN`). Every request passes `final_check` against the current machine first.
"Accuracy by acceptable" counts a prediction as correct if it is the expected or an acceptable
category; missing results and failed requests count as wrong.

## 2026-10-19 — qwen3.5:9b, prompt v1 (first run)

| | |
|---|---|
| Date | 2026-10-19 (local; the example prints the UTC date 2026-10-18) |
| Provider / model | Ollama 0.40.6, `qwen3.5:9b` (Q4_K_M, 9.0B), `think: false`, `num_ctx` 8192 |
| Hardware | RTX 3070 8 GB; model 6.4 GB with 5.6 GB in VRAM (partly on CPU) |
| Prompt | `v1` (`PROMPT_VERSION`) |
| Cases | 150, batch 8 → 19 requests, 0 failed, 0 missing |
| **Strict accuracy** | **0.807** (121/150) |
| **Accuracy by acceptable** | **0.887** (133/150) |
| Traps answered acceptably | 39/42 |
| `must_not` hits | 2 (Chrome profile → `cache`, Ollama models → `user_files`) |
| Latency | mean 18.5 s per request of 8 items (2.35 s per item), max 21.4 s; wall 352 s |
| Cost | $0 (local) |

Confusion matrix (rows expected, columns predicted):

| expected \ predicted | gs | gc | ac | ad | uf | de | cr | ss | ri | ca | un | miss | err | n | strict | acceptable |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| game_save (gs) | 15 | · | · | · | · | · | · | · | · | · | · | · | · | 15 | 1.00 | 1.00 |
| game_config (gc) | 2 | 12 | · | · | · | · | · | · | · | · | · | · | · | 14 | 0.86 | 1.00 |
| app_config (ac) | · | · | 13 | 1 | 1 | · | · | · | 1 | · | · | · | · | 16 | 0.81 | 0.88 |
| app_data (ad) | · | · | · | 10 | 2 | · | 1 | · | · | 2 | · | · | · | 15 | 0.67 | 0.93 |
| user_files (uf) | · | · | · | · | 13 | 1 | · | · | · | · | · | · | · | 14 | 0.93 | 1.00 |
| dev_environment (de) | · | · | 3 | · | · | 10 | · | 1 | · | · | · | · | · | 14 | 0.71 | 0.79 |
| credentials (cr) | · | · | 2 | · | · | · | 12 | · | · | · | · | · | · | 14 | 0.86 | 1.00 |
| system_settings (ss) | · | · | · | · | · | · | · | 5 | 8 | · | · | · | · | 13 | 0.38 | 0.46 |
| reinstallable (ri) | · | 1 | · | · | 1 | · | · | · | 14 | · | · | · | · | 16 | 0.88 | 0.88 |
| cache (ca) | · | · | · | · | · | · | · | · | · | 16 | · | · | · | 16 | 1.00 | 1.00 |
| unknown (un) | · | · | · | · | · | · | · | · | 2 | · | 1 | · | · | 3 | 0.33 | 0.33 |

Legend: gs game_save, gc game_config, ac app_config, ad app_data, uf user_files,
de dev_environment, cr credentials, ss system_settings, ri reinstallable, ca cache, un unknown.

Main error patterns (17 cases outside the acceptable set):

1. **System settings → `reinstallable`** (8 of 13): taskbar pins, WinX menu, Libraries, Start
   layout (`start2.bin`), themes, spelling dictionary, IME user dictionary. The model reasons
   "Windows recreates it"; the prompt has no guidance for `system_settings` beyond the one-line
   definition.
2. **UWP `LocalState` → `reinstallable`**: Windows Terminal settings and the Start menu state were
   treated as "Store app, reinstalled from the Store".
3. **Dev config → `app_config`/`system_settings`** (4): DBeaver workspace, PSReadLine history,
   `~/.config` dotfiles. Category boundary, not a gross error; arguably acceptable.
4. **Large mixed folders**: the Chrome profile (cache inside) → `cache` (a `must_not` hit:
   bookmarks/passwords would be lost); the Cyberpunk 2077 install → `game_config` (it claims save
   data exists); Ollama model blobs → `user_files` ("not re-downloadable").
5. **`unknown` is almost never used** (1 of 3); ambiguous blobs get `reinstallable` with
   confidence 0.95. Confidences are high (0.8–1.0) on wrong answers too, so
   `min_confidence_to_apply = 0.5` does not filter these mistakes.

Strong areas: game saves (15/15, including saves in `Documents`, `.zip` saves, saves inside the
install folder), caches (16/16, including JSON-heavy caches and logs under Unreal `Saved`),
Electron traps (Discord/Slack → cache, Signal/Element → app_data).

**NFR-08-02:** the thresholds name `claude-haiku-4-5` (≥ 0.80) and `qwen2.5:7b-instruct`
(≥ 0.70); neither was run (not installed / no cloud calls in this task). For `qwen3.5:9b`
both strict (0.807) and acceptable (0.887) accuracy are above the 0.70 local threshold, and
even above the 0.80 cloud one. **NFR-08-01** (local ≤ 3 s per batch of 8) is **not met** on this
machine: 18.5 s per batch with the model partly offloaded to CPU.
