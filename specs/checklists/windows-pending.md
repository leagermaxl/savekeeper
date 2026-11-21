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

- [x] **macOS test fixes** (коммит `test: fix test builds on non-Windows hosts`): в `crates/sk-system/src/exporters/cloud_tests.rs` импорт `SystemExporter` перенесён в `#[cfg(windows)] mod win`. Проверить: `cargo clippy -p sk-system --all-targets -- -D warnings` и `cargo test -p sk-system cloud` (Windows-модуль компилируется и проходит).

- [x] **T-09-04** (`merge_nested`): `crates/sk-score/src/nested.rs::split` повторяет покомпонентное сравнение путей `sk_core::path` (буквы дисков, UNC, `\\?\`, регистр); тесты есть только на Unix-путях `Environment::fake`. Проверить: `cargo test -p sk-score nested` — и вручную, что находки `C:\Users\…` и `\\?\C:\Users\…` с разным регистром корректно вкладываются.

## Тесты, падающие на macOS (сверить на Windows и Linux)

Найдено 2026-10-31 при первом прогоне `cargo test --workspace` на macOS (Rust 1.99.1), до
изменений T-08-15 и не связано с ними (крейты не зависят от `sk-llm`). Если на Windows и в
Linux-CI они зелёные — это особенности macOS (семантика путей), нужна отдельная задача/правка
спеки на поддержку macOS как хоста разработки; если красные — регрессия.

- [x] `sk-heuristics`: `git::bundle_tests::merged_submodule_has_no_child_of_its_own`
- [x] `sk-heuristics`: `junk::env_tests::{launcher_folders_and_game_libraries_are_reinstallable, steamapps_common_is_a_library_even_with_one_game, the_folder_of_a_single_game_is_not_a_library, the_parent_of_two_games_is_a_library}` — «expected Reinstallable (heuristic.junk.launcher), got None»
- [x] `sk-system`: `cmd::tests::fake_runner_answers_by_file_name_and_records_calls`
- [x] `sk-system`: `git_bundle::gitdir_tests::{gitdir_and_commondir_are_checked, cloud_only_gitdir_of_linked_worktree_runs_no_git}` — `linked/../main` не нормализуется
- [x] `sk-system`: `git_bundle::tests::bundle_and_changed_files_are_exported` — `safe.directory`
- [x] `sk-system`: `wifi::tests::{probe_asks_the_wlan_service_only_when_the_folder_cannot_be_read, run::run_without_keys_removes_them_and_writes_only_target_dir}` — ожидается `System32\netsh.exe`, получено `/netsh.exe`

Команда для сверки:

```powershell
cargo test -p sk-heuristics -p sk-system
```

## Результат проверки на Windows (2026-11-21)

Windows 10 Pro 22H2 (19045), MSVC 14.44: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace` — зелёные (72 тестовых бинарника, 0 упавших).
Все тесты из списка выше на Windows проходят: падения на macOS — особенности семантики путей
macOS, не регрессия. Для T-09-04 добавлены Windows-тесты `sk-score::nested_win_tests`
(`C:\…` ↔ `\?\c:\…`, UNC ↔ `\?\UNC\…`, разный регистр) — вложение работает.
