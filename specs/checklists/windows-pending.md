# Отложенные проверки на Windows

| Поле | Значение |
|---|---|
| Назначение | Задачи, сделанные на macOS, чей Windows-код (`#[cfg(windows)]`, `win`-модули, `tests/win_*.rs`) не компилировался и не запускался локально |
| Где выполнять | Windows-ПК разработчика, корень репозитория |
| Как закрывать | Выполнить команды раздела; при успехе отметить `[x]`; при падении — завести `fix`-коммит с номером задачи |

Базовые проверки для каждого пункта:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Задачи

- [ ] **macOS test fixes** (коммит `test: fix test builds on non-Windows hosts`): в `crates/sk-system/src/exporters/cloud_tests.rs` импорт `SystemExporter` перенесён в `#[cfg(windows)] mod win`. Проверить: `cargo clippy -p sk-system --all-targets -- -D warnings` и `cargo test -p sk-system cloud` (Windows-модуль компилируется и проходит).

## Тесты, падающие на macOS (сверить на Windows и Linux)

Найдено 2026-10-31 при первом прогоне `cargo test --workspace` на macOS (Rust 1.99.1), до
изменений T-08-15 и не связано с ними (крейты не зависят от `sk-llm`). Если на Windows и в
Linux-CI они зелёные — это особенности macOS (семантика путей), нужна отдельная задача/правка
спеки на поддержку macOS как хоста разработки; если красные — регрессия.

- [ ] `sk-heuristics`: `git::bundle_tests::merged_submodule_has_no_child_of_its_own`
- [ ] `sk-heuristics`: `junk::env_tests::{launcher_folders_and_game_libraries_are_reinstallable, steamapps_common_is_a_library_even_with_one_game, the_folder_of_a_single_game_is_not_a_library, the_parent_of_two_games_is_a_library}` — «expected Reinstallable (heuristic.junk.launcher), got None»
- [ ] `sk-system`: `cmd::tests::fake_runner_answers_by_file_name_and_records_calls`
- [ ] `sk-system`: `git_bundle::gitdir_tests::{gitdir_and_commondir_are_checked, cloud_only_gitdir_of_linked_worktree_runs_no_git}` — `linked/../main` не нормализуется
- [ ] `sk-system`: `git_bundle::tests::bundle_and_changed_files_are_exported` — `safe.directory`
- [ ] `sk-system`: `wifi::tests::{probe_asks_the_wlan_service_only_when_the_folder_cannot_be_read, run::run_without_keys_removes_them_and_writes_only_target_dir}` — ожидается `System32\netsh.exe`, получено `/netsh.exe`

Команда для сверки:

```powershell
cargo test -p sk-heuristics -p sk-system
```
