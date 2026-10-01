# SPEC-02: Модель данных

| Поле | Значение |
|---|---|
| ID | SPEC-02 |
| Статус | approved |
| Фаза | P0 |
| Крейт(ы) | `sk-core` |
| Зависит от | SPEC-00 |
| Используется в | все спеки |
| Последнее изменение | 2026-10-01 (определены `OsInfo`, `KnownFolder`, `DriveSnapshot`, `LauncherSnapshot`, `ScanOptionsSnapshot`, `CollectorToggles`, `LlmMode`; обязательные Known Folders; состав и зависимости T-02-01/05/08) |

## 1. Цель

Единые доменные типы, которыми обмениваются все крейты и UI: находки (`Finding`),
доказательства (`Evidence`), шаблоны путей (`PathTemplate`), окружение (`Environment`),
сводки папок (`FolderSummary`), отчёт скана (`ScanReport`). Изменение этой спеки требует
проверки всех зависимых спек.

## 2. Доменные типы

Все типы: `#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]`,
`#[serde(rename_all = "snake_case")]`. JSON-представление является публичным контрактом
(UI, `scans/*.json`, манифест бэкапа).

### 2.1 Finding

```rust
pub struct Finding {
    pub id: FindingId,                  // §2.7 — стабильный между сканами
    pub target: Target,                 // что сохраняем
    pub category: Category,
    pub app: Option<AppRef>,            // к какому приложению/игре относится
    pub title: String,                  // "Elden Ring — сохранения", "VS Code — настройки"
    pub evidence: Vec<Evidence>,        // ≥ 1 (принцип P3)
    pub stats: Option<TargetStats>,     // заполняется в фазе Measure (SPEC-03)
    pub sensitivity: Sensitivity,
    pub score: Option<Score>,           // заполняется в фазе Score (SPEC-09)
    pub default_selected: bool,         // рекомендация SPEC-09
    pub requires_elevation: bool,       // для бэкапа нужны права админа
    pub tags: Vec<String>,              // свободные метки: "steam", "unity", "cloud-synced"
    pub children: Vec<FindingId>,       // вложенные находки, поглощённые при слиянии (SPEC-09 §merge)
    pub notes_key: Option<String>,      // ключ i18n подсказки: «лучше синхронизировать аккаунтом», «пароли DPAPI не переносятся» (SPEC-04)
}
```

### 2.2 Target — что именно сохраняется

```rust
#[serde(tag = "kind")]
pub enum Target {
    /// Набор файлов под корнем.
    FileSet {
        root: PathTemplate,             // §3
        resolved: PathBuf,              // абсолютный путь на текущей машине
        include: Vec<String>,           // glob относительно root; пусто = "**"
        exclude: Vec<String>,           // glob относительно root
    },
    /// Отдельный файл (частый случай для конфигов).
    File { path: PathTemplate, resolved: PathBuf },
    /// Ветка реестра, экспортируется в .reg.
    Registry { hive: RegHive, key: String, recursive: bool },
    /// Результат выполнения экспортёра (winget export, netsh ...). SPEC-06.
    SystemExport { exporter_id: String, params: serde_json::Value },
}

pub enum RegHive { Hkcu, Hklm }         // HKLM — только чтение, для справки (Uninstall и т.п.)
```

### 2.3 Category

```rust
pub enum Category {
    GameSave,          // сохранения игр
    GameConfig,        // настройки и конфиги игр, моды
    AppConfig,         // настройки программ (маленькие, важные)
    AppData,           // данные программ (базы, профили, локальные библиотеки)
    UserFiles,         // документы, медиа, проекты пользователя вне стандартных мест
    DevEnvironment,    // ssh, git, IDE, репозитории, WSL
    Credentials,       // ключи, сертификаты, менеджеры паролей (всегда sensitivity >= High)
    SystemSettings,    // winget, Wi-Fi, драйверы, шрифты, hosts ...
    Reinstallable,     // восстанавливается переустановкой/скачиванием
    Cache,             // кэш, временные файлы, логи
    Unknown,           // не распознано (кандидат для LLM, SPEC-08)
}
```

Для UI у каждой категории есть: ключ i18n, иконка, цвет, вес по умолчанию для скоринга (SPEC-09 §4).

### 2.4 AppRef

```rust
pub struct AppRef {
    pub id: String,                     // нормализованный slug: "elden-ring", "vscode", "obs-studio"
    pub name: String,                   // "ELDEN RING"
    pub kind: AppKind,                  // Game | Application | System | DevTool
    pub source_ids: BTreeMap<String, String>, // {"steam": "1245620", "ludusavi": "ELDEN RING", "winget": "Microsoft.VisualStudioCode"}
    pub installed: Option<bool>,        // установлено ли сейчас (по Uninstall-ключам или лаунчерам)
    pub process_names: Vec<String>,     // lowercase имена exe ("eldenring.exe") — детект запущенных при бэкапе/восстановлении (SPEC-10, SPEC-13)
}
```
Правило нормализации `id`: lowercase, ASCII-транслитерация, `[^a-z0-9]+` → `-`, trim `-`.

### 2.5 Evidence

```rust
pub struct Evidence {
    pub source: EvidenceSource,
    pub message_key: String,            // ключ i18n, например "evidence.rule_match"
    pub message_args: BTreeMap<String, String>, // аргументы для подстановки
    pub confidence: f32,                // 0.0..=1.0
    pub importance: Option<f32>,        // 0.0..=1.0, оценка «насколько больно потерять» от источника (сейчас только LLM, SPEC-08); вход для SPEC-09
}

#[serde(tag = "kind")]
pub enum EvidenceSource {
    Rule { rule_id: String },                          // SPEC-04
    Ludusavi { game: String, manifest_version: String },// SPEC-05
    Launcher { launcher: String },                     // SPEC-05 (steam, epic, gog ...)
    System { exporter_id: String },                    // SPEC-06
    Heuristic { heuristic_id: String },                // SPEC-07
    Llm { provider: String, model: String },           // SPEC-08
    User,                                              // пользователь вручную
}
```
Для LLM `message_key = "evidence.llm"`, а текст объяснения лежит в `message_args["reason"]` (не переводится).

### 2.6 TargetStats, Sensitivity, Score, ScanIssue

```rust
pub struct TargetStats {
    pub total_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub newest_mtime: Option<OffsetDateTime>,
    pub oldest_mtime: Option<OffsetDateTime>,
    pub locked_files: u32,               // файлы, которые не удалось открыть на чтение (SPEC-03)
    pub cloud_only_bytes: u64,           // часть total_bytes, лежащая только в облаке (плейсхолдеры OneDrive и т.п., не гидрируются, SPEC-03 FR-03-03)
    pub cloud_only_files: u64,
    pub largest_file_bytes: Option<u64>, // для проверки лимита FAT32 (SPEC-10 FR-10-04)
    pub truncated: bool,                 // обход прерван лимитом или ошибкой
}

pub enum Sensitivity { None, Low, High }  // High: пароли, ключи, токены — предупреждение в UI, шифрование рекомендуется

pub struct Score {
    pub value: f32,                       // 0.0..=1.0
    pub components: BTreeMap<String, f32>,// "irreplaceability", "user_authored", "recency", "size_penalty" ... (SPEC-09)
}

pub struct ScanIssue {
    pub severity: IssueSeverity,          // Info | Warning | Error
    pub source: String,                   // id коллектора/фазы
    pub path: Option<String>,             // обезличенный (шаблон), если есть
    pub message_key: String,
    pub message_args: BTreeMap<String, String>,
}
```

### 2.7 FindingId — стабильный идентификатор

`FindingId(String)` = первые 16 hex-символов `blake3(canonical_key)`, где `canonical_key`:
- `FileSet`/`File`: `"fs:" + lowercase(normalized template string) + "|" + sorted(include).join(",") + "|" + sorted(exclude).join(",")`
- `Registry`: `"reg:" + hive + "\" + lowercase(key)`
- `SystemExport`: `"sys:" + exporter_id + "|" + canonical_json(params)`

Используем шаблон, а не resolved-путь, поэтому id совпадает на разных машинах и у разных
пользователей. Это нужно для SPEC-13 (restore) и для сравнения сканов.

## 3. Шаблоны путей и окружение

### 3.1 Токены PathTemplate

| Токен | Источник (Windows) | Пример |
|---|---|---|
| `{HOME}` | `FOLDERID_Profile` | `C:\Users\max` |
| `{APPDATA}` | `FOLDERID_RoamingAppData` | `C:\Users\max\AppData\Roaming` |
| `{LOCALAPPDATA}` | `FOLDERID_LocalAppData` | `...\AppData\Local` |
| `{LOCALLOW}` | `FOLDERID_LocalAppDataLow` | `...\AppData\LocalLow` |
| `{DOCUMENTS}` | `FOLDERID_Documents` (**учитывает перенаправление в OneDrive**) | `C:\Users\max\OneDrive\Документы` |
| `{DESKTOP}` | `FOLDERID_Desktop` | |
| `{PICTURES}`, `{MUSIC}`, `{VIDEOS}`, `{DOWNLOADS}` | соответствующие FOLDERID | |
| `{SAVED_GAMES}` | `FOLDERID_SavedGames` | `C:\Users\max\Saved Games` |
| `{PROGRAMDATA}` | `FOLDERID_ProgramData` | `C:\ProgramData` |
| `{PUBLIC}` | `FOLDERID_Public` | `C:\Users\Public` |
| `{WINDIR}` | `FOLDERID_Windows` | `C:\Windows` |
| `{PROGRAMFILES}`, `{PROGRAMFILES_X86}` | FOLDERID_ProgramFiles(X86) | |
| `{ONEDRIVE}` | `HKCU\Software\Microsoft\OneDrive\UserFolder` | может отсутствовать |
| `{STEAM}` | корень Steam (SPEC-05) | `C:\Program Files (x86)\Steam` |
| `{STEAM_USERID}` | id3 пользователя Steam (SPEC-05), может раскрываться в несколько | `12345678` |
| `{GAME_DIR}` | каталог установки конкретной игры (SPEC-05) | контекстный |
| `{DRIVE:X}` | корень диска `X:\` | для эвристик на других дисках |
| `{DRIVE:*}` | корни **всех** фиксированных дисков (мульти-значный) | портативные эмуляторы и т.п. (SPEC-04) |
| `{PACKAGE:<name>}` | `{LOCALAPPDATA}\Packages\<name>_<publisherId>`, publisherId ищется по префиксу имени (мульти-значный) | MS Store/UWP-пакеты (SPEC-04) |

Токены регистрозависимы и пишутся в верхнем регистре. Разделитель в шаблоне всегда `\`.

### 3.2 API

```rust
pub struct PathTemplate(String);        // "{APPDATA}\\Code\\User"

impl PathTemplate {
    pub fn parse(s: &str) -> Result<Self, TemplateError>;   // валидирует токены
    pub fn tokens(&self) -> impl Iterator<Item = Token>;
    /// Раскрывает шаблон. Мульти-значные токены ({STEAM_USERID}) дают несколько путей.
    pub fn resolve(&self, env: &Environment, ctx: &ResolveContext) -> Vec<PathBuf>;
    /// Обратная операция: абсолютный путь → самый специфичный шаблон (выбирается самый длинный совпавший префикс).
    pub fn from_path(path: &Path, env: &Environment) -> PathTemplate;
    pub fn as_str(&self) -> &str;
}

pub struct ResolveContext {
    pub game_dir: Option<PathBuf>,          // {GAME_DIR}
    pub steam_user_ids: Vec<String>,        // {STEAM_USERID}
    pub store_game_id: Option<String>,      // <storeGameId> из манифеста Ludusavi (SPEC-05)
    pub game_dir_name: Option<String>,      // <game> из манифеста Ludusavi (SPEC-05)
}
```

- `from_path` используется для обезличивания (логи, LLM, манифест бэкапа). Сравнение префиксов регистронезависимое, по компонентам пути, а не по строке (`C:\Users\maxim` не является потомком `C:\Users\max`).
- При отсутствии токена в окружении (`{ONEDRIVE}` нет) `resolve` возвращает пустой список, не ошибку.
- **Решение по SPEC-05 §9:** плейсхолдеры Ludusavi `<storeGameId>` и `<game>` **не** подставляются строкой до `parse`, а передаются через `ResolveContext`. Так шаблон остаётся стабильным, а `FindingId` не зависит от имени папки установки. В нашем синтаксисе они записываются как `{STORE_GAME_ID}` и `{GAME_DIR_NAME}` (токены, валидные только в контексте игр).

### 3.3 Environment

```rust
pub struct Environment {
    pub os: OsInfo,                     // версия, build, edition, архитектура, язык UI
    pub machine_name: String,
    pub user_name: String,
    pub user_sid: Option<String>,
    pub is_elevated: bool,
    pub known_folders: BTreeMap<KnownFolder, PathBuf>,
    pub drives: Vec<DriveInfo>,
    pub cloud_roots: Vec<CloudRoot>,    // заполняет sk-scan::cloud::detect_roots (SPEC-03)
    pub launchers: Vec<LauncherInfo>,   // заполняет SPEC-05
    pub installed_programs: Vec<InstalledProgram>, // заполняет SPEC-06 §4.4 (Uninstall-ключи HKLM+HKCU+WOW6432Node, MSIX)
    pub running_processes: Vec<String>, // lowercase имена exe, снимок в фазе Environment (SPEC-04 условия, SPEC-10 locked-подсказки)
}

pub struct DriveInfo {
    pub letter: char,
    pub kind: DriveKind,                // Fixed | Removable | Network | CdRom | Unknown
    pub media: DriveMedia,              // Ssd | Hdd | Unknown (IOCTL_STORAGE_QUERY_PROPERTY, SPEC-03 NFR)
    pub fs: Option<String>,             // "NTFS", "exFAT", "FAT32"
    pub label: Option<String>,
    pub volume_serial: Option<u32>,     // для детекта смены буквы диска при восстановлении (SPEC-13)
    pub total_bytes: u64,
    pub free_bytes: u64,
}

pub struct CloudRoot { pub provider: CloudProvider, pub path: PathBuf } // OneDrive | OneDriveBusiness | Dropbox | GoogleDrive | YandexDisk | ICloud | Other(String)

pub struct OsInfo {
    pub product: String,                // "Windows 11 Pro" (редакция входит в строку)
    pub display_version: Option<String>,// "24H2" (DisplayVersion из HKLM\...\CurrentVersion)
    pub build: String,                  // "26100.2033" = CurrentBuildNumber + "." + UBR
    pub arch: String,                   // архитектура ОС (не процесса): "x86_64" | "aarch64" | "x86"
    pub ui_language: String,            // язык интерфейса, BCP-47: "ru-RU"
}

/// Известные папки из §3.1, которые берутся из Known Folders API.
/// Сериализуются именем токена (`"DOCUMENTS"`, `"SAVED_GAMES"`, `"PROGRAMFILES_X86"`) —
/// исключение из правила snake_case §2: так ключи `known_folders` совпадают с токенами шаблонов.
pub enum KnownFolder {
    Home, AppData, LocalAppData, LocalLow, Documents, Desktop, Pictures, Music, Videos, Downloads,
    SavedGames, ProgramData, Public, WinDir, ProgramFiles, ProgramFilesX86,
}
```
- `KnownFolder` ↔ токен ↔ `FOLDERID` — ровно строки таблицы §3.1. Токены из других источников (`{ONEDRIVE}`, `{STEAM}`, `{DRIVE:X}`, `{PACKAGE:...}` и т.п.) в `KnownFolder` не входят.
- `OsInfo.product`: в `ProductName` у Windows 11 записано «Windows 10 …», поэтому при `build ≥ 22000` префикс заменяется на «Windows 11».

Типы окружения, которые **заполняют фичевые крейты**, определяются в `sk-core`, чтобы не было
зависимости `sk-core → sk-games/sk-system`. Поля соответствуют SPEC-05 §4.1 и SPEC-06 §4.4:

```rust
pub struct LauncherInfo { pub id: String, pub root: Option<PathBuf>, pub user_ids: Vec<StoreUser>, pub games: Vec<InstalledGame> }
pub struct StoreUser { pub id: String, pub alt_id: Option<String>, pub name: Option<String> }   // Steam: id = id3, alt_id = id64
pub struct InstalledGame {
    pub store_game_id: String, pub name: String, pub install_dir: PathBuf,
    pub size_bytes: Option<u64>, pub manifest_key: Option<String>,
}
pub struct InstalledProgram {
    pub name: String, pub publisher: Option<String>, pub version: Option<String>,
    pub install_location: Option<PathBuf>, pub install_date: Option<String>,
    pub estimated_size_kb: Option<u64>, pub source: ProgramSource,  // Hklm | Hklm32 | Hkcu | Msix
    pub uninstall_key: String,
}

impl Environment {
    pub fn detect() -> Result<Self, EnvError>;           // Windows: Known Folders API; другие ОС: заглушка из env/home
    pub fn fake(root: &Path) -> Self;                    // для тестов: все папки внутри root (SPEC-12)
}
```
- `Environment::fake` обязателен: на нём строятся все кроссплатформенные тесты.
- `launchers` и `installed_programs` заполняются в фазе `Environment` соответствующими крейтами через функцию-обогатитель `fn enrich(env: &mut Environment)`, чтобы не создавать зависимость `sk-core → sk-games`.

## 4. FolderSummary

Компактное, обезличенное описание папки для эвристик (SPEC-07) и LLM (SPEC-08).

```rust
pub struct FolderSummary {
    pub path: PathTemplate,             // обезличенный путь
    pub total_bytes: u64,
    pub file_count: u64,
    pub dir_count: u64,
    pub max_depth: u32,
    pub newest_mtime: Option<OffsetDateTime>,
    pub oldest_mtime: Option<OffsetDateTime>,
    pub ext_histogram: Vec<ExtStat>,    // top-15 по количеству: {ext, count, bytes}
    pub sample_names: Vec<String>,      // до 20 относительных путей: приоритет — top-level, затем самые свежие, затем разнообразие расширений
    pub top_children: Vec<ChildStat>,   // до 10 крупнейших подпапок: {name, bytes, files}
    pub markers: Vec<Marker>,           // §4.1
    pub truncated: bool,                // упёрлись в лимит обхода
}

pub struct ExtStat { pub ext: String, pub count: u64, pub bytes: u64 } // ext в lowercase без точки, "" — без расширения
pub struct ChildStat { pub name: String, pub bytes: u64, pub files: u64 }
```

### 4.1 Marker — быстрые признаки

```rust
pub enum Marker {
    HasExecutables,      // .exe/.dll/.sys составляют > 30% байт
    GitRepo,             // есть .git
    UnityGame,           // структура LocalLow\<Company>\<Product>, файлы Player.log / output_log.txt
    UnrealSaveGames,     // Saved\SaveGames
    ElectronApp,         // Local Storage, IndexedDB, Cache, GPUCache, Code Cache
    ChromiumProfile,     // "Default\Preferences", "Local State"
    SqliteFiles,         // *.sqlite, *.db с сигнатурой "SQLite format 3"
    CacheLike,           // имя содержит cache/temp/tmp/logs/crash или доминируют .tmp/.log
    ConfigLike,          // доминируют .json/.ini/.xml/.cfg/.toml/.yaml
    MediaHeavy,          // изображения/видео/аудио > 70% байт
    DocumentHeavy,       // office/pdf/txt/md > 50% файлов
    ProjectLike,         // package.json, Cargo.toml, *.sln, *.csproj, pyproject.toml, *.uproject, *.blend
    CloudSynced,         // внутри одного из Environment.cloud_roots
    UwpPackage,          // папка {LOCALAPPDATA}\Packages\<PFN> (MS Store / UWP)
}
```
Правила вычисления маркеров описаны в SPEC-03 §summarize. Список расширяется только через правку этой спеки.

### 4.2 Приватность FolderSummary
- `path` — всегда шаблон (`{LOCALAPPDATA}\Foo`), никогда не абсолютный путь с именем пользователя.
- В `sample_names` имя пользователя, email-подобные строки и строки, похожие на токены (≥ 24 символов `[A-Za-z0-9_-]`), заменяются на `<redacted>` (функция `sk-core::privacy::redact`).

## 5. Пути и кодировки
- Внутри программы используются `PathBuf`. Для JSON: `String` через `to_string_lossy()`. Если была потеря, добавляется тег `non_utf8_path` к находке.
- Длинные пути: функция `sk-core::path::to_extended(&Path) -> PathBuf` добавляет `\\?\` (или `\\?\UNC\`) для всех операций ФС в `sk-scan`/`sk-backup`.
- Сравнение путей на Windows регистронезависимое: `sk-core::path::eq_ci`, `starts_with_ci` (по компонентам, с Unicode case folding через `to_lowercase`).

## 6. ScanReport

```rust
pub struct ScanReport {
    pub schema_version: u32,            // = 1
    pub scan_id: Uuid,
    pub app_version: String,
    pub started_at: OffsetDateTime,
    pub finished_at: OffsetDateTime,
    pub environment: EnvironmentSnapshot, // обезличенная копия Environment (без путей с именем пользователя, только шаблоны и OS info)
    pub options: ScanOptionsSnapshot,
    pub findings: Vec<Finding>,         // отсортированы: category order, затем score desc
    pub unknown_summaries: Vec<FolderSummary>, // папки, которые остались Unknown (для UI «Не распознано»)
    pub issues: Vec<ScanIssue>,
    pub totals: Totals,                 // SPEC-09 §4.9
}

pub struct Totals {
    pub by_category: BTreeMap<Category, CategoryTotals>,
    pub all: CategoryTotals,
    pub sensitive_selected: u32,
    pub unknown_count: u32,
    pub needs_elevation_count: u32,
    pub too_large_count: u32,
}
pub struct CategoryTotals { pub count: u32, pub bytes: u64, pub selected_count: u32, pub selected_bytes: u64 }

/// Обезличенная копия Environment. Используется в ScanReport и в манифесте бэкапа (SPEC-10 §4.4), на неё опирается SPEC-13.
pub struct EnvironmentSnapshot {
    pub os: OsInfo,                                  // product, build, arch, ui_language
    pub machine_name: String,
    pub known_folders: BTreeMap<KnownFolder, PathTemplate>, // относительно {HOME}/{ONEDRIVE}/{DRIVE:X}, без имени пользователя
    pub drives: Vec<DriveSnapshot>,                  // letter, kind, fs, label, volume_serial (без свободного места)
    pub launchers: Vec<LauncherSnapshot>,            // id, root как шаблон, число игр
    pub is_elevated: bool,
}

pub struct DriveSnapshot {
    pub letter: char,
    pub kind: DriveKind,
    pub fs: Option<String>,
    pub label: Option<String>,
    pub volume_serial: Option<u32>,     // SPEC-13: детект смены буквы диска (DifferentDriveLetter)
}

pub struct LauncherSnapshot {
    pub id: String,                     // "steam", "epic" ...
    pub root: Option<PathTemplate>,     // PathTemplate::from_path(LauncherInfo.root)
    pub game_count: u32,                // LauncherInfo.games.len()
}

/// Параметры скана, с которыми построен отчёт (обезличенная копия ScanOptions, SPEC-01 §4.4).
pub struct ScanOptionsSnapshot {
    pub roots: Vec<PathTemplate>,       // ScanOptions.roots через PathTemplate::from_path
    pub collectors: CollectorToggles,
    pub llm: LlmMode,
    pub max_depth: Option<u32>,
}

/// Вкл/выкл коллекторов по их id (SPEC-01 §4.3). По умолчанию все true.
pub struct CollectorToggles { pub rules: bool, pub games: bool, pub system: bool, pub heuristics: bool }

/// Режим LLM-классификации: значение `llm.mode` конфига (SPEC-01 §4.8.2) и `ScanOptions.llm` (SPEC-08 FR-08-01).
pub enum LlmMode { Off, Local, Cloud }  // по умолчанию Off
```

- Сохраняется в `savekeeper-data/scans/<scan_id>.json` (SPEC-01 §4.8.1).
- Совместимость: читатель принимает `schema_version <= current`. Для новых полей используется `#[serde(default)]`.

## 7. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Неизвестный токен в шаблоне | `TemplateError::UnknownToken`. Для правил это ошибка валидации (SPEC-04). |
| Known Folder не существует (удалён, сетевой) | Нет в `known_folders`. Шаблоны с ним дают пустой resolve + `ScanIssue::Info`. |
| Documents перенаправлены на другой диск или в OneDrive | Берём фактический путь из API. Находки получают тег `cloud-synced`, если путь внутри `{ONEDRIVE}`. |
| Имя пользователя с кириллицей или пробелами | Работает (UTF-16 → OsString). Покрыто тестом. |
| Путь без UTF-8 представления | Тег `non_utf8_path`, lossy строка в JSON, операции идут по `PathBuf`. |

## 8. Тестирование

- Round-trip serde (JSON) для всех типов + snapshot-тест JSON-схемы через `insta` (контракт для UI).
- `FindingId` стабилен: одинаковый для двух `Environment::fake` с разными корнями и именами пользователей.
- `PathTemplate::from_path` выбирает самый специфичный токен: `{LOCALLOW}` вместо `{HOME}\AppData\LocalLow`.
- Покомпонентное сравнение: `C:\Users\maxim` не начинается с `C:\Users\max`.
- `redact()`: email, токены, имя пользователя.
- Windows-only: `Environment::detect()` возвращает обязательные Known Folders (`Home`, `AppData`, `LocalAppData`, `LocalLow`, `Documents`, `ProgramData`, `WinDir`, `ProgramFiles`); `{DOCUMENTS}` совпадает с `SHGetKnownFolderPath`.

## 9. Задачи

- [ ] **T-02-01** — Модуль `sk-core::model`: `Finding`, `Target`, `Category`, `AppRef`, `Evidence`, `EvidenceSource`, `TargetStats`, `Sensitivity`, `Score`, `ScanIssue`, а также `CollectorToggles` и `LlmMode` из §6 (нужны конфигу, T-01-04) + serde/specta derive. *Готово, когда:* round-trip тесты и insta-снапшот.
- [ ] **T-02-02** — `sk-core::path`: `to_extended`, `eq_ci`, `starts_with_ci` (покомпонентно), `PathSet` (префиксное дерево, используется SPEC-01). *Готово, когда:* тесты §8.
- [ ] **T-02-03** — `PathTemplate`: parse, resolve (включая мульти-значные токены), from_path. *Зависит:* T-02-02, T-02-05.
- [ ] **T-02-04** — `sk-core::privacy::redact` и обезличивание путей для логов. *Зависит:* T-02-03.
- [ ] **T-02-05** — `Environment` (+ `OsInfo`, `KnownFolder`, `DriveInfo`, `CloudRoot`, `LauncherInfo`, `InstalledProgram` и вложенные типы §3.3): структура, `fake()`, `detect()` для Windows (`SHGetKnownFolderPath`, `GetUserNameW`, `IsUserAnAdmin`/token elevation, `GetLogicalDrives`+`GetDriveTypeW`+`GetVolumeInformationW`, версия ОС из `RtlGetVersion` / реестра `CurrentVersion`) и заглушка для других ОС. *Готово, когда:* Windows-тест §8.
- [ ] **T-02-06** — `FindingId` по §2.7. *Зависит:* T-02-01, T-02-03.
- [ ] **T-02-07** — `FolderSummary`, `Marker`, `ExtStat`, `ChildStat` (только типы; вычисление в SPEC-03).
- [ ] **T-02-08** — `ScanReport`, `EnvironmentSnapshot`, `DriveSnapshot`, `LauncherSnapshot`, `ScanOptionsSnapshot`, `Totals`, `CategoryTotals` + версионирование. *Зависит:* T-02-01, T-02-03, T-02-05, T-02-07.
- [ ] **T-02-09** — Экспорт TS-типов через specta в `app/src/bindings.ts` (скрипт `cargo run -p sk-core --example export-types` или build step в `app/src-tauri`). *Готово, когда:* файл генерируется и компилируется `tsc`.

## 10. Критерии приёмки

- [ ] Все типы §2–§6 реализованы и совпадают со спекой по полям и именам.
- [ ] JSON-снапшоты зафиксированы. Любое изменение контракта видно в диффе снапшота.
- [ ] На Windows с перенаправленными в OneDrive Documents шаблон `{DOCUMENTS}` раскрывается верно.

## 11. Открытые вопросы

- Нужно ли хранить в `Finding` список конкретных файлов? **Решение:** нет, файлы перечисляются лениво при бэкапе (SPEC-10), чтобы не раздувать память и отчёт.
- ~~`{STEAM_USERID}`: id3 или id64?~~ **Решено:** `StoreUser.id` = id3, `alt_id` = id64. `{STEAM_USERID}` раскрывается в id3, для `<storeUserId>` Ludusavi перебираются оба (SPEC-05 §4.3).
- **Слияние и `exclude` (вопрос из SPEC-09):** `exclude`, добавленные при слиянии (родитель исключает поглощённого ребёнка), дописываются в `target.exclude`. `FindingId` вычисляется **один раз при создании** находки коллектором и после слияния не пересчитывается. Так id остаётся стабильным между сканами.
