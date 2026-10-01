# SPEC-12: Тестирование и CI

| Поле | Значение |
|---|---|
| ID | SPEC-12 |
| Статус | in-progress |
| Фаза | P0 (сквозная, дополняется в каждой фазе) |
| Крейт(ы) | все, `fixtures/`, `xtask/`, `.github/workflows/` |
| Зависит от | SPEC-00, SPEC-01, SPEC-02 |
| Используется в | все спеки (§6 «Тестирование» каждой спеки опирается на эту) |
| Последнее изменение | 2026-10-01 (§4.4: снапшот TS-деклараций вместо JSON Schema для SPEC-02; §4.8: Node 24 LTS; §4.2: состав `sk-testkit`, `collect_ctx` со сканером, правило unit-тестов; §4.5: `MockClassifier` в `sk-llm`; §4.6: транзитивные рёбра, `xtask`; зависимости T-12-02/04/05/06; §4.8: MSRV 1.93) |

## 1. Цель

Общая стратегия тестирования: как тестировать Windows-специфичную программу, разрабатывая
преимущественно кроссплатформенный код. Как устроены фикстуры, снапшоты, eval LLM и e2e GUI.
Какие проверки выполняет CI на каждый PR. Спеки фич описывают *что* тестировать,
эта спека — *как* и *где*.

## 2. Область

### 2.1 Входит
- Пирамида тестов и соглашения по размещению.
- Генератор фикстур ФС и формат описания фикстур.
- Snapshot-тесты (`insta`).
- Windows-интеграционные тесты, в т.ч. требующие админа.
- Eval-набор для LLM-классификатора (моки + опциональный прогон реальной модели).
- E2E smoke GUI через `tauri-driver` (WebDriver).
- CI GitHub Actions: матрица, шаги, кэши, проверки архитектуры.
- Ручной чек-лист приёмки на эталонной машине.

### 2.2 Не входит
- Релизный pipeline: SPEC-14 §4.7.
- Бенчмарки производительности как CI-гейт (только ручные `cargo bench` в P1). См. §9.

## 3. Требования

### 3.1 Функциональные
- **FR-12-01** — Каждый крейт, кроме `sk-cli`, имеет unit-тесты, которые проходят на Linux, macOS и Windows без реальной Windows-среды, за счёт `Environment::fake` (SPEC-02 §3.3) и фейкового `FsScanner` (SPEC-03 §4.1).
- **FR-12-02** — Windows-специфичные тесты помечены `#[cfg(windows)]` и живут в `tests/win_*.rs` или модулях `win::tests`. Требующие прав администратора или изменяющие систему дополнительно помечены `#[ignore = "admin"]`/`#[ignore = "manual"]`.
- **FR-12-03** — Фикстуры ФС генерируются детерминированно из YAML-описаний командой `cargo xtask fixtures`. Бинарные фикстуры в git не хранятся (кроме ≤ 100 КБ образцов форматов: `.vdf`, `.acf`, `.reg`).
- **FR-12-04** — JSON-контракты (`ScanReport`, `Finding`, `BackupManifest`, конфиг) покрыты `insta`-снапшотами. Изменение контракта видно в PR как дифф `.snap`.
- **FR-12-05** — LLM тестируется без сети: `MockClassifier` и `ReplayProvider` (воспроизведение записанных ответов). Реальная модель — только вручную (`cargo xtask llm-eval --provider ollama`).
- **FR-12-06** — CI на каждый PR и push в `main`: fmt, clippy, тесты (Windows + Linux), cargo-deny, проверка графа крейтов, фронтенд lint/typecheck/test, актуальность `bindings.ts`, сборка release-exe (Windows).
- **FR-12-07** — E2E smoke GUI запускается в CI на Windows (nightly и по метке PR `e2e`), не на каждый push.

### 3.2 Нефункциональные
- **NFR-12-01** — Полный CI на PR ≤ 20 мин (с кэшем).
- **NFR-12-02** — Unit-тесты workspace локально ≤ 60 с.
- **NFR-12-03** — Тесты не зависят от порядка и могут идти параллельно: каждый использует собственный `tempfile::TempDir`. Глобальных путей нет.
- **NFR-12-04** — Тесты никогда не пишут в реальный профиль пользователя. Исключение — Windows-интеграционные тесты, которые используют ключи реестра `HKCU\Software\SaveKeeperTest\<uuid>` и удаляют их в `Drop`-guard.

## 4. Дизайн

### 4.1 Пирамида и размещение

| Уровень | Где | Что | Запуск |
|---|---|---|---|
| Unit | `src/**` `#[cfg(test)] mod tests` | Чистые функции, парсеры, алгоритмы | везде |
| Snapshot | `tests/snapshots/*.snap` | JSON-контракты, `report.html`, промпты LLM | везде |
| Интеграционный (кросс) | `crates/*/tests/*.rs` | Коллектор / бэкап на фикстуре ФС | везде |
| Интеграционный (Windows) | `crates/*/tests/win_*.rs` | Known Folders, реестр, RM API, `reg.exe`, `winget` | windows |
| Admin/manual | те же файлы, `#[ignore]` | драйверы, VSS, UAC | вручную: `cargo test -- --ignored` |
| Pipeline | `crates/sk-engine/tests/` | Полный скан фикстуры `fixtures/profiles/gamer` | везде |
| CLI | `crates/sk-cli/tests/` (`assert_cmd`, `predicates`) | `scan --out`, `backup`, коды выхода | везде |
| Фронт | `app/src/**/*.test.ts(x)` (vitest) | дерево, фильтры, форматирование, i18n | везде |
| E2E GUI | `app/e2e/` (WebdriverIO + tauri-driver) | smoke-сценарий | windows (nightly) |
| LLM eval | `crates/sk-llm/eval/` | качество классификации | вручную |

### 4.2 Общие тестовые утилиты — крейт `sk-testkit`
Dev-dependency для всех крейтов (не входит в релиз; правило графа §4.6 это разрешает).
Сам зависит только от `sk-core` и `sk-scan` (SPEC-01 §4.2). Фейки живут рядом со своими трейтами:
`MemFs` в `sk-scan` (SPEC-03 §4.1), `MockClassifier` в `sk-llm` под feature `mock` (SPEC-08 §4.2.4).
Unit-тесты (`src/**`) крейтов `sk-core` и `sk-scan` не используют `sk-testkit`: в них была бы вторая
копия крейта, и типы не совпали бы. `sk-testkit` в этих крейтах подключается только в `tests/`.

```rust
// sk-testkit
pub struct FakeProfile { pub root: TempDir, pub env: Environment }

impl FakeProfile {
    /// Разворачивает фикстуру fixtures/profiles/<name>.yaml во временную папку и строит Environment::fake(root).
    pub fn load(name: &str) -> Self;
    pub fn path(&self, template: &str) -> PathBuf;             // "{APPDATA}\\Foo" → абсолютный путь внутри root
    pub fn write(&self, template: &str, content: impl AsRef<[u8]>);
    pub fn set_mtime(&self, template: &str, t: OffsetDateTime);
}

/// Контекст коллектора над профилем + приёмник событий для проверок. Сканер передаётся явно (MemFs или RealFs).
pub fn collect_ctx(profile: &FakeProfile, config: Config, scanner: Arc<dyn FsScanner>) -> (CollectContext, Receiver<Event>);
pub fn drain_events(rx: &mut Receiver<Event>) -> Vec<Event>;
pub fn tree_hash(dir: &Path) -> BTreeMap<String, String>;   // относительный путь → blake3 (сравнение деревьев в тестах бэкапа)
pub struct RegTestKey { /* HKCU\Software\SaveKeeperTest\<uuid>, удаляется в Drop */ }  // #[cfg(windows)]
```

### 4.3 Фикстуры ФС

```
fixtures/
├── profiles/                 # описания «профилей пользователя»
│   ├── empty.yaml
│   ├── gamer.yaml            # Steam + 5 игр (Unity LocalLow, Unreal SaveGames, Documents\My Games, Saved Games), Discord, OBS
│   ├── developer.yaml        # .ssh, .gitconfig, VS Code, JetBrains, 3 git-репо (clean / dirty / unpushed), node_modules
│   ├── office.yaml           # документы вне Documents, OneDrive-перенаправление, Outlook .pst
│   ├── messy.yaml            # 200 неизвестных AppData-папок, кэши, длинные пути, кириллица, пробелы, non-UTF8 (только Linux)
│   └── huge.yaml             # генеративный: 1 млн файлов (для ручных бенчей, #[ignore])
├── samples/                  # маленькие реальные образцы форматов
│   ├── steam/libraryfolders.vdf, loginusers.vdf, appmanifest_1245620.acf
│   ├── ludusavi/manifest-mini.yaml   # 20 игр из реального манифеста
│   └── reg/putty-sessions.reg
└── expected/                 # ожидаемые находки: <profile>.findings.yaml (id шаблона, category, app.id)
```

Формат описания профиля:
```yaml
# fixtures/profiles/gamer.yaml
known_folders:                     # переопределения относительно root (по умолчанию стандартная раскладка)
  DOCUMENTS: "OneDrive/Documents"  # эмуляция перенаправления
launchers:
  steam: { root: "Program Files (x86)/Steam", users: ["12345678"] }
tree:
  - path: "{APPDATA}/EldenRing/76561198000000000/ER0000.sl2"
    size: 28311552                  # генерируется детерминированный псевдослучайный контент (seed = path)
    mtime: "-1d"                    # относительные даты от «сейчас» теста
  - path: "{LOCALLOW}/Team Cherry/Hollow Knight/user1.dat"
    size: 12 KiB
  - path: "{LOCALAPPDATA}/discord/Cache/Cache_Data/f_000001"
    size: 1 MiB
    repeat: 500                     # f_000001..f_000500
  - path: "{HOME}/Projects/app/.git"
    git: { commits: 3, dirty: true, remote: null }   # генератор создаёт настоящий git-репо через gix
  - path: "{STEAM}/config/libraryfolders.vdf"
    sample: "steam/libraryfolders.vdf"
```
- `cargo xtask fixtures [--profile gamer] [--out target/fixtures]` материализует профили. `FakeProfile::load` делает то же в `TempDir` (с кэшем по хэшу YAML в `target/fixtures-cache`, копированием).
- На Windows генератор выставляет атрибуты (`hidden`, `readonly`), если они указаны в YAML (`attrs: [hidden]`).
- `expected/*.findings.yaml` — «золотые» ожидания для pipeline-тестов: тест проверяет, что каждая ожидаемая находка присутствует (recall), и выводит список лишних (без падения, как warning в отчёте теста).

### 4.4 Snapshot-тесты
- `insta` с `json`/`yaml`-снапшотами, `redactions` для недетерминированных полей: `scan_id`, `*_at`, `app_version`, `mtime`, абсолютные пути (`[root]`).
- Обновление: `cargo insta review`. В CI `INSTA_UPDATE=no`, так что несовпадение роняет тест.
- Что обязательно покрыть снапшотами: TS-декларации типов (`specta-typescript`, SPEC-02 T-02-09) и пример `ScanReport`/`Finding` (SPEC-02), `Config` по умолчанию (SPEC-01), `BackupManifest` (SPEC-10), промпт и JSON Schema ответа LLM (SPEC-08), `report.html` фикстуры (SPEC-10).

### 4.5 LLM: моки и eval
- `MockClassifier` (в `sk-llm`, feature `mock`, SPEC-08 §4.2.4): правила «шаблон пути → Classification», по умолчанию `Unknown`. Используется во всех тестах pipeline.
- `ReplayProvider` (в `sk-llm`, feature `replay`): читает `eval/cassettes/<provider>-<model>.jsonl` (запрос-хэш → ответ). Проверяет парсинг, ретраи, кэш без сети.
- Eval-набор `crates/sk-llm/eval/dataset.jsonl`: ≥ 150 размеченных `FolderSummary` (из фикстуры `messy` + реальные, обезличенные вручную) с полями `expected_category`, `expected_app` (опционально), `must_not`: например `Cache` не должен быть `GameSave`.
- `cargo xtask llm-eval --provider ollama|anthropic|openai-compat --model <m> [--record]`:
  - метрики: accuracy по категории, macro-F1, доля «опасных ошибок» (незаменимое → `Cache`/`Reinstallable`), средняя латентность, стоимость (для облака);
  - `--record` пишет кассету для `ReplayProvider`;
  - результат в `target/llm-eval/<date>-<model>.md`.
- Порог приёмки для модели по умолчанию (SPEC-08): accuracy ≥ 0.8, опасные ошибки ≤ 3%.

### 4.6 Проверка архитектуры (граф крейтов)
`cargo xtask check-deps` на базе `cargo metadata --format-version 1`:
- разрешённые рёбра между крейтами workspace — граф SPEC-01 §4.2 с транзитивным замыканием, плюс `sk-testkit → sk-core, sk-scan`; `xtask` может зависеть от любых крейтов (таблица в `xtask/src/deps_rules.rs`);
- `sk-testkit` разрешён только как `dev-dependency`, от `xtask` не зависит никто;
- крейт workspace, которого нет в таблице, — ошибка (новый крейт требует правки SPEC-01 §4.2);
- `reqwest` (сеть) разрешён только в `sk-llm`, `sk-games` (NFR-01-03) и `app/src-tauri` (проверка обновлений SPEC-14, опционально);
- запрещены `openssl-sys` (используем `rustls`) и дубли крупных зависимостей (`windows` — одна мажорная версия).
Нарушение выводит ребро и правило, CI падает.

### 4.7 E2E GUI
- `tauri-driver` + `msedgedriver` (версия под WebView2 раннера) + WebdriverIO (`app/e2e/wdio.conf.ts`).
- Приложение собирается с feature `e2e`. Команды `start_scan` игнорируют реальный профиль и используют `SK_E2E_PROFILE=<path>` (материализованная фикстура `gamer`), бэкап пишется в `SK_E2E_OUT`.
- Сценарий smoke (`app/e2e/smoke.spec.ts`):
  1. окно открылось, язык — en (форсируется `SK_LANG=en`);
  2. «Start scan» → ожидание экрана результатов ≤ 60 с;
  3. в дереве есть «ELDEN RING» и «Hollow Knight», выбраны по умолчанию;
  4. снять Discord cache (если выбран), «Next: backup», задать папку через тестовый хук (`SK_E2E_OUT`, диалог не открывается);
  5. «Start backup» → экран итога со статусом complete;
  6. проверка на ФС: zip существует, `manifest.json` валиден.
- Фича `e2e` не попадает в релизную сборку (проверка в SPEC-14).

### 4.8 CI (GitHub Actions)

`.github/workflows/ci.yml`:

```yaml
on: { pull_request: {}, push: { branches: [main] } }
concurrency: { group: ci-${{ github.ref }}, cancel-in-progress: true }
jobs:
  lint:            # ubuntu-latest
    steps: [checkout, rust-toolchain(stable, rustfmt, clippy), rust-cache,
            "cargo fmt --all --check",
            "cargo xtask check-deps",
            "cargo deny check"]           # EmbarkStudios/cargo-deny-action
  test:
    strategy: { matrix: { os: [windows-latest, ubuntu-latest] } }
    steps: [checkout, rust-toolchain, rust-cache (key: os + Cargo.lock),
            "cargo clippy --workspace --all-targets --locked -- -D warnings",   # на windows — все крейты, на ubuntu — --exclude app
            "cargo xtask fixtures",
            "cargo test --workspace --locked"]     # ubuntu: --exclude savekeeper-app
    env: { INSTA_UPDATE: "no", RUST_BACKTRACE: "1" }
  msrv:            # ubuntu-latest, toolchain 1.93: cargo check --workspace --exclude savekeeper-app
  frontend:        # ubuntu-latest
    steps: [checkout, pnpm/action-setup, setup-node(24, cache pnpm),
            "pnpm -C app install --frozen-lockfile",
            "pnpm -C app lint", "pnpm -C app typecheck", "pnpm -C app test -- --run",
            "cargo xtask bindings && git diff --exit-code app/src/bindings.ts",
            "cargo xtask i18n-check"]     # ключи ru/en совпадают, все message_key из Rust присутствуют
  build-windows:   # windows-latest, needs: [test, frontend]
    steps: ["pnpm -C app tauri build --no-bundle", upload-artifact(savekeeper.exe, retention 7d)]
```

`.github/workflows/nightly.yml` (cron `0 3 * * *` + `workflow_dispatch` + PR label `e2e`):
- `e2e-windows`: сборка с `--features e2e`, установка `msedgedriver` под версию WebView2, `cargo install tauri-driver --locked`, `pnpm -C app e2e`.
- `slow-tests`: `cargo test --workspace -- --ignored` **без** admin-тестов (фильтр `--skip admin`), в т.ч. zip64 5 ГиБ (только если на раннере ≥ 15 ГБ свободно).
- `advisories`: `cargo deny check advisories` (падает только nightly, в PR — warning).

Кэши: `Swatinem/rust-cache` (отдельно по os и job), pnpm store, `target/fixtures-cache`.
Секреты в CI: **нет** (LLM eval не выполняется в CI).

### 4.9 `xtask`
Крейт `xtask/` (бинарник, не публикуется): `fixtures`, `check-deps`, `bindings` (запуск экспорта specta, SPEC-02 T-02-09), `i18n-check`, `llm-eval`, `dist` (SPEC-14). Алиас в `.cargo/config.toml`: `xtask = "run -p xtask --"`.

### 4.10 Ручной чек-лист приёмки на эталонной машине (P1 и далее)
Эталонная машина — реальный Windows-ПК разработчика (+ по возможности чистая VM Windows 10 22H2 и Windows 11 с тестовыми данными).
Файл `specs/checklists/reference-machine.md` (создаётся в T-12-08), поля:
1. Составить вручную список «что я обязан сохранить» (≥ 40 пунктов: игры, программы, dev, система) до запуска SaveKeeper.
2. Запустить `savekeeper-cli scan --out ref.json` (без LLM), затем с `--llm local`.
3. Для каждого пункта отметить: найдено / найдено с неверной категорией / не найдено. Метрика P1: recall ≥ 90% (SPEC-00 §6).
4. Отметить ложные «важные» (`default_selected = true`, но это мусор): не более 5% выбранного объёма.
5. Засечь время скана и пиковую память (Диспетчер задач / `Get-Process`), сверить с SPEC-00 P1 и NFR-01-02.
6. Каждый пропуск оформить задачей в правила (SPEC-04) или эвристики (SPEC-07).

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| На Linux нет `reg.exe`/`winget` | Соответствующие тесты под `#[cfg(windows)]`, заглушки `sk-system` возвращают `Unavailable`, и на Linux тестируется именно это. |
| Флаки из-за mtime-разрешения ФС (FAT/exFAT 2 с) | Фикстуры только на NTFS/ext4 (temp раннера). В сравнениях mtime допуск 2 с. |
| Non-UTF8 имена недоступны на Windows (NTFS — UTF-16, но возможны непарные суррогаты) | Отдельный Windows-тест с непарным суррогатом через `OsString::from_wide`. На Linux — невалидные байты. |
| GitHub windows-latest сменил образ | Фиксировать `windows-2022` для e2e (версия WebView2/Edge driver), `windows-latest` для остального. |
| Тест оставил ключ реестра | `RegTestKey` удаляет в `Drop`. Nightly-джоба в конце проверяет, что `HKCU\Software\SaveKeeperTest` пуст. |

## 6. Тестирование

Эта спека описывает тесты; самопроверка:
- `sk-testkit`: тест, что `FakeProfile::load("gamer")` детерминирован (два раза → одинаковый `tree_hash`).
- `xtask check-deps`: тест на фейковом `cargo metadata` JSON с запрещённым ребром.
- `xtask i18n-check`: тест с намеренно отсутствующим ключом.

## 7. Задачи

- [x] **T-12-01** — Крейт `xtask` с алиасом, подкоманда-заглушка для каждой команды §4.9. *Зависит:* T-01-01.
- [x] **T-12-02** — CI `ci.yml`: jobs lint, test (win+linux), msrv. *Зависит:* T-12-01, T-12-03 (lint-job вызывает `check-deps`). Шаг `cargo xtask fixtures` подключается в T-12-06, `cargo deny check` — в T-12-04. *Готово, когда:* зелёный прогон на пустом workspace.
- [x] **T-12-03** — `cargo xtask check-deps` по правилам §4.6. *Зависит:* T-12-01. *Готово, когда:* тест с запрещённым ребром.
- [x] **T-12-04** — `deny.toml`: лицензии (allow: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode-3.0, MPL-2.0), bans, advisories, sources (только crates.io) + шаг `cargo deny check` в lint-job `ci.yml`. *Зависит:* T-12-02.
- [ ] **T-12-05** — `sk-testkit`: `FakeProfile`, `collect_ctx`, `drain_events`, `tree_hash`, `RegTestKey` (`MockClassifier` — в SPEC-08 T-08-01). *Зависит:* T-02-05, T-01-03.
- [ ] **T-12-06** — Генератор фикстур (`cargo xtask fixtures`) + профили `empty`, `gamer`, `developer` + `samples/` + шаг `cargo xtask fixtures` в `ci.yml`. *Зависит:* T-12-05, T-12-02. *Готово, когда:* детерминизм-тест.
- [ ] **T-12-07** — Профили `office`, `messy`, `huge` + `expected/*.findings.yaml`. *Зависит:* T-12-06, фаза P1.
- [ ] **T-12-08** — Ручной чек-лист `specs/checklists/reference-machine.md`. *Зависит:* —.
- [ ] **T-12-09** — CI job `frontend` + `xtask bindings` + `xtask i18n-check`. *Зависит:* T-11-02.
- [ ] **T-12-10** — CI job `build-windows` (артефакт exe). *Зависит:* T-11-01.
- [ ] **T-12-11** — E2E: feature `e2e` (`SK_E2E_PROFILE`, `SK_E2E_OUT`, `SK_LANG`), WebdriverIO + tauri-driver, smoke-сценарий §4.7, `nightly.yml`. *Зависит:* T-11-11, T-12-06.
- [ ] **T-12-12** — LLM eval: датасет ≥ 150 примеров, `xtask llm-eval`, `ReplayProvider`-кассеты. *Зависит:* SPEC-08 (провайдеры), T-12-07.

## 8. Критерии приёмки

- [ ] PR с намеренной ошибкой форматирования, clippy-предупреждением, запрещённым ребром крейтов, устаревшим `bindings.ts` или отсутствующим i18n-ключом — каждый падает в CI на соответствующем шаге.
- [ ] `cargo test --workspace` проходит на Windows и Linux, а на Linux не требует ни одной Windows-утилиты.
- [ ] Pipeline-тест на `gamer` и `developer` показывает recall 100% по `expected/*.findings.yaml` к концу P2.
- [ ] Nightly e2e smoke зелёный 5 прогонов подряд перед релизом v0.1.0.

## 9. Открытые вопросы

- Бенчмарки (`criterion`) скана на `huge` как нерегулярный CI-гейт (раз в неделю, сравнение с baseline)? Предварительно: ручные в P1, автоматизировать при появлении регрессий.
- Self-hosted Windows-раннер для admin-тестов (драйверы, VSS)? Пока ручной прогон перед релизом.
- ~~Предложение к SPEC-01 §4.1: добавить в структуру репозитория `xtask/` и `crates/sk-testkit/` (dev-only), а в §4.2 — правило «`sk-testkit` только как dev-dependency».~~ **Принято** в SPEC-01 §4.1, §4.2.
