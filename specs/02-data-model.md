# SPEC-02: Модель данных

| Поле | Значение |
|---|---|
| ID | SPEC-02 |
| Статус | in-progress |
| Фаза | P0 |
| Крейт(ы) | `sk-core` |
| Зависит от | SPEC-00 |
| Используется в | все спеки |
| Последнее изменение | 2026-10-02 (§2.7: API `FindingId::for_target`, уточнения формулы; §4.2: API `sk-core::privacy`, правила замен, `machine_name`; §3.1–§3.3: синтаксис шаблонов, `Token`, `TemplateError`, правила `resolve`/`from_path`, токены `{STORE_GAME_ID}`/`{GAME_DIR_NAME}` в таблице, `Environment.store_packages` для `{PACKAGE:…}`; §3.3: `Environment::known_folder`, `KnownFolder::ALL/token/from_token`; §3.1, §3.3: OneDrive-корни заполняет `detect()`, `EnvError`, раскладка `fake`, правила для дисков и процессов, состав T-02-05; §5: API `sk-core::path` и `PathSet`; §8, T-02-09: снапшот контракта — TS-декларации вместо JSON Schema, `u64` → `number`; T-02-09: `xtask bindings`, зависит от T-11-01; определены `OsInfo`, `KnownFolder`, `DriveSnapshot`, `LauncherSnapshot`, `ScanOptionsSnapshot`, `CollectorToggles`, `LlmMode`; обязательные Known Folders; состав и зависимости T-02-01/05/08) |

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

```rust
impl FindingId {
    pub fn for_target(target: &Target) -> FindingId;   // по формуле выше
    pub fn as_str(&self) -> &str;
}
```
- `File` использует ту же формулу с пустыми `include`/`exclude`: `"fs:" + шаблон + "||"`.
- `hive` — имя из JSON (`hkcu`, `hklm`). `lowercase` — Unicode `to_lowercase`.
- `canonical_json` — компактный JSON с рекурсивно отсортированными ключами объектов, не зависящий от feature `preserve_order` у `serde_json`.
- Формула — часть контракта: изменение меняет id всех находок и ломает сопоставление со старыми бэкапами (SPEC-13). Значения id для эталонных целей закреплены тестом.

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
| `{ONEDRIVE}` | `Environment.cloud_roots`: корень `OneDrive`, иначе первый `OneDriveBusiness` (§3.3) | может отсутствовать |
| `{STEAM}` | `root` лаунчера `steam` из `Environment.launchers` (SPEC-05) | `C:\Program Files (x86)\Steam` |
| `{STEAM_USERID}` | id3 пользователя Steam (SPEC-05), может раскрываться в несколько | `12345678` |
| `{GAME_DIR}` | каталог установки конкретной игры, `ResolveContext.game_dir` (SPEC-05) | контекстный |
| `{STORE_GAME_ID}` | `ResolveContext.store_game_id` (`<storeGameId>` Ludusavi, §3.2) | контекстный, значение |
| `{GAME_DIR_NAME}` | `ResolveContext.game_dir_name` (`<game>` Ludusavi, §3.2) | контекстный, значение |
| `{DRIVE:X}` | корень диска `X:\`, если диск есть в `Environment.drives` | для эвристик на других дисках |
| `{DRIVE:*}` | корни **всех** фиксированных дисков (мульти-значный) | портативные эмуляторы и т.п. (SPEC-04) |
| `{PACKAGE:<name>}` | `{LOCALAPPDATA}\Packages\<pfn>` для каждого `pfn` из `Environment.store_packages` вида `<name>_<publisherId>` (мульти-значный, сравнение имени регистронезависимое) | MS Store/UWP-пакеты (SPEC-04) |

Токены регистрозависимы и пишутся в верхнем регистре. Разделитель в шаблоне всегда `\`.

Синтаксис (проверяет `parse`):
- Токен — `{NAME}` или `{NAME:arg}`, где `NAME` = `[A-Z][A-Z0-9_]*`. Скобки с другим содержимым — обычный текст: папки с GUID вроде `{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}` не считаются токенами. Неизвестный `NAME` — ошибка.
- **Корневые** токены (папки из таблицы до `{PROGRAMFILES_X86}`, `{ONEDRIVE}`, `{STEAM}`, `{GAME_DIR}`, `{DRIVE:…}`, `{PACKAGE:…}`) стоят только целым первым сегментом. **Значения** (`{STEAM_USERID}`, `{STORE_GAME_ID}`, `{GAME_DIR_NAME}`) могут быть частью любого сегмента, кроме первого.
- `{DRIVE:X}`: одна латинская буква в верхнем регистре или `*`. `{PACKAGE:name}`: непустое имя из `[A-Za-z0-9.-]`.
- `*` в обычных сегментах допустим (glob-сегменты `glob_root`, SPEC-04), `resolve` оставляет его как есть.
- Нормализация: `/` → `\`, повторные разделители схлопываются, завершающий `\` убирается. Сегменты `.` и `..` — ошибка. Шаблон без корневого токена (например UNC-путь из `from_path`) допустим.

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
// Deserialize проверяет строку через parse; Display выводит as_str().

pub enum Token {
    Folder(KnownFolder),                    // {HOME}, {APPDATA} ...
    OneDrive, Steam, GameDir,
    Drive(char),                            // {DRIVE:X}
    AllDrives,                              // {DRIVE:*}
    Package(String),                        // {PACKAGE:name}
    SteamUserId, StoreGameId, GameDirName,  // значения
}

#[derive(thiserror::Error, Debug)]
pub enum TemplateError {
    Empty,
    UnknownToken(String),                   // {APPDTA}
    Unclosed(usize),                        // `{` без `}`, байтовая позиция
    MisplacedToken(String),                 // корневой токен не первым сегментом или значение в первом
    InvalidArgument(String),                // {DRIVE:cd}, {PACKAGE:}
    DotSegment,                             // `.` или `..`
}

#[derive(Debug, Clone, Default)]
pub struct ResolveContext {
    pub game_dir: Option<PathBuf>,          // {GAME_DIR}
    pub steam_user_ids: Vec<String>,        // {STEAM_USERID}
    pub store_game_id: Option<String>,      // <storeGameId> из манифеста Ludusavi (SPEC-05)
    pub game_dir_name: Option<String>,      // <game> из манифеста Ludusavi (SPEC-05)
}
```

- `from_path` используется для обезличивания (логи, LLM, манифест бэкапа). Сравнение префиксов регистронезависимое, по компонентам пути, а не по строке (`C:\Users\maxim` не является потомком `C:\Users\max`).
- При отсутствии токена в окружении (`{ONEDRIVE}` нет) `resolve` возвращает пустой список, не ошибку.
- `resolve`: декартово произведение значений всех токенов (`{DRIVE:*}` × `{STEAM_USERID}`), порядок — порядок значений в `Environment`/`ResolveContext`. `{DRIVE:*}` — диски `Fixed`.
- `from_path`: кандидаты — пути `known_folders`, `{ONEDRIVE}`, `{STEAM}` и корни дисков `Environment.drives`; выбирается самый длинный по числу компонентов, при равенстве — в этом порядке. Если результат `{LOCALAPPDATA}\Packages\<name>_<publisherId>\…` (publisherId — 13 символов `[a-z0-9]`), он записывается как `{PACKAGE:name}\…`. Путь вне кандидатов с буквой диска даёт `{DRIVE:X}\…`, иначе (UNC, относительный) — сам путь. Регистр хвоста сохраняется.
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
    pub cloud_roots: Vec<CloudRoot>,    // OneDrive — detect() (§3.3); Dropbox, Google Drive, Yandex.Disk — sk-scan::cloud::detect_roots (SPEC-03)
    pub launchers: Vec<LauncherInfo>,   // заполняет SPEC-05
    pub installed_programs: Vec<InstalledProgram>, // заполняет SPEC-06 §4.4 (Uninstall-ключи HKLM+HKCU+WOW6432Node, MSIX)
    pub running_processes: Vec<String>, // lowercase имена exe, снимок в фазе Environment (SPEC-04 условия, SPEC-10 locked-подсказки)
    pub store_packages: Vec<String>,    // имена папок {LOCALAPPDATA}\Packages (package family names), для {PACKAGE:…}
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
- `cloud_roots` для OneDrive заполняет `Environment::detect()`, потому что от них зависят `{ONEDRIVE}` и `EnvironmentSnapshot` в `sk-core`: `OneDrive` — `HKCU\Software\Microsoft\OneDrive\Accounts\Personal\UserFolder` (если нет — `HKCU\Software\Microsoft\OneDrive\UserFolder`), `OneDriveBusiness` — `UserFolder` каждого `Accounts\Business<N>`. Несуществующие на диске пути пропускаются. Остальных провайдеров добавляет `sk-scan::cloud::detect_roots(env)` через обогатитель (конец §3.3).
- `drives`: том запрашивается (`GetVolumeInformationW`, `GetDiskFreeSpaceExW`, тип носителя) только для `Fixed` и `Removable`; у сетевых и CD-дисков заполняются только `letter` и `kind`, чтобы отключённый сетевой диск не блокировал скан.
- `running_processes`: снимок `CreateToolhelp32Snapshot`, имена exe в lowercase, без дублей, отсортированы.
- `store_packages`: имена подкаталогов `{LOCALAPPDATA}\Packages` как есть, отсортированы. Только имена, без обхода вглубь. В `fake()` список пуст.
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
    pub fn known_folder(&self, folder: KnownFolder) -> Option<&Path>;
}

impl KnownFolder {
    pub const ALL: [KnownFolder; 16];                   // в порядке объявления
    pub fn token(self) -> &'static str;                 // имя токена без скобок: "SAVED_GAMES"
    pub fn from_token(token: &str) -> Option<Self>;     // регистрозависимо, как токены §3.1
}

#[derive(thiserror::Error, Debug)]
pub enum EnvError {
    /// Не удалось получить обязательные данные (профиль, имя пользователя).
    Os { api: &'static str, code: i32 },
    /// Обязательная Known Folder `Home` недоступна.
    MissingHome,
}
```
- Отсутствие необязательных папок, OneDrive, сведений о дисках и процессах — не ошибка: поле остаётся пустым.
- Раскладка `fake(root)` (как у Windows, относительно `root`): `{HOME}` = `Users\user`; `{APPDATA}`, `{LOCALAPPDATA}`, `{LOCALLOW}` = `{HOME}\AppData\Roaming|Local|LocalLow`; `Documents`, `Desktop`, `Pictures`, `Music`, `Videos`, `Downloads`, `Saved Games` под `{HOME}`; `{PUBLIC}` = `Users\Public`, `{PROGRAMDATA}` = `ProgramData`, `{WINDIR}` = `Windows`, `{PROGRAMFILES}` = `Program Files`, `{PROGRAMFILES_X86}` = `Program Files (x86)`. `user_name = "user"`, `machine_name = "FAKE-PC"`, ОС «Windows 11 Pro» 26100, один диск `C` (Fixed, Ssd, NTFS), остальные списки пусты. Папки на диске не создаются (это делает `FakeProfile`, SPEC-12).
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

```rust
// sk-core::privacy
pub const REDACTED: &str = "<redacted>";
/// Заменяет на REDACTED: email, токеноподобные строки, user_name и machine_name из env.
pub fn redact(text: &str, env: &Environment) -> String;
/// Обезличенный путь для логов и issue: PathTemplate::from_path + redact.
pub fn redact_path(path: &Path, env: &Environment) -> String;
```
- Порядок замен: email → токены → имена. Email — `local@domain.tld` (local из `[A-Za-z0-9._%+-]`, домен из `[A-Za-z0-9.-]` с точкой и буквенной зоной ≥ 2). Токен — непрерывная серия ≥ 24 символов `[A-Za-z0-9_-]` (GUID тоже попадает).
- `user_name` и `machine_name` ищутся без учёта регистра как отдельное слово: соседние символы не буквы и не цифры. Имена короче 2 символов не заменяются.

## 5. Пути и кодировки
- Внутри программы используются `PathBuf`. Для JSON: `String` через `to_string_lossy()`. Если была потеря, добавляется тег `non_utf8_path` к находке.
- Длинные пути: функция `sk-core::path::to_extended(&Path) -> PathBuf` добавляет `\\?\` (или `\\?\UNC\`) для всех операций ФС в `sk-scan`/`sk-backup`.
- Сравнение путей регистронезависимое (на всех ОС, чтобы кроссплатформенные тесты вели себя как Windows): `sk-core::path::eq_ci`, `starts_with_ci` (по компонентам, с Unicode case folding через `to_lowercase`). Сравнение лексическое, без обращения к ФС; префикс `\\?\` игнорируется, `..` не раскрывается.
- `PathSet` — множество путей в виде префиксного дерева компонентов, с тем же сравнением:

```rust
// sk-core::path
pub fn to_extended(path: &Path) -> PathBuf;
pub fn eq_ci(a: &Path, b: &Path) -> bool;
pub fn starts_with_ci(path: &Path, base: &Path) -> bool;   // path == base или под ним

#[derive(Debug, Clone, Default)]
pub struct PathSet { /* trie */ }
impl PathSet {
    pub fn new() -> Self;
    pub fn insert(&mut self, path: impl Into<PathBuf>) -> bool;  // false, если равный путь уже есть
    pub fn contains(&self, path: &Path) -> bool;                 // точное совпадение
    pub fn covers(&self, path: &Path) -> bool;                   // path — элемент или лежит под элементом
    pub fn has_descendant(&self, path: &Path) -> bool;           // есть элемент строго под path
    pub fn descendants(&self, path: &Path) -> Vec<&Path>;        // элементы строго под path (SPEC-07 §4.2.4)
    pub fn iter(&self) -> impl Iterator<Item = &Path>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
impl<P: Into<PathBuf>> FromIterator<P> for PathSet;
impl<P: Into<PathBuf>> Extend<P> for PathSet;
```

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

- Round-trip serde (JSON) для всех типов + insta-снапшоты JSON-примеров. Схема контракта для UI — снапшот TS-деклараций, которые `specta-typescript` генерирует в T-02-09 (JSON Schema через specta не используется: `specta-jsonschema` незрелый).
- `FindingId` стабилен: одинаковый для двух `Environment::fake` с разными корнями и именами пользователей.
- `PathTemplate::from_path` выбирает самый специфичный токен: `{LOCALLOW}` вместо `{HOME}\AppData\LocalLow`.
- Покомпонентное сравнение: `C:\Users\maxim` не начинается с `C:\Users\max`.
- `redact()`: email, токены, имя пользователя.
- Windows-only: `Environment::detect()` возвращает обязательные Known Folders (`Home`, `AppData`, `LocalAppData`, `LocalLow`, `Documents`, `ProgramData`, `WinDir`, `ProgramFiles`); `{DOCUMENTS}` совпадает с `SHGetKnownFolderPath`.

## 9. Задачи

- [x] **T-02-01** — Модуль `sk-core::model`: `Finding`, `Target`, `Category`, `AppRef`, `Evidence`, `EvidenceSource`, `TargetStats`, `Sensitivity`, `Score`, `ScanIssue`, а также `CollectorToggles` и `LlmMode` из §6 (нужны конфигу, T-01-04) + serde/specta derive. *Готово, когда:* round-trip тесты и insta-снапшот.
- [x] **T-02-02** — `sk-core::path`: `to_extended`, `eq_ci`, `starts_with_ci` (покомпонентно), `PathSet` (префиксное дерево, используется SPEC-01). *Готово, когда:* тесты §8.
- [x] **T-02-03** — `PathTemplate`: синтаксис §3.1, `Token`, `TemplateError`, parse (+ проверка при десериализации), resolve (включая мульти-значные токены), from_path, поле `Environment.store_packages` и его заполнение в `detect()`. *Зависит:* T-02-02, T-02-05.
- [x] **T-02-04** — `sk-core::privacy::redact` и обезличивание путей для логов. *Зависит:* T-02-03.
- [x] **T-02-05** — `Environment` (+ `OsInfo`, `KnownFolder`, `DriveInfo`, `CloudRoot`, `LauncherInfo`, `InstalledProgram` и вложенные типы §3.3): структура, `fake()`, `detect()` для Windows (`SHGetKnownFolderPath`, `GetUserNameW`, `IsUserAnAdmin`/token elevation, `GetLogicalDrives`+`GetDriveTypeW`+`GetVolumeInformationW`, версия ОС из `RtlGetVersion` / реестра `CurrentVersion`) OneDrive-корни, `running_processes` (`CreateToolhelp32Snapshot`, SPEC-04 §9), тип носителя (`IOCTL_STORAGE_QUERY_PROPERTY`) и заглушка для других ОС. *Готово, когда:* Windows-тест §8.
- [ ] **T-02-06** — `FindingId` по §2.7. *Зависит:* T-02-01, T-02-03.
- [ ] **T-02-07** — `FolderSummary`, `Marker`, `ExtStat`, `ChildStat` (только типы; вычисление в SPEC-03).
- [ ] **T-02-08** — `ScanReport`, `EnvironmentSnapshot`, `DriveSnapshot`, `LauncherSnapshot`, `ScanOptionsSnapshot`, `Totals`, `CategoryTotals` + версионирование. *Зависит:* T-02-01, T-02-03, T-02-05, T-02-07.
- [ ] **T-02-09** — `cargo xtask bindings` (SPEC-12 §4.9): экспорт TS-типов `sk-core` через specta в `app/src/bindings.ts`. В SPEC-11 T-11-02 тот же экспорт дополняется командами и событиями `tauri-specta` (один генератор, один файл). `u64`/`i64` экспортируются как `number` (`BigIntExportBehavior::Number`): размеры и счётчики не превышают 2^53. *Зависит:* T-02-08, T-11-01 (`app/` и `tsconfig` для проверки), T-12-01. *Готово, когда:* файл генерируется и компилируется `tsc`, insta-снапшот TS-деклараций типов `sk-core` зафиксирован.

## 10. Критерии приёмки

- [ ] Все типы §2–§6 реализованы и совпадают со спекой по полям и именам.
- [ ] JSON-снапшоты зафиксированы. Любое изменение контракта видно в диффе снапшота.
- [ ] На Windows с перенаправленными в OneDrive Documents шаблон `{DOCUMENTS}` раскрывается верно.

## 11. Открытые вопросы

- Нужно ли хранить в `Finding` список конкретных файлов? **Решение:** нет, файлы перечисляются лениво при бэкапе (SPEC-10), чтобы не раздувать память и отчёт.
- ~~`{STEAM_USERID}`: id3 или id64?~~ **Решено:** `StoreUser.id` = id3, `alt_id` = id64. `{STEAM_USERID}` раскрывается в id3, для `<storeUserId>` Ludusavi перебираются оба (SPEC-05 §4.3).
- **Слияние и `exclude` (вопрос из SPEC-09):** `exclude`, добавленные при слиянии (родитель исключает поглощённого ребёнка), дописываются в `target.exclude`. `FindingId` вычисляется **один раз при создании** находки коллектором и после слияния не пересчитывается. Так id остаётся стабильным между сканами.
