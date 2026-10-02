# SPEC-03: Сканер файловой системы

| Поле | Значение |
|---|---|
| ID | SPEC-03 |
| Статус | in-progress |
| Фаза | P1 |
| Крейт(ы) | `sk-scan` |
| Зависит от | SPEC-01, SPEC-02 |
| Используется в | SPEC-04, SPEC-05, SPEC-07, SPEC-08, SPEC-10 |
| Последнее изменение | 2026-10-02 (§4.2: облачные заглушки — `Reparse(CloudPlaceholder)` для файлов и каталогов, различение по `FILE_ATTRIBUTE_DIRECTORY`; §4.2: собственный обход на rayon вместо jwalk, метаданные из листинга, правило числа потоков; T-03-04: ручная проверка Ctrl+C перенесена в SPEC-04 T-04-07; §4.1, T-03-05, §6: `probe_readable` не следует по ссылкам; §4.6: семантика `scan.exclude_globs`; §4.1: `PathFilter::check` → `Exclusion`, `ExcludeSet::builtin/with_user(env)`, `names_only`, поведение `walk`; §4.2: проверка условных исключений; §4.5: раскрытие шаблонов путей; §4.1: соглашения `MemFs`, счётчики вызовов, загрузка фикстур перенесена в `sk_testkit::mem_fixture`; T-03-02; §9; T-03-04: ручная проверка Ctrl+C из SPEC-01 §8; §4.4: правило `UwpPackage`, 14 маркеров как в SPEC-02 §4.1; §4.1: `CloudState` без `Pinned`, как в определении enum; §9: OneDrive-корни заполняет `sk-core`; трейт `FsScanner` и его типы — в `sk-core::fs`; `WalkOptions.excludes` через трейт `PathFilter`) |

## 1. Цель

Единый, быстрый и безопасный доступ к файловой системе для всех коллекторов: проверка
существования, листинг, параллельный обход, замер размеров находок (`TargetStats`) и
построение `FolderSummary` с маркерами. Сканер **только читает** (P1), никогда не
«гидрирует» облачные плейсхолдеры, не ходит по reparse points и не падает на ошибках доступа.

## 2. Область

### 2.1 Входит
- Трейт `FsScanner` + реальная реализация `RealFs` (rayon) и фейковая `MemFs` для тестов.
- `measure()`: `TargetStats` для `Target::FileSet` / `Target::File`.
- `summarize()`: `FolderSummary` + вычисление всех `Marker` из SPEC-02 §4.1.
- Глобальные исключения (встроенные + `config.scan.exclude_globs`).
- Длинные пути, reparse points, облачные плейсхолдеры, заблокированные файлы.
- Лимиты обхода и отмена.

### 2.2 Не входит
- Решение, что важно, а что нет: SPEC-07, SPEC-09.
- Копирование файлов: SPEC-10 (использует `FsScanner::walk` для перечисления).
- Быстрый скан через MFT/USN: SPEC-17 (бэклог). API спроектирован так, чтобы `RealFs` можно было заменить.

## 3. Требования

### 3.1 Функциональные
- **FR-03-01** — Все операции работают через `dyn FsScanner`. Ни один другой крейт не вызывает `std::fs` напрямую для чтения пользовательских данных (исключение: чтение конкретного конфиг-файла лаунчера через `FsScanner::read_head`/`read_small`).
- **FR-03-02** — Обход не следует по симлинкам, junction'ам и любым reparse points (кроме OneDrive-файлов, которые учитываются как файлы без чтения содержимого).
- **FR-03-03** — Файлы с атрибутами `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` (0x400000), `FILE_ATTRIBUTE_RECALL_ON_OPEN` (0x40000) или `FILE_ATTRIBUTE_OFFLINE` (0x1000) никогда не открываются на чтение. Они учитываются в статистике как `cloud_only` (размер берётся из метаданных).
- **FR-03-04** — Ошибки доступа (`ERROR_ACCESS_DENIED`, `ERROR_SHARING_VIOLATION`) не прерывают обход. Они считаются в `locked_files` и/или порождают `ScanIssue` (агрегированно, не больше 1 issue на папку верхнего уровня находки).
- **FR-03-05** — Встроенные глобальные исключения §4.5 применяются всегда. Пользователь может добавить свои через `scan.exclude_globs`, но отключить встроенные нельзя (кроме флага `include_excluded` в `measure` для отладки CLI).
- **FR-03-06** — Все обращения к ФС идут через `sk-core::path::to_extended` (пути > 260 символов).
- **FR-03-07** — Обход уважает `CancellationToken`: проверка минимум раз в 1000 записей и не реже 100 мс.
- **FR-03-08** — `measure` для набора находок выполняется параллельно, но пересекающиеся корни обходятся один раз (кэш `DirStatsCache` в рамках скана).

### 3.2 Нефункциональные
- **NFR-03-01** — ≥ 20 000 записей/с на SSD в `walk` (цель P1: 1 млн файлов ≤ 60 с на весь скан).
- **NFR-03-02** — Память `measure`/`summarize` ограничена O(глубина + top-N) на корень. Полный список файлов не хранится.
- **NFR-03-03** — Число потоков: `min(num_cpus, 8)` для SSD. Для HDD/сетевых дисков (`DriveInfo.kind`) 2 потока, чтобы не «убить» диск seek'ами.

## 4. Дизайн

### 4.1 Публичный API

Трейт `FsScanner` и типы его сигнатур (`EntryMeta`, `EntryKind`, `ReparseKind`, `CloudState`,
`DirEntryInfo`, `WalkControl`, `Readability`, `WalkOptions`, `WalkStats`, `FsError`, `PathFilter`)
определены в **`sk-core::fs`** и реэкспортируются из `sk-scan`. Причина: на трейт ссылается
`CollectContext` (SPEC-01 §4.3), а `sk-core` не может зависеть от `sk-scan` (SPEC-01 §4.2).
Реализации (`RealFs`, `MemFs`, `ExcludeSet`) и функции `measure`/`summarize` живут в `sk-scan`.

```rust
// sk-core/src/fs.rs (реэкспорт: sk_scan::FsScanner и т.д.)
pub trait FsScanner: Send + Sync {
    fn metadata(&self, path: &Path) -> Result<EntryMeta, FsError>;
    fn exists(&self, path: &Path) -> bool;                        // без ошибок: false при любой проблеме
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>, FsError>; // один уровень
    /// Параллельный рекурсивный обход. Вызывает `visit` для каждой записи (кроме исключённых).
    fn walk(&self, root: &Path, opts: &WalkOptions, visit: &mut dyn FnMut(&DirEntryInfo) -> WalkControl,
            cancel: &CancellationToken) -> Result<WalkStats, FsError>;
    /// Первые `max` байт файла (для сигнатур, VDF/ACF, маркеров). Не читает cloud-only файлы.
    fn read_head(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError>;
    /// Файл целиком, если ≤ max байт, иначе FsError::TooLarge.
    fn read_small(&self, path: &Path, max: usize) -> Result<Vec<u8>, FsError>;
    /// Пробное открытие на чтение с FILE_SHARE_READ|WRITE|DELETE → Locked при sharing violation.
    /// Не следует по ссылкам (FR-03-02): атрибуты берутся у самой записи; CloudOnly → без открытия;
    /// для reparse point открывается сама ссылка (FILE_FLAG_OPEN_REPARSE_POINT), цель не проверяется.
    fn probe_readable(&self, path: &Path) -> Readability;
}

pub struct EntryMeta {
    pub kind: EntryKind,               // File | Dir | Reparse(ReparseKind)
    pub size: u64,                     // логический размер
    pub mtime: Option<OffsetDateTime>,
    pub ctime: Option<OffsetDateTime>,
    pub attrs: u32,                    // сырые FILE_ATTRIBUTE_* (0 на не-Windows)
    pub cloud: CloudState,             // Local | CloudOnly (по атрибутам §4.2)
}
pub enum ReparseKind { Symlink, Junction, CloudPlaceholder, AppExecLink, Other(u32) }
pub enum CloudState { Local, CloudOnly }

pub struct DirEntryInfo { pub path: PathBuf, pub rel: PathBuf, pub depth: u32, pub meta: EntryMeta }
pub enum WalkControl { Continue, SkipDir, Stop }
pub enum Readability { Ok, Locked, Denied, CloudOnly, Missing }

pub struct WalkOptions {
    pub max_depth: u32,                // из config.scan.max_depth (32)
    pub max_entries: u64,              // лимит на корень, по умолчанию 2_000_000
    pub follow_links: bool,            // всегда false в MVP, поле для SPEC-17
    pub excludes: Arc<dyn PathFilter>, // ExcludeSet, §4.5
    pub include: Option<GlobSet>,      // относительно root
    pub exclude: Option<GlobSet>,      // относительно root (из Target)
    pub threads: usize,
}

pub struct WalkStats { pub entries: u64, pub skipped_excluded: u64, pub errors: u64, pub truncated: bool }

#[derive(thiserror::Error, Debug)]
pub enum FsError { NotFound, AccessDenied, SharingViolation, TooLarge, CloudOnly, Cancelled, Io(std::io::Error) }

// Реализации — sk-scan
pub struct RealFs { /* drive kinds для выбора числа потоков */ }
impl RealFs { pub fn new(env: &Environment) -> Self; }
pub struct MemFs { /* BTreeMap<PathBuf, MemNode> */ }
impl MemFs {
    pub fn new() -> Self;
    pub fn add_file(&mut self, path: &str, size: u64, mtime: &str, content: Option<&[u8]>) -> &mut Self;
    pub fn add_dir(&mut self, path: &str) -> &mut Self;
    pub fn add_reparse(&mut self, path: &str, kind: ReparseKind) -> &mut Self;
    pub fn set_locked(&mut self, path: &str) -> &mut Self;
    pub fn set_cloud_only(&mut self, path: &str) -> &mut Self;
    pub fn calls(&self) -> MemFsCalls;   // счётчики вызовов с момента создания
}
pub struct MemFsCalls { pub read_dir: u64, pub exists: u64, pub read_head: u64 }
// Загрузка YAML-фикстур в MemFs — sk_testkit::mem_fixture (SPEC-12 §4.2, §4.3 «Фикстуры fixtures/fs»).

// Высокоуровневые функции
pub fn measure(fs: &dyn FsScanner, target: &Target, cache: &DirStatsCache, opts: &MeasureOptions,
               cancel: &CancellationToken) -> Result<TargetStats, FsError>;
pub fn measure_all(fs: &dyn FsScanner, findings: &mut [Finding], opts: &MeasureOptions,
                   events: &EventSink, cancel: &CancellationToken) -> Vec<ScanIssue>;
pub fn summarize(fs: &dyn FsScanner, dir: &Path, env: &Environment, opts: &SummaryOptions,
                 cancel: &CancellationToken) -> Result<FolderSummary, FsError>;

pub struct MeasureOptions { pub probe_locks: bool /* по умолчанию true */, pub include_excluded: bool }
pub struct SummaryOptions { pub max_entries: u64 /* 200_000 */, pub max_depth: u32 /* 12 */ }
// sk-core::fs — фильтр исключений, через который WalkOptions не зависит от реализации §4.5
/// Решение фильтра. Условия проверяет обходчик (§4.2), фильтр сам к ФС не обращается.
pub enum Exclusion {
    Keep,
    Exclude,
    ExcludeIfSibling(&'static str), // исключить, если в том же каталоге есть запись, подходящая под глоб имени
    ExcludeIfChild(&'static str),   // исключить каталог, если внутри есть файл с этим именем
}
pub trait PathFilter: Send + Sync + Debug {
    fn check(&self, abs: &Path, name: &OsStr, is_dir: bool) -> Exclusion;
}

// sk-scan
pub struct ExcludeSet { /* globset + быстрый HashSet имён каталогов */ }
impl ExcludeSet {
    pub fn builtin(env: &Environment) -> Self;
    pub fn with_user(env: &Environment, globs: &[String]) -> Result<Self, globset::Error>;
    /// Тот же набор без исключений по полным путям (для `explicit_root` в `measure`, §4.5).
    pub fn names_only(&self) -> Self;
}
impl PathFilter for ExcludeSet { /* check */ }
```

Поведение `walk` (одинаково для `RealFs` и `MemFs`):
- корень сам в `visit` не передаётся; его дети имеют `depth = 1`;
- `include` фильтрует только файлы, каталоги обходятся всегда;
- исключённые записи не учитываются в `max_entries`;
- `MemFsCalls.read_dir` считает и листинги, сделанные внутри `walk`.

Соглашения `MemFs`:
- Путь `&str` делится и по `/`, и по `\`; компоненты сравниваются без учёта регистра (как в NTFS).
- `mtime` — синтаксис фикстур SPEC-12 §4.3 (`-1d` или RFC 3339); если не разбирается — `EntryMeta.mtime = None`.
- Родительские папки создаются неявно.
- Файл без `content` читается (`read_head`/`read_small`) как нули длиной `size`.
- `calls()` считает вызовы `read_dir`, `exists`, `read_head` (нужно тестам §6 и SPEC-05 §6).

### 4.2 Реализация `RealFs::walk`
1. `root = to_extended(root)`. `metadata(root)`: если Reparse → `WalkStats{entries:0}` + Info-issue на стороне вызывающего.
2. Собственный параллельный обход: пул `rayon` из `threads` потоков (правило ниже), одна задача на листинг каталога (`std::fs::read_dir`), дочерние каталоги ставятся в пул; записи передаются в `visit` на вызывающем потоке через канал. Скрытые файлы не пропускаются, по ссылкам не идём, глубина ограничена `max_depth`. `jwalk` не используется: он отбрасывает `std::fs::DirEntry` и даёт метаданные только через `symlink_metadata` (`CreateFileW` на запись), к тому же не поддерживается автором.
3. При обработке листинга каталога (до спуска):
   - отбрасываем исключённые (`PathFilter::check` по имени и полному пути): `ExcludeIfSibling` проверяется по именам из того же листинга каталога, `ExcludeIfChild` — одним `metadata(abs.join(name))` на каталог-кандидат;
   - reparse-каталоги (`attrs & FILE_ATTRIBUTE_REPARSE_POINT`) не раскрываем, они передаются в `visit` как `EntryKind::Reparse`;
   - проверка `cancel` → если отменено, у всех детей обнуляется спуск.
4. Каждая запись → `DirEntryInfo`. Метаданные — `std::fs::DirEntry::metadata()` (на Windows из `WIN32_FIND_DATAW` листинга, без системных вызовов), `attrs` — `MetadataExt::file_attributes()`; tag только для записей с `FILE_ATTRIBUTE_REPARSE_POINT` — `sk-scan::win::reparse_tag` (`FindFirstFileExW`). **Файл не открывается.**
5. `visit` возвращает `SkipDir`/`Stop`. `Stop` выставляет внутренний флаг, обход сворачивается.
6. Счётчик `entries ≥ max_entries` → `truncated = true`, Stop.

Число потоков (NFR-03-03) = min(`opts.threads`, лимит диска корня). Лимит = 2 для `DriveKind::Network` или `DriveMedia::Hdd`, иначе min(num_cpus, 8). При `opts.threads == 0` используется лимит диска. Диск не найден в `env.drives` → как SSD.

Определение `CloudState::CloudOnly`: `attrs & (0x400000 | 0x40000 | 0x1000) != 0`.
Определение `ReparseKind`: по `dwReserved0` из `WIN32_FIND_DATAW` (tag): `IO_REPARSE_TAG_SYMLINK` → Symlink, `IO_REPARSE_TAG_MOUNT_POINT` → Junction, `IO_REPARSE_TAG_CLOUD*` (маска `0x9000001A`) → CloudPlaceholder (см. ниже), `IO_REPARSE_TAG_APPEXECLINK` → AppExecLink. `std::fs::DirEntry` tag не отдаёт, поэтому он берётся через `unsafe` FFI в `sk-scan::win::reparse_tag` с `FindFirstFileExW` (FindExInfoBasic).

> `IO_REPARSE_TAG_CLOUD*` (маска `0x9000001A`) → `EntryKind::Reparse(CloudPlaceholder)` и для файлов, и для каталогов. Это не ссылка: файл-заглушка учитывается как файл (размер из метаданных, содержимое не читается), каталог-заглушка (OneDrive-папка) **обходится** как обычный каталог — его перечисление не гидрирует файлы. Файл и каталог различаются по `attrs & FILE_ATTRIBUTE_DIRECTORY` (`MemFs` выставляет этот атрибут так же). Потребители (`measure`, `summarize`) считают `Reparse(CloudPlaceholder)` без `FILE_ATTRIBUTE_DIRECTORY` файлом, с ним — каталогом. Не раскрываются Symlink, Junction, AppExecLink, Other.

### 4.3 `measure`
Для `Target::File`: `metadata` → stats с `file_count=1`. Если `probe_locks`, то `probe_readable`.
Для `Target::FileSet`:
1. `cache.get(resolved, include, exclude)`, если уже есть.
2. `walk` с `include`/`exclude` из target (глобы относительно root, `globset` с `case_insensitive(true)`, `literal_separator(true)`).
3. Аккумуляция: `total_bytes += size` (для CloudOnly тоже, но в `cloud_only_bytes`, см. Открытые вопросы), `file_count`, `dir_count`, `newest/oldest_mtime`.
4. Probe блокировок: только для файлов с расширениями из «часто блокируемых» (`db, sqlite, ldb, log, dat, lock, pst, ost, vhdx`) и не больше 500 проб на корень (probe дорог). Результат `Locked` → `locked_files += 1`.
5. `truncated = walk.truncated || errors > 0`.
6. `Target::Registry` / `Target::SystemExport` → `measure` возвращает `None`-stats (размер оценивает SPEC-06 `plan()`).

`measure_all`: сортирует findings по глубине пути; для вложенных корней (A ⊂ B) результат A вычисляется в том же обходе B, если у A и B пустые include/exclude (оптимизация через `DirStatsCache`, агрегирующий stats по каждому каталогу до глубины 3 от корня). Прогресс: `Event::Progress{phase: Measure, done: i, total: n, current: template}`.

### 4.4 `summarize` и маркеры
Обход `dir` с `SummaryOptions` → аккумулируем:
- `ext_histogram`: HashMap ext → (count, bytes), затем top-15 по count;
- `sample_names`: 5 top-level имён (сортировка по имени), затем 10 самых свежих файлов (min-heap по mtime), затем добор до 20 файлами с ещё не представленными расширениями;
- `top_children`: bytes/files по каждому прямому ребёнку, top-10 по bytes;
- `max_depth`, `newest/oldest_mtime`;
- флаги-кандидаты для маркеров (наличие конкретных имён).
После обхода: `path = PathTemplate::from_path(dir, env)`, `sample_names` → `privacy::redact`.

Точные правила маркеров (все сравнения имён регистронезависимы):

| Marker | Условие |
|---|---|
| `HasExecutables` | байты `exe+dll+sys+msi+ocx` > 30% от `total_bytes` **или** ≥ 3 `.exe` на глубине ≤ 1 |
| `GitRepo` | прямой ребёнок `.git` (каталог или файл-gitdir) |
| `UnityGame` | путь внутри `{LOCALLOW}` на глубине 2 (`Company\Product`) **и** (`Player.log`/`Player-prev.log`/`output_log.txt` или `Unity\` подпапка), либо любой уровень содержит `*_Data\` рядом с `UnityPlayer.dll` |
| `UnrealSaveGames` | существует подпуть `Saved\SaveGames` (глубина ≤ 3) или ≥ 1 файл `.sav` рядом с `Saved\Config\Windows*` |
| `ElectronApp` | ≥ 3 из прямых детей: `Local Storage`, `IndexedDB`, `Cache`, `GPUCache`, `Code Cache`, `Session Storage`, `blob_storage`, `Service Worker` |
| `ChromiumProfile` | (`Local State` в корне **и** `Default\Preferences`) или (`Preferences` + `Bookmarks`/`History` в корне) |
| `SqliteFiles` | ≥ 1 файл с ext `sqlite, sqlite3, db, db3` и первые 16 байт == `"SQLite format 3\0"` (проверяем `read_head` не больше чем у 5 файлов) |
| `CacheLike` | имя `dir` или ≥ 50% байт в детях с именами по regex `(?i)^(cache|caches|.*cache|temp|tmp|logs?|crash(es|dumps|pad)?|shadercache|dxcache|gpucache)$`, **или** `tmp+log+dmp+etl` > 60% файлов |
| `ConfigLike` | `json, ini, xml, cfg, conf, toml, yaml, yml, reg, config, prefs, plist` ≥ 60% файлов **и** `total_bytes` ≤ 50 МБ |
| `MediaHeavy` | изображения (`jpg, jpeg, png, gif, webp, heic, raw, cr2, nef, arw, dng, psd, tif, tiff, bmp`) + видео (`mp4, mkv, mov, avi, webm, m4v`) + аудио (`mp3, flac, wav, ogg, m4a, aac, opus`) > 70% байт |
| `DocumentHeavy` | `doc, docx, xls, xlsx, ppt, pptx, odt, ods, odp, pdf, txt, md, rtf, epub, djvu` > 50% файлов |
| `ProjectLike` | в любой папке глубины ≤ 2: `package.json, Cargo.toml, go.mod, pyproject.toml, requirements.txt, pom.xml, build.gradle*, CMakeLists.txt, Makefile, *.sln, *.csproj, *.vcxproj, *.uproject, *.unity (ProjectSettings\), *.blend, *.aep, *.prproj, *.als, *.flp, *.rpp, *.kra, *.xcf` |
| `CloudSynced` | `dir` внутри `{ONEDRIVE}` или пути из `HKCU\Software\Dropbox`/`%LOCALAPPDATA%\Dropbox\info.json`, `Google Drive` (`DriveFS`), `Yandex.Disk` (см. Открытые вопросы — где хранить корни) |
| `UwpPackage` | `dir` — сам каталог пакета `{LOCALAPPDATA}\Packages\<name>_<publisherId>` (publisherId — 13 символов `[a-z0-9]`), то есть `PathTemplate::from_path(dir)` = `{PACKAGE:name}` без хвоста |

Маркеры не взаимоисключающие. `summarize` не присваивает категорию: это задача SPEC-07.

### 4.5 Встроенные глобальные исключения
Имена каталогов (сравнение по имени, в любом месте дерева):
```
node_modules, .pnpm-store, bower_components, __pycache__, .pytest_cache, .mypy_cache, .tox,
.venv*, venv (только если содержит pyvenv.cfg), .gradle\caches, .m2\repository, .nuget\packages,
.cargo\registry, .cargo\git, target (только если рядом Cargo.toml), obj/bin (только если рядом *.csproj),
$Recycle.Bin, System Volume Information, $WinREAgent, $SysReset, $Windows.~BT, $Windows.~WS,
Windows.old, Config.Msi, Recovery, MSOCache, PerfLogs
```
Полные пути (шаблоны, раскрываются из Environment):
```
{WINDIR}, {PROGRAMFILES}, {PROGRAMFILES_X86}, {PROGRAMDATA}\Microsoft\Windows\WER,
{LOCALAPPDATA}\Temp, {LOCALAPPDATA}\Microsoft\Windows\INetCache, {LOCALAPPDATA}\Microsoft\Windows\Explorer (thumbcache),
{LOCALAPPDATA}\Packages\*\AC\INetCache, {LOCALAPPDATA}\CrashDumps, {LOCALAPPDATA}\D3DSCache,
{LOCALAPPDATA}\NVIDIA\DXCache, {LOCALAPPDATA}\NVIDIA\GLCache, {LOCALAPPDATA}\AMD\DxCache
```
Файлы в корнях дисков: `pagefile.sys, hiberfil.sys, swapfile.sys, DumpStack.log*`.

Шаблоны путей раскрываются один раз при создании `ExcludeSet` из `env.known_folders`. Шаблон, чья папка отсутствует, пропускается. Шаблоны с `*` (`{LOCALAPPDATA}\Packages\*\AC\INetCache`) становятся глобом по абсолютному пути без учёта регистра. Условные имена (`target`, `obj`/`bin`, `venv`) возвращают `ExcludeIfSibling`/`ExcludeIfChild` (§4.1).

Исключения относятся к **обходу**. Коллектор может явно указать `Target` внутри исключённого пути (например, правило для `{PROGRAMDATA}\...` конкретной программы): `measure` для такого target применяет только имена-исключения, но не path-исключения (флаг `explicit_root` внутри `measure`).

### 4.6 Конфигурация
- `scan.exclude_globs` добавляются в `ExcludeSet::with_user`:
  - пробелы по краям обрезаются, пустые строки пропускаются;
  - `\` читается как разделитель, а не как экранирование;
  - сравнение без учёта регистра; `*` и `?` не переходят через разделитель, `**` переходит;
  - глоб без разделителя сравнивается с именем записи (файла или каталога) в любом месте дерева (`*.tmp`);
  - глоб с разделителем сравнивается с абсолютным путём записи без префикса `\\?\` (`D:\Games\**`, `**\build\out`) и исключает также всё, что лежит ниже совпавшего пути (как встроенные исключения по путям §4.5);
  - ошибка синтаксиса → `globset::Error` из `with_user`, конфиг отвергается при загрузке;
  - `names_only` сохраняет глобы по имени и отбрасывает глобы с разделителем (это исключения по путям, §4.5 `explicit_root`).
- `scan.follow_symlinks` игнорируется в MVP (всегда false), с предупреждением в лог, если true.
- `scan.max_depth` → `WalkOptions.max_depth`.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Корень не существует | `measure` → `FsError::NotFound`. Коллектор решает сам (обычно находка не создаётся). |
| Корень — junction (например, `Documents\My Music` legacy-ссылки в профиле) | Не раскрываем, `WalkStats.entries = 0`. У находки тег `reparse_root` и Info-issue. |
| Legacy junctions профиля (`Application Data`, `Local Settings`, `My Documents`) | Не обходятся (hidden+system+junction) → нет двойного счёта. |
| Symlink-цикл | Невозможен (не следуем по ссылкам). |
| OneDrive Files On-Demand, файл только в облаке | Не открываем. Размер учитывается, `probe_readable` = CloudOnly, в stats отдельным счётчиком (Открытые вопросы). |
| Каталог без прав на листинг (`AccessDenied`) | `errors += 1`, спуск пропущен, агрегированный `ScanIssue::Warning` «n папок недоступны». |
| Файл заблокирован (`SharingViolation`) | `locked_files += 1`. SPEC-10 решает, что делать при копировании. |
| Путь > 32 767 символов | Практически невозможно. `FsError::Io` → пропуск записи. |
| Имя файла не в UTF-8 (неспаренные суррогаты) | Работаем через `OsString`. В `sample_names` lossy + тег `non_utf8_path` на находке. |
| Огромная папка (> `max_entries`) | `truncated = true`. Stats — нижняя граница, в UI «≥ N ГБ». |
| Сетевой диск / медленный HDD | 2 потока. Сетевые корни только если явно в `extra_roots`. |
| Отмена посреди обхода | `FsError::Cancelled` наверх. Кэш не пополняется частичными данными. |
| AppExecLink (`{LOCALAPPDATA}\Microsoft\WindowsApps\*.exe`) | Считается файлом размера 0, не открывается. |

## 6. Тестирование

- **Unit (MemFs, кроссплатформенно):**
  - `walk`: исключения по имени и пути, `SkipDir`, `Stop`, `max_depth`, `max_entries` → truncated, отмена.
  - `measure`: include/exclude глобы (регистронезависимо), вложенные корни и кэш (обход один раз — счётчик вызовов `read_dir` в MemFs), locked/cloud-only.
  - `summarize`: для каждого `Marker` положительный и отрицательный кейс на граничном пороге (29%/31%, 59%/61% ...), порядок `sample_names`, redact.
- **Фикстуры (SPEC-12):** `fixtures/fs/electron-app.yaml`, `unity-locallow.yaml`, `unreal-save.yaml`, `chromium-profile.yaml`, `node-project.yaml`, `cache-dir.yaml`, `onedrive-mixed.yaml`.
- **Windows-интеграционные (`#[cfg(windows)]`, tempdir):**
  - создание junction (`mklink /J` через `std::os::windows::fs::symlink_dir` или `junction` crate) → не обходится;
  - файл, открытый другим хэндлом с `share_mode(0)` → `Readability::Locked`;
  - путь длиной 400 символов → успешный `measure`;
  - файл с атрибутом OFFLINE (`SetFileAttributesW`) → не открывается (проверка через счётчик `read_head`-вызовов в обёртке);
  - junction на удалённый каталог → `probe_readable` = Ok (проверяется сама ссылка, не цель).
- **Бенчмарк (`criterion`, ручной запуск):** генератор 1 млн файлов в tempdir → `walk` ≥ NFR-03-01.

## 7. Задачи

- [x] **T-03-01** — Типы API §4.1 в `sk-core::fs` (`FsScanner`, `PathFilter`, `EntryMeta`, `DirEntryInfo`, `WalkOptions`, `FsError` ...) + реэкспорт из `sk-scan`. *Зависит:* T-02-01. Нужна в P0: от неё зависит SPEC-01 T-01-03. *Готово, когда:* крейты компилируются, документация `///`.
- [x] **T-03-02** — `MemFs` (+ счётчики вызовов) в `sk-scan`; `mem_fixture` и ключи `fixtures/fs` (SPEC-12 §4.3) в `sk-testkit`; фикстура `fixtures/fs/electron-app.yaml`. *Зависит:* T-03-01. *Готово, когда:* интеграционный тест в `crates/sk-scan/tests/` (`sk-testkit` как dev-dependency) загружает `electron-app.yaml`, и `walk` по ней проходит.
- [x] **T-03-03** — `ExcludeSet` (встроенный список §4.5 + пользовательские глобы, условные исключения `target`/`obj`/`venv`). *Зависит:* T-03-01. *Готово, когда:* unit-тесты на каждый пункт списка.
- [ ] **T-03-04** — `RealFs::walk` на rayon: reparse, cloud-атрибуты, исключения, отмена, лимиты, выбор потоков по типу диска. *Зависит:* T-03-03, T-02-02. *Готово, когда:* Windows-тесты junction/long path.
- [x] **T-03-05** — `sk-scan::win`: `reparse_tag`, `probe_readable` через `CreateFileW` (`GENERIC_READ`, share RW|D, `OPEN_EXISTING`, `FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT`, `FILE_FLAG_BACKUP_SEMANTICS` для каталогов); при CloudOnly-атрибутах самой записи — `Readability::CloudOnly` без открытия. *Зависит:* T-03-01. *Готово, когда:* тест Locked + OFFLINE.
- [ ] **T-03-06** — `read_head`/`read_small` с защитой от cloud-only и лимитом. *Зависит:* T-03-05.
- [ ] **T-03-07** — `measure` + `DirStatsCache` + `measure_all` с прогрессом. *Зависит:* T-03-04. *Готово, когда:* тест «вложенные корни — один обход».
- [ ] **T-03-08** — `summarize` + все маркеры по таблице §4.4. *Зависит:* T-03-06, T-02-07, T-02-04. *Готово, когда:* тесты порогов для всех 14 маркеров.
- [ ] **T-03-09** — Бенчмарк `criterion` и генератор дерева. *Зависит:* T-03-04. *Готово, когда:* результат ≥ NFR-03-01 на машине разработчика, число записано в спеку.
- [ ] **T-03-10** — Команда CLI `savekeeper-cli debug summarize <path>` (вывод FolderSummary JSON). *Зависит:* T-03-08, T-01-07.

## 8. Критерии приёмки

- [ ] Ни один тест и ни один ручной прогон не гидрирует OneDrive-файлы (проверка: папка «только онлайн» остаётся с облачным значком после скана).
- [ ] Скан профиля с junction'ами профиля (`Application Data` и т.д.) не даёт двойного счёта размеров.
- [ ] NFR-03-01 подтверждён бенчмарком.
- [ ] Все 14 маркеров покрыты тестами порогов.

## 9. Открытые вопросы

- **Предложение к SPEC-02 §2.6:** добавить в `TargetStats` поле `cloud_only_bytes: u64` и `cloud_only_files: u64`. Нужно для UI («2,3 ГБ, из них 1,1 ГБ только в облаке») и для SPEC-10 (не пытаться копировать).
- ~~**Предложение к SPEC-02 §3.3:** добавить в `Environment` поле `cloud_roots`~~ **Принято** (SPEC-02 §3.3). OneDrive-корни заполняет `Environment::detect()` в `sk-core` (нужны для `{ONEDRIVE}`), `sk-scan::cloud::detect_roots(env)` добавляет Dropbox, Google Drive и Yandex.Disk в фазе Environment.
- **Предложение к SPEC-02 §3.3:** в `DriveInfo` явно поле `media: Ssd | Hdd | Unknown` (через `IOCTL_STORAGE_QUERY_PROPERTY` / `StorageDeviceSeekPenaltyProperty`) для NFR-03-03.
- ~~Формат YAML-фикстур `MemFs`~~ **Решено:** SPEC-12 §4.3 «Фикстуры `fixtures/fs`», загрузка — `sk_testkit::mem_fixture`.
- Нужен ли hard-link дедуп (одинаковый FileId) при подсчёте размеров? Пока нет (редко в пользовательских данных).
