# SPEC-04: Движок правил и база известных мест

| Поле | Значение |
|---|---|
| ID | SPEC-04 |
| Статус | in-progress |
| Фаза | P1 |
| Крейт(ы) | `sk-rules`, каталог `rules/` |
| Зависит от | SPEC-01, SPEC-02, SPEC-03 |
| Используется в | SPEC-07 (claimed_paths), SPEC-09, SPEC-11 (редактор правил, P4) |
| Последнее изменение | 2026-10-03 (T-04-03/T-04-04: API `RuleSource`/`conditions`/`registry`, семантика условий, `installed.winget` зарезервирован, regex проверяется при компиляции, ключи issue в §5, детали слияния; T-04-02: `RuleDiagnostic.severity`, API `compile`/`diagnostic` в §4.1, `hklm` в §4.2 согласован с §4.4; уточнения по T-04-01: `tags` у target, `registry.recursive` по умолчанию `true`, `file_contains.pattern` — regex, обязательные поля §4.4; 2026-10-02: T-04-07: ручные проверки критериев SPEC-03 §8; T-04-07: ручная проверка Ctrl+C из SPEC-01 §8; 2026-10-01: YAML: `serde-saphyr` вместо `serde_yaml`; `from_json` в схеме v1) |

## 1. Цель

Декларативно описать, где лежат настройки и данные известных программ, и превратить эти
описания в находки (`Finding`). Правила — основной детерминированный источник (P4).
Они покрывают около 80% типичной машины без эвристик и LLM. Формат простой: пользователь
или контрибьютор добавляет YAML-файл без перекомпиляции.

## 2. Область

### 2.1 Входит
- Формат YAML-правила (схема v1) и его валидация.
- Загрузка: встроенные правила (`include_dir!("rules/")`) + пользовательские `savekeeper-data/rules.d/*.yaml`.
- Приоритеты и переопределения.
- Условия (`conditions`) и матчинг.
- `RulesCollector: Collector` → findings + `claimed_paths`.
- Стартовая база правил (§4.7).

### 2.2 Не входит
- Сохранения игр из манифеста Ludusavi: SPEC-05. Правила для игр вне манифеста и эмуляторов живут здесь (§4.7.9).
- Системные экспорты (winget, Wi-Fi ...): SPEC-06. Правило может ссылаться только на `Target::Registry`, но не на `SystemExport`.
- UI-редактор правил: будущее (SPEC-11, P4).

## 3. Требования

### 3.1 Функциональные
- **FR-04-01** — Один YAML-файл содержит одно или несколько правил (`rules: [...]`).
- **FR-04-02** — Каждое правило порождает **не больше одной находки на каждый раскрытый путь** target'а. Если target раскрывается в несколько путей (`{STEAM_USERID}`), будет несколько находок.
- **FR-04-03** — Находка создаётся только если выполнены `conditions` правила и хотя бы один путь target'а существует (или ветка реестра существует).
- **FR-04-04** — Все пути из сработавших правил (корни targets) попадают в `claimed_paths`, даже если находка отфильтрована как `Cache` или `Reinstallable`.
- **FR-04-05** — Правило может объявить `claims` — дополнительные пути, которые считаются «объяснёнными», но не сохраняются (например, кэш браузера), чтобы SPEC-07 не считал их неизвестными.
- **FR-04-06** — Пользовательское правило с тем же `id` полностью заменяет встроенное. Правило с `disabled: true` отключает встроенное.
- **FR-04-07** — Ошибки валидации встроенных правил ломают сборку (тест). Ошибки пользовательских правил приводят к `ScanIssue::Warning` и пропуску файла.
- **FR-04-08** — `savekeeper-cli rules validate [path]` проверяет файлы и выводит ошибки с номерами строк.

### 3.2 Нефункциональные
- **NFR-04-01** — Загрузка и компиляция 500 правил ≤ 50 мс. Прогон всех правил ≤ 1 с (только `exists`/`read_dir`, без обхода; размеры считает фаза Measure).
- **NFR-04-02** — Правила не выполняют код: никаких скриптов и regex с backtracking (используем `regex` crate, он линейный).

## 4. Дизайн

### 4.1 Публичный API

```rust
pub struct RuleSet { rules: Vec<CompiledRule>, sources: Vec<RuleSource> }

impl RuleSet {
    pub fn builtin() -> Result<Self, RuleError>;                         // из include_dir
    pub fn load(builtin: bool, user_dir: Option<&Path>) -> (Self, Vec<ScanIssue>);
    pub fn validate_file(path: &Path) -> Vec<RuleDiagnostic>;            // для CLI
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, id: &str) -> Option<&CompiledRule>;
    pub fn source(&self, id: &str) -> Option<&RuleSource>;            // источник активного правила (бейдж «ваше правило», §4.6)
}
// Отключённые правила в RuleSet не входят.
pub enum RuleSource { Builtin { file: String /* имя файла в rules/ */ }, User { file: PathBuf } } // + fn is_user(&self) -> bool

pub struct RulesCollector { set: Arc<RuleSet> }
impl Collector for RulesCollector { /* id() = "rules" */ }

pub struct RuleDiagnostic { pub file: PathBuf, pub line: Option<usize>, pub rule_id: Option<String>, pub severity: DiagnosticSeverity, pub message: String }
/// Error — файл невалиден; Warning — автоисправление §4.4 (например, credentials → high), файл валиден.
pub enum DiagnosticSeverity { Error, Warning }

// Компиляция одного файла правил (§4.4); файл проходит или отклоняется целиком, возвращаются все ошибки.
pub mod compile {
    pub fn compile_yaml(text: &str) -> Result<CompiledFile, Vec<RuleError>>;
    pub fn compile(file: RuleFile) -> Result<CompiledFile, Vec<RuleError>>;
    pub struct CompiledFile { pub rules: Vec<CompiledRule>, pub warnings: Vec<RuleWarning> }
    pub struct RuleWarning { /* rule_id, message */ }
    pub struct CompiledRule { pub rule: Rule, pub targets: Vec<CompiledTarget> }
    pub struct CompiledTarget { pub root: TargetRoot, /* globs, effective category/sensitivity/tags, optional, label_key */ }
    pub enum TargetRoot { Path { template: PathTemplate, glob_root: bool }, Registry(..), FromJson(..) }
}

// Условия (§4.3). Evaluator — Sync, кэш exists/installed/registry_exists/regex на один скан.
pub mod conditions {
    pub const APP_RUNNING_TAG: &str = "app-running";
    pub struct ConditionOutcome { pub matched: bool, pub app_running: bool }
    pub struct ConditionEvaluator<'a>;
    impl ConditionEvaluator<'_> {
        pub fn new(env: &Environment, fs: &dyn FsScanner, registry: &dyn RegistryProbe, resolve: &ResolveContext) -> Self;
        pub fn evaluate(&self, rule: &Rule) -> ConditionOutcome;
        pub fn take_issues(&self) -> Vec<ScanIssue>;
    }
}

// Доступ к реестру для registry_exists.
pub mod registry {
    pub enum KeyState { Present, Missing, AccessDenied }
    pub trait RegistryProbe: Send + Sync { fn key_state(&self, hive: RegHive, key: &str) -> KeyState; }
    pub struct SystemRegistry; // winreg, только KEY_READ; не-Windows: всегда Missing
    pub struct MemRegistry;    // фейк для тестов: new/add_key/add_denied_key, без учёта регистра, родители существуют
}

// Диагностика для CLI (RuleSet::validate_file делегирует сюда).
pub mod diagnostic {
    pub fn validate_file(path: &Path) -> Vec<RuleDiagnostic>;
    pub fn validate_str(file: &Path, text: &str) -> Vec<RuleDiagnostic>;
}

#[derive(thiserror::Error, Debug)]
pub enum RuleError { Yaml(serde_saphyr::Error), Invalid { rule_id: String, reason: String }, DuplicateId(String) }
```

### 4.2 Формат правила (схема v1)

```yaml
schema_version: 1
rules:
  - id: vscode.user-settings                # уникален, [a-z0-9._-]+, префикс = app.id
    app:
      id: vscode
      name: Visual Studio Code
      kind: dev_tool                        # game | application | system | dev_tool
      winget: Microsoft.VisualStudioCode    # → AppRef.source_ids.winget (опционально)
    category: app_config                    # SPEC-02 §2.3, snake_case
    title_key: rules.vscode.user_settings   # ключ i18n; или title: "VS Code — настройки"
    message_key: evidence.rule_match        # по умолчанию evidence.rule_match
    confidence: 0.95                        # 0..1, по умолчанию 0.9
    sensitivity: none                       # none | low | high
    tags: [ide]
    priority: 100                           # больше → важнее при конфликтах (по умолчанию 100)
    conditions:                             # все должны выполняться (AND); any_of — OR
      - exists: "{APPDATA}\\Code\\User"
    targets:
      - path: "{APPDATA}\\Code\\User"       # FileSet, если каталог; File, если файл
        include: ["settings.json", "keybindings.json", "snippets/**", "profiles/**", "tasks.json"]
        exclude: ["workspaceStorage/**", "History/**", "globalStorage/**/*.vsix"]
      - registry: { hive: hkcu, key: "Software\\Classes\\vscode", recursive: true }
        optional: true                      # отсутствие не мешает созданию находок из других targets
    claims:                                 # объяснено, но не сохраняется
      - "{APPDATA}\\Code\\Cache"
      - "{APPDATA}\\Code\\CachedData"
      - "{APPDATA}\\Code\\User\\workspaceStorage"
    notes_key: rules.vscode.notes           # подсказка в UI: «Settings Sync может уже хранить это в облаке»
```

**Семантика targets.** Каждый элемент `targets` становится **отдельной** находкой с
`id = FindingId(target)` (SPEC-02 §2.7). `title` получает суффикс, если target'ов больше одного
(`title_key` + `target.label_key` при наличии). Evidence каждой находки:
`EvidenceSource::Rule { rule_id }`, `confidence` из правила.

Поля target:

| Поле | Тип | Описание |
|---|---|---|
| `path` | PathTemplate | Корень. Каталог → `Target::FileSet`, файл → `Target::File`, определяется по `metadata` в рантайме. |
| `include` / `exclude` | [glob] | Относительно `path`, синтаксис globset, `/` или `\\` допустимы, нормализуются в `/`. |
| `registry` | {hive, key, recursive} | → `Target::Registry`. `recursive` необязательное, по умолчанию `true`. `hive: hklm` допустим только с `category: system_settings` или `app_config` (§4.4) и помечает `requires_elevation: false` (чтение HKLM доступно), экспорт `reg export` HKLM тоже работает без админа для большинства веток. |
| `category` / `sensitivity` | override | Переопределение для конкретного target (например, `Cookies` → high). |
| `optional` | bool | См. FR-04-03. |
| `label_key` | string | Суффикс заголовка. |
| `tags` | [string] | Дополнительные теги находок этого target'а, добавляются к `tags` правила. |
| `glob_root` | bool | Если `true`, `path` может содержать `*` в сегментах (`{APPDATA}\\Mozilla\\Firefox\\Profiles\\*`); каждое совпадение даёт отдельную находку. Глубина `*` не больше 2 сегментов. |
| `from_json` | object | Динамические корни, прочитанные из JSON-конфига программы (§4.2.1). Взаимоисключающее с `path`/`registry`. |

#### 4.2.1 Target `from_json` (динамические пути)

Некоторые программы хранят расположение пользовательских данных в своём конфиге: vault'ы
Obsidian, проекты Unity Hub. Правило с таким target'ом читает конфиг и превращает каждый
найденный путь в отдельную находку.

```yaml
  - id: obsidian.vaults
    app: { id: obsidian, name: Obsidian, kind: application }
    category: user_files
    title_key: rules.obsidian.vault          # label: имя папки vault'а подставляется в args.name
    conditions: [ { exists: "{APPDATA}\\obsidian\\obsidian.json" } ]
    targets:
      - from_json:
          file: "{APPDATA}\\obsidian\\obsidian.json"
          format: json                       # json | jsonc (комментарии и висячие запятые допускаются)
          select: "/vaults/*/path"           # JSON Pointer (RFC 6901) + сегмент `*` = все ключи объекта / элементы массива
          max_matches: 50                    # по умолчанию 50
        include: []                          # применяются к каждому найденному корню
        exclude: [".trash/**"]
        tags: [notes]
```

**Семантика:**
1. `file` раскрывается как обычный `PathTemplate`. Если файла нет и target не `optional`, находок нет, issue не создаётся.
2. Файл читается через `FsScanner::read_small(path, 1 MiB)` (SPEC-03 §4.1). Больше 1 MiB → `ScanIssue::Warning issue.rules.from_json_too_large`. Cloud-only файл не читается (SPEC-03 FR-03-03).
3. Парсинг: `serde_json`. Для `jsonc` перед парсингом удаляются `//` и `/* */` комментарии и висячие запятые (свой минимальный препроцессор с учётом строк, без внешней зависимости).
4. `select` вычисляется по дереву: обычные сегменты как в RFC 6901 (`~0`, `~1`), `*` разворачивается во все ключи объекта или элементы массива. Допускается не больше 3 сегментов `*`.
5. Берутся только **строковые** значения. Нормализация:
   - `file:///C:/x` → `C:\x` (percent-decoding);
   - `/` → `\`;
   - переменные `%VAR%` раскрываются через окружение процесса;
   - относительный путь разрешается относительно папки `file`.
6. Фильтры безопасности (значение отбрасывается с `ScanIssue::Info issue.rules.from_json_skipped`):
   - не абсолютный путь после нормализации;
   - UNC или сетевой диск (`DriveKind::Network`);
   - внутри `{WINDIR}` или `{PROGRAMFILES*}`;
   - путь не существует;
   - дубль другого значения.
7. Каждый оставшийся путь → `PathTemplate::from_path` (SPEC-02 §3.2) → `Target::FileSet` или `Target::File`. `FindingId` считается от шаблона, поэтому vault в `{DOCUMENTS}\Notes` получает одинаковый id на любой машине.
8. Evidence: `EvidenceSource::Rule { rule_id }`, `message_key: "evidence.rule_from_json"`, `message_args: { file: <шаблон file>, select, name: <последний компонент пути> }`.
9. Найденные корни добавляются в `claimed_paths`. Сам `file` тоже добавляется (конфиг объяснён, сохраняется отдельным target'ом `obsidian.config`).
10. Больше `max_matches` значений → берутся первые по порядку в документе + warning.

**Ограничения v1:** только JSON/JSONC. INI, XML, VDF, SQLite не поддерживаются. VDF Steam читает SPEC-05 своим парсером. Остальные форматы — в будущих версиях схемы.

### 4.3 Условия

| Условие | Пример | Семантика |
|---|---|---|
| `exists` | `exists: "{LOCALAPPDATA}\\Obsidian"` | путь существует (`FsScanner::exists`); шаблон, раскрывшийся в несколько путей, — существует хотя бы один; шаблон без путей не существует |
| `not_exists` | | инверсия (шаблон без путей → истинно) |
| `installed` | `installed: { display_name_regex: "(?i)^obs studio" }` или `{ winget: "OBSProject.OBSStudio" }` | совпадение в `Environment.installed_programs` (SPEC-06 §4.4): `display_name_regex` — regex (крейт `regex`) по `InstalledProgram.name`. Если заданы оба поля, достаточно любого (OR). `winget` в v1 зарезервирован: у `InstalledProgram` нет winget-id (SPEC-02 §3.3), поэтому критерий `winget` всегда ложен; правило с `winget` без `display_name_regex` даёт warning при компиляции (§4.4). `installed: {}` без критериев — ошибка валидации. |
| `registry_exists` | `registry_exists: { hive: hkcu, key: "Software\\SimonTatham\\PuTTY" }` | ключ существует |
| `file_contains` | `{ path: "...", pattern: "\"telemetry\"", max_bytes: 65536 }` | `read_small` + regex (редко, для различения форков). `pattern` — регулярное выражение (крейт `regex`); обычная подстрока — это regex без метасимволов. `max_bytes` необязателен, по умолчанию 65536, не больше 1 MiB (иначе ошибка валидации). Файл больше `max_bytes`, cloud-only или нечитаемый → условие ложно, без issue. |
| `os` | `os: { min_build: 22000 }` | версия Windows |
| `any_of` | `any_of: [ {exists: ...}, {installed: ...} ]` | OR; `any_of: []` ложно |
| `process_running` | `process_running: "obs64.exe"` | **Не влияет на создание находки.** Добавляет тег `app-running` → UI/SPEC-10 предупредит «закройте программу перед бэкапом». |

Пустой `conditions` означает «достаточно существования хотя бы одного target'а» (FR-04-03).

`process_running` не участвует в AND/OR: правило, где оно единственное условие (или единственный элемент `any_of`), совпадает; в смешанном `any_of` оно игнорируется. `app_running` ставится, только если правило совпало.

### 4.4 Компиляция и валидация
1. Парсинг `serde-saphyr` с `deny_unknown_fields` (опечатки ловятся сразу).
2. Проверки: `schema_version == 1`; если у правила нет `disabled: true`, обязательны `app`, `category`, `title_key` или `title` и непустой `targets` или `claims` (правилу только с отключением, §4.6, достаточно `id`); `id` по regex и уникальность в пределах источника; `PathTemplate::parse` для всех путей (неизвестный токен — ошибка); глобы компилируются; `confidence ∈ [0,1]`; `category: credentials` ⇒ `sensitivity: high` (автоматически повышается с warning); `hive: hklm` ⇒ `category ∈ {system_settings, app_config}`; `*` в `path` только при `glob_root: true`; `installed.display_name_regex` и `file_contains.pattern` (включая вложенные в `any_of`) компилируются крейтом `regex`, ошибка — ошибка валидации правила; `installed: {}` — ошибка; `installed` с `winget` без `display_name_regex` — warning; `file_contains.max_bytes` ≤ 1 MiB.
3. Результат `CompiledRule { rule, targets: Vec<CompiledTarget { template, include: GlobSet, exclude: GlobSet, ... }> }`.
4. Слияние источников: builtin → user (по `id`: replace / disable). Итоговый порядок: `priority desc`, затем `id`. Встроенные файлы — один источник: `id` уникален во всех `rules/*.yaml`, иначе `RuleError::DuplicateId` (`builtin()` возвращает первую ошибку). Пользовательские правила проверяются на уникальность внутри файла, между файлами действует §5. `disabled: true` у встроенного правила делает его неактивным. Отключение несуществующего `id` молча игнорируется. Автоисправления (warnings §4.4) в пользовательских файлах не порождают `ScanIssue`. Если встроенные правила не загрузились, `load` продолжает только с пользовательскими и добавляет `ScanIssue::Error issue.rules.builtin_invalid {error}`.

### 4.5 Алгоритм `RulesCollector::collect`
1. Для каждого правила (параллельно через rayon, правила независимы):
   1. Проверить `conditions` (кэш результатов `exists`/`installed` в `HashMap` на скан).
   2. Для каждого target раскрыть шаблон (`resolve`, `glob_root` → `read_dir` по сегментам).
   3. Для каждого существующего пути создать `Finding { target, category, app, title, evidence, sensitivity, tags, default_selected: false /*SPEC-09*/, stats: None }`.
   4. Добавить корни targets и `claims` (раскрытые) в `claimed_paths`.
   5. Проверить `process_running` по `Environment.running_processes` (SPEC-02 §3.3, снимок делается один раз в фазе Environment).
2. Конфликты: если два правила дают одинаковый `FindingId`, остаётся находка от правила с большим `priority`, evidence второго дописывается. Вложенность (правило A — `{APPDATA}\Foo`, правило B — `{APPDATA}\Foo\Bar`) не решается здесь, это задача SPEC-09 §merge.
3. `Event::Progress { phase: Collect, current: rule_id }` каждые N правил.

### 4.6 Пользовательские правила
- Каталог `savekeeper-data/rules.d/` (SPEC-01 §4.8.1), файлы `*.yaml`/`*.yml`, загружаются в алфавитном порядке. Читаются только файлы прямо в `rules.d/` (без подпапок), расширение без учёта регистра; порядок алфавитный без учёта регистра, при равенстве — точное сравнение. Отсутствие `rules.d` — не ошибка.
- Пример отключения: `rules: [{ id: discord.settings, disabled: true }]` (все остальные поля необязательны при `disabled`).
- В UI (SPEC-11) у находки из пользовательского правила бейдж «ваше правило».

### 4.7 Стартовая база правил (встроенная, `rules/*.yaml`)

Группировка по файлам. Для каждого правила указаны `id`, target(s) и категория. Детальные
include/exclude пишутся в YAML при реализации T-04-07..T-04-09 по этому списку.

#### 4.7.1 `rules/browsers.yaml` (category `app_data`, sensitivity `high`, note: «пароли и закладки надёжнее синхронизировать аккаунтом браузера»)
| id | Путь | Include / Exclude / claims |
|---|---|---|
| `chrome.profiles` | `{LOCALAPPDATA}\Google\Chrome\User Data` | include: `*/Bookmarks, */Preferences, */Secure Preferences, */Extensions/**, */Local Extension Settings/**, */Login Data*, */Web Data, */History, Local State`; claims: `*/Cache, */Code Cache, */GPUCache, */Service Worker/CacheStorage` |
| `edge.profiles` | `{LOCALAPPDATA}\Microsoft\Edge\User Data` | как Chrome |
| `brave.profiles` | `{LOCALAPPDATA}\BraveSoftware\Brave-Browser\User Data` | как Chrome |
| `vivaldi.profiles` | `{LOCALAPPDATA}\Vivaldi\User Data` | как Chrome |
| `opera.profiles` | `{APPDATA}\Opera Software\Opera Stable` | + `{APPDATA}\Opera Software\Opera GX Stable` |
| `yandex.profiles` | `{LOCALAPPDATA}\Yandex\YandexBrowser\User Data` | как Chrome |
| `firefox.profiles` | `{APPDATA}\Mozilla\Firefox` | include: `profiles.ini, Profiles/*/{places.sqlite,key4.db,logins.json,cert9.db,prefs.js,user.js,extensions/**,chrome/**,containers.json,handlers.json,sessionstore*}`; claims: `{LOCALAPPDATA}\Mozilla\Firefox\Profiles` (кэш) |
| `thunderbird.profiles` | `{APPDATA}\Thunderbird` | весь профиль, exclude `**/cache2/**`, category `app_data` |

> Пароли Chromium зашифрованы DPAPI и **не расшифруются** на новой установке Windows (другой мастер-ключ пользователя). Это указывается в `notes_key` и в отчёте. Правило полезно для закладок, расширений и истории.

#### 4.7.2 `rules/dev.yaml` (category `dev_environment`)
| id | Путь | Примечание |
|---|---|---|
| `ssh.keys` | `{HOME}\.ssh` | category `credentials`, sensitivity high |
| `git.config` | `{HOME}\.gitconfig`, `{HOME}\.config\git` | |
| `vscode.user-settings` | `{APPDATA}\Code\User` | см. §4.2; + `vscode.extensions-list` target `{HOME}\.vscode\extensions\extensions.json` (только список) |
| `vscode-insiders.user-settings` | `{APPDATA}\Code - Insiders\User` | |
| `cursor.user-settings` | `{APPDATA}\Cursor\User` | |
| `jetbrains.config` | `{APPDATA}\JetBrains\*` (glob_root) | include: `options/**, keymaps/**, codestyles/**, templates/**, colors/**, *.key`; claims `{LOCALAPPDATA}\JetBrains` |
| `visualstudio.settings` | `{LOCALAPPDATA}\Microsoft\VisualStudio\*` | include `Settings/**, *.vssettings` |
| `windows-terminal.settings` | `{LOCALAPPDATA}\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState` | include `settings.json, state.json`; + Preview-пакет |
| `powershell.profile` | `{DOCUMENTS}\PowerShell`, `{DOCUMENTS}\WindowsPowerShell` | include `*profile*.ps1, Modules/**` (модули также в SPEC-06) |
| `npm.config` | `{HOME}\.npmrc` | sensitivity high (токены) |
| `cargo.config` | `{HOME}\.cargo\config.toml`, `{HOME}\.cargo\credentials.toml` | credentials → high |
| `docker.config` | `{HOME}\.docker\config.json` | high |
| `aws.config` | `{HOME}\.aws` | credentials |
| `kube.config` | `{HOME}\.kube\config` | credentials |
| `putty.sessions` | registry `HKCU\Software\SimonTatham\PuTTY` + `{HOME}\.putty` | |
| `winscp.config` | `{APPDATA}\WinSCP.ini`, registry `HKCU\Software\Martin Prikryl\WinSCP 2` | high |
| `dbeaver.config` | `{APPDATA}\DBeaverData\workspace6\General\.dbeaver` | high (credentials-config.json) |

#### 4.7.3 `rules/media-streaming.yaml`
| id | Путь | Категория |
|---|---|---|
| `obs.config` | `{APPDATA}\obs-studio` | `app_config`; include `basic/**, global.ini, plugin_config/**`; exclude `logs/**, crashes/**, updates/**` |
| `sharex.config` | `{DOCUMENTS}\ShareX` | `app_config`; exclude `Screenshots/**` (они отдельной находкой `sharex.screenshots` → `user_files`) |
| `vlc.config` | `{APPDATA}\vlc` | `app_config`, exclude `art/**` |
| `mpc-hc.settings` | registry `HKCU\Software\MPC-HC` | `app_config` |
| `foobar2000.config` | `{APPDATA}\foobar2000-v2`, `{APPDATA}\foobar2000` | `app_config` |
| `spotify.none` | claims `{APPDATA}\Spotify`, `{LOCALAPPDATA}\Spotify` | только claims (всё в облаке) |

#### 4.7.4 `rules/messengers.yaml`
| id | Путь | Примечание |
|---|---|---|
| `telegram.tdata` | `{APPDATA}\Telegram Desktop\tdata` | `app_data`, sensitivity **high** (сессия = вход в аккаунт без 2FA), exclude `user_data/**, emoji/**, dumps/**`; note: «лучше войти заново» |
| `discord.settings` | `{APPDATA}\discord\settings.json` | `app_config`; claims `{APPDATA}\discord` целиком, `{LOCALAPPDATA}\Discord` (reinstallable) |
| `slack.none` | claims `{APPDATA}\Slack` | в облаке |
| `whatsapp.none` | claims `{LOCALAPPDATA}\Packages\5319275A.WhatsAppDesktop_*` | история в облаке/телефоне |
| `skype.none` | claims | |

#### 4.7.5 `rules/productivity.yaml`
| id | Путь | Примечание |
|---|---|---|
| `obsidian.config` | `{APPDATA}\obsidian\obsidian.json` | `app_config` |
| `obsidian.vaults` | `from_json`: `{APPDATA}\obsidian\obsidian.json`, `/vaults/*/path` | `user_files`, tag `notes`; exclude `.trash/**` (пример §4.2.1) |
| `keepass.config` | `{APPDATA}\KeePass` | `app_config` (`KeePass.config.xml`); сами `.kdbx` ищет SPEC-07 как `credentials` |
| `keepassxc.config` | `{APPDATA}\KeePassXC` | `app_config` |
| `notepadpp.config` | `{APPDATA}\Notepad++` | `app_config`; include `*.xml, userDefineLangs/**, themes/**, plugins/config/**, backup/**` (несохранённые вкладки!) |
| `sublime.config` | `{APPDATA}\Sublime Text\Packages\User`, `...\Local\Session.sublime_session` | |
| `office.templates` | `{APPDATA}\Microsoft\Templates`, `{APPDATA}\Microsoft\UProof` (пользовательский словарь) | `app_config` |
| `outlook.pst` | `{DOCUMENTS}\Outlook Files`, `{LOCALAPPDATA}\Microsoft\Outlook\*.pst` | `app_data`, tag `large`; `*.ost` → claims (кэш Exchange) |
| `autohotkey.scripts` | `{DOCUMENTS}\AutoHotkey` | `user_files` |
| `sevenzip.settings` | registry `HKCU\Software\7-Zip` | `app_config` |
| `total-commander.config` | `{APPDATA}\GHISLER` | `app_config` |
| `everything.config` | `{APPDATA}\Everything\Everything.ini` | `app_config` |
| `qbittorrent.config` | `{APPDATA}\qBittorrent`, `{LOCALAPPDATA}\qBittorrent\BT_backup` | `app_data` (активные торренты) |
| `figma.settings` | `{APPDATA}\Figma\settings.json` | `app_config`; claims `{APPDATA}\Figma` |
| `adobe.settings` | `{APPDATA}\Adobe\Adobe Photoshop *\Adobe Photoshop * Settings` (glob_root), `{APPDATA}\Adobe\Lightroom\Presets`, `{APPDATA}\Adobe\CameraRaw\Settings` | `app_config`; claims `{APPDATA}\Adobe\Common\Media Cache*` |

Правило `from_json` в `rules/dev.yaml`:

| id | Target | Примечание |
|---|---|---|
| `unityhub.projects` | `from_json`: `{APPDATA}\UnityHub\projects-v1.json`, `/data/*/path` | `user_files`, tag `project`; exclude `Library/**, Temp/**, Logs/**, obj/**` (переустанавливаемое), claims эти подпапки |

#### 4.7.6 `rules/hardware-tuning.yaml`
| id | Путь | Категория |
|---|---|---|
| `msi-afterburner.profiles` | `{PROGRAMFILES_X86}\MSI Afterburner\Profiles` | `app_config` (явный корень внутри исключения, SPEC-03 §4.5) |
| `rivatuner.profiles` | `{PROGRAMFILES_X86}\RivaTuner Statistics Server\Profiles` | `app_config` |
| `rainmeter.skins` | `{DOCUMENTS}\Rainmeter\Skins`, `{APPDATA}\Rainmeter\Rainmeter.ini` | `app_config` |
| `wallpaper-engine.config` | `{STEAM}\steamapps\common\wallpaper_engine\config.json` | `app_config` |
| `logitech-ghub.settings` | `{LOCALAPPDATA}\LGHUB\settings.db` | `app_config` |
| `razer-synapse.none` | claims | профили в облаке |

#### 4.7.7 `rules/windows-shell.yaml` (category `system_settings`)
| id | Target | Примечание |
|---|---|---|
| `windows.start-taskbar-pins` | `{APPDATA}\Microsoft\Internet Explorer\Quick Launch\User Pinned` | информационно |
| `windows.sendto` | `{APPDATA}\Microsoft\Windows\SendTo` | |
| `windows.user-fonts` | — | **не здесь**, SPEC-06 (`fonts`) |
| `windows.explorer-quickaccess` | `{APPDATA}\Microsoft\Windows\Recent\AutomaticDestinations\f01b4d95cf55d32a.automaticDestinations-ms` | «Быстрый доступ» |
| `windows.sticky-notes` | `{LOCALAPPDATA}\Packages\Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe\LocalState\plum.sqlite` | `app_data` |
| `windows.snipping-screenshots` | `{PICTURES}\Screenshots` | `user_files` |
| `windows.powertoys` | `{LOCALAPPDATA}\Microsoft\PowerToys` | `app_config`, include `**/settings.json` |

#### 4.7.8 `rules/cloud-claims.yaml` — только `claims`, без находок
OneDrive, Dropbox, Google Drive, iCloud, Yandex.Disk: локальные кэши/служебные папки (`{LOCALAPPDATA}\Microsoft\OneDrive`, `{LOCALAPPDATA}\Dropbox`, `{LOCALAPPDATA}\Google\DriveFS`) помечаются как объяснённые. Содержимое синхронизируемых папок сохранять не нужно (оно в облаке), но SPEC-07 помечает его тегом `cloud-synced`.

#### 4.7.9 `rules/games-extra.yaml` и `rules/emulators.yaml` (дополнение к SPEC-05)
| id | Путь | Категория |
|---|---|---|
| `steam.userdata-config` | `{STEAM}\userdata\{STEAM_USERID}\config` | `game_config` (localconfig.vdf, скриншоты-мета, контроллеры) |
| `steam.screenshots` | `{STEAM}\userdata\{STEAM_USERID}\760\remote` | `user_files` |
| `minecraft.java` | `{APPDATA}\.minecraft` | `game_save`; include `saves/**, options.txt, servers.dat, resourcepacks/**, shaderpacks/**, screenshots/**, mods/**`; claims `versions, libraries, assets` (reinstallable) |
| `prismlauncher.instances` | `{APPDATA}\PrismLauncher\instances` | `game_save` |
| `retroarch.saves` | `{APPDATA}\RetroArch` + `{DRIVE:*}\RetroArch` через `installed` | `game_save`; include `saves/**, states/**, config/**, retroarch.cfg`; claims `cores, system (bios — предупреждение)` |
| `dolphin.user` | `{DOCUMENTS}\Dolphin Emulator`, `{APPDATA}\Dolphin Emulator` | `game_save`; include `GC/**, Wii/**, StateSaves/**, Config/**` |
| `pcsx2.user` | `{DOCUMENTS}\PCSX2` | `memcards/**, sstates/**, inis/**` |
| `ppsspp.user` | `{DOCUMENTS}\PPSSPP\PSP\SAVEDATA`, `...\PPSSPP_STATE`, `...\SYSTEM` | |
| `yuzu-ryujinx.user` | `{APPDATA}\yuzu\nand\user\save`, `{APPDATA}\Ryujinx\bis\user\save` | keys → sensitivity high, предупреждение |
| `duckstation.user` | `{DOCUMENTS}\DuckStation` | `memcards/**, savestates/**, settings.ini` |
| `rpcs3.user` | `{GAME_DIR}` — через `installed`, `dev_hdd0\home\*\savedata` | |
| `cemu.user` | `{APPDATA}\Cemu\mlc01\usr\save` | |

Итого в стартовой базе ≈ 70 правил. Минимум для закрытия спеки — все правила из таблиц §4.7.1–§4.7.9.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Пользовательский YAML битый | `ScanIssue::Warning { source: "rules", message_key: "issue.rules.invalid_file", args: {file (только имя файла), line?, rule_id?, error} }`, файл пропущен целиком. |
| Два пользовательских правила с одинаковым id | Побеждает последнее по алфавиту файлов + `ScanIssue::Warning issue.rules.duplicate_id {rule_id, file, previous_file}`. |
| Каталог `rules.d` существует, но не читается | Пользовательские правила пропущены, `ScanIssue::Warning issue.rules.user_dir_unreadable {error}`. |
| Правило ссылается на `{STEAM}`, а Steam не установлен | resolve пустой → находки нет, без issue. |
| `glob_root` дал > 50 совпадений | Берём первые 50 по mtime desc + warning (защита от патологий). |
| `from_json`: файл битый, не тот формат или схема программы изменилась (`select` ничего не нашёл) | `ScanIssue::Warning issue.rules.from_json_parse` (с номером строки, если есть) / `Info issue.rules.from_json_empty`. Остальные targets правила работают. |
| `from_json`: значение указывает на отключённый внешний диск | Путь не существует → пропуск + Info с шаблоном пути («vault на диске E:, диск не подключён»). |
| Target — файл, а include задан | include игнорируется, warning при валидации. |
| HKLM/HKCU-ветка без прав на чтение | `registry_exists` = false, `ScanIssue::Info issue.rules.registry_access_denied {rule_id, hive, key}`, один раз на ключ за скан. |
| Некорректный regex в `display_name_regex`/`file_contains.pattern` (правило не прошло через компиляцию §4.4) | Условие ложно, `ScanIssue::Warning issue.rules.invalid_regex {rule_id, pattern, error}`, один раз на шаблон за скан. |
| Портативная программа в нестандартной папке (Notepad++ portable) | Не покрывается правилами → SPEC-07. |
| Путь внутри глобального исключения SPEC-03 (`{PROGRAMFILES_X86}\MSI Afterburner\Profiles`) | Разрешено как явный корень (SPEC-03 §4.5 `explicit_root`). |

## 6. Тестирование

- Unit: парсинг схемы (все поля, `deny_unknown_fields`), валидация каждого правила из §4.4 (позитив/негатив), слияние builtin+user (replace, disable), приоритеты при конфликте FindingId.
- **Тест встроенной базы** (`builtin_rules_valid`): все файлы `rules/*.yaml` компилируются, id уникальны, `credentials` ⇒ `high`. Ломает CI (FR-04-07).
- Collector на `MemFs` + `Environment::fake`: фикстура `fixtures/fs/profile-typical.yaml` (VS Code, Chrome с 2 профилями, Firefox, .ssh, OBS, Telegram) → ожидаемый snapshot находок (`insta`), включая `claimed_paths`.
- `glob_root` для JetBrains с 3 версиями IDE → 3 находки.
- `from_json`: фикстуры `obsidian.json` (2 vault'а, один на несуществующем диске, один как `file:///`), `projects-v1.json` Unity Hub, JSONC с комментариями и висячей запятой, `*` в массиве и объекте, экранирование `~1`, файл > 1 MiB, относительный путь, UNC (отброшен) → ожидаемые находки и issues.
- Стабильность id: тот же `obsidian.json` на `Environment::fake` с другим именем пользователя → те же `FindingId`.
- `conditions`: `installed` по regex на фейковом `installed_programs`.
- Windows-интеграционный: `registry_exists` на реальной ветке `HKCU\Software\Microsoft`.
- Бенч: 500 синтетических правил → NFR-04-01.

## 7. Задачи

- [x] **T-04-01** — Serde-модель схемы v1 (§4.2, §4.3) с `deny_unknown_fields`. *Зависит:* T-02-01. *Готово, когда:* пример из §4.2 парсится.
- [x] **T-04-02** — Компиляция и валидация (§4.4), `RuleDiagnostic` с номерами строк (позиции из ошибок `serde-saphyr`). *Зависит:* T-04-01, T-02-03.
- [x] **T-04-03** — Загрузка builtin (`include_dir!`) + `rules.d`, слияние, disable. *Зависит:* T-04-02, T-01-04.
- [ ] **T-04-04** — Оценщик условий с кэшем на скан; `process_running` по `Environment.running_processes`. *Зависит:* T-04-02, T-03-01, T-02-05.
- [ ] **T-04-05** — Раскрытие targets, `glob_root`, создание `Finding` и `claimed_paths`. *Зависит:* T-04-04. *Готово, когда:* snapshot-тест profile-typical.
- [ ] **T-04-06** — `RulesCollector: Collector`, прогресс, конфликты FindingId. *Зависит:* T-04-05, T-01-03.
- [ ] **T-04-12** — Target `from_json` (§4.2.1): JSONC-препроцессор, вычисление `select` с `*`, нормализация и фильтры путей, создание находок и `claimed_paths`. *Зависит:* T-04-05, T-03-06. *Готово, когда:* тесты из §6 по `from_json` проходят.
- [ ] **T-04-07** — YAML-правила §4.7.1–§4.7.2 (браузеры, dev, включая `unityhub.projects`). *Зависит:* T-04-02, T-04-12. *Готово, когда:* проверены вручную на Windows-машине разработчика (чек-лист в PR); ручная проверка критерия SPEC-01 §8: Ctrl+C во время `savekeeper-cli scan` по реальному профилю завершает процесс за ≤ 1 с с кодом 2 (перенесено из SPEC-03 T-03-04: первый скан, который идёт по реальным файлам); ручная проверка критериев SPEC-03 §8 на том же скане: OneDrive-папка «только онлайн» после скана остаётся с облачным значком (файлы не гидрированы), junction'ы профиля (`Application Data` и т.д.) не дают двойного счёта размеров.
- [ ] **T-04-08** — YAML-правила §4.7.3–§4.7.7 (включая `obsidian.vaults`). *Зависит:* T-04-02, T-04-12.
- [ ] **T-04-09** — YAML-правила §4.7.8–§4.7.9. *Зависит:* T-04-02, T-05-03 (токены Steam).
- [ ] **T-04-10** — CLI `rules validate` + вывод диагностики. *Зависит:* T-04-02, T-01-07.
- [ ] **T-04-11** — i18n-ключи для всех `title_key`/`notes_key` (ru, en) в `app/src/i18n/*.json` (генерация списка ключей тестом). *Зависит:* T-04-07..09.

## 8. Критерии приёмки

- [ ] Все правила §4.7 присутствуют, валидны и покрыты snapshot-тестом хотя бы по группам.
- [ ] На машине разработчика находки из правил покрывают все установленные программы из списка §4.7.
- [ ] Пользовательское правило в `rules.d` подхватывается без перезапуска сборки, отключение встроенного работает.
- [ ] NFR-04-01 выполнен.

## 9. Открытые вопросы

- ~~Динамические пути из конфигов~~ **Решено (2026-10-01):** `from_json` входит в схему v1 (§4.2.1), только JSON/JSONC. Кандидаты на будущие форматы: INI (папка записей OBS в `basic.ini`, qBittorrent), XML.
- ~~`notes_key` у `Finding`, токен `{DRIVE:*}`~~ **Приняты** в SPEC-02 (§2.1, §3.1).
- ~~Токен `{PACKAGE:<name>}`~~ **Принят** в SPEC-02 §3.1. Правила для Store-приложений переписываются на него при реализации T-04-08.
- ~~Snapshot процессов~~ **Принято:** `Environment.running_processes` (SPEC-02 §3.3), заполняется в `sk-core` (`CreateToolhelp32Snapshot`) в рамках T-02-05.
