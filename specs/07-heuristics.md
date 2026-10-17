# SPEC-07: Эвристики

| Поле | Значение |
|---|---|
| ID | SPEC-07 |
| Статус | in-progress |
| Фаза | P2 |
| Крейт(ы) | `sk-heuristics` |
| Зависит от | SPEC-01, SPEC-02, SPEC-03, SPEC-04, SPEC-05 |
| Используется в | SPEC-08, SPEC-09, SPEC-11 |
| Последнее изменение | 2026-10-17 (T-07-09: уточнения H-GIT §4.4 — поиск и обход `{HOME}`, `parent`, приоритет строк, теги, сабмодули, вложенные репозитории, список тяжёлых каталогов, ключи `heuristic.git.*`, `heuristic.git_failed`, `heuristic.git_truncated`; §7 T-07-11); 2026-10-10 (T-07-08: уточнения H-USR §4.3.3 — доминирование, H-JUNK при спуске, стандартные папки, ключи `heuristic.usr.*`; §7 T-07-13; §9); 2026-10-10 (T-07-07: `HeuristicContext.roots_walk`/`RootsWalkCache` §4.1, уточнения корней и исключений обхода §4.3.1, агрегатов §4.3.3); 2026-10-10 (T-07-06: уточнения H-WEB §4.6 — кандидаты, include всей папки, exclude, `Sync Data`, учётные данные через `notes_key`, ключи `heuristic.web.*`/`heuristic.title.app_data`; §6); 2026-10-10 (T-07-05: `HeuristicContext.excludes` §4.1, уточнения зон §4.2.1, UWP §4.2.3, частичного покрытия и `installed` §4.2.4, `heuristic.read_failed` §5); 2026-10-10 (T-07-04: `classify_by_markers_with` §4.1, уточнения правил §4.7); 2026-10-10 (T-07-03: `junk_verdict_with_siblings` §4.1/§4.5, приоритет строк, определение игровой библиотеки и игнор широких корней §4.5, ключи `heuristic.junk.*`); 2026-10-10 (T-07-02: форматы `vendors.txt` §4.2.2 и `extensions.yaml` §4.3.2, issue `heuristic.data_invalid` §5); 2026-10-01 (`HeuristicsConfig` — в `sk-core::config`) |

## 1. Цель

Найти то, что не покрыли детерминированные коллекторы (правила SPEC-04, игры SPEC-05,
система SPEC-06): неизвестные папки программ в `AppData`, пользовательские файлы вне
стандартных мест, git-репозитории с несохранёнными изменениями, профили Electron/Chromium.
Отдельно нужно распознать мусор и переустанавливаемое, чтобы не предлагать его по умолчанию.
Всё, что эвристики не смогли уверенно классифицировать, уходит в LLM (SPEC-08) как
`Category::Unknown` вместе с `FolderSummary`.

## 2. Область

### 2.1 Входит
- **H-UNK**: неизвестные top-level папки в зонах поиска (§4.2).
- **H-USR**: пользовательские файлы вне стандартных мест (§4.3).
- **H-GIT**: git-репозитории и их состояние (§4.4).
- **H-JUNK**: детектор мусора и переустанавливаемого (§4.5).
- **H-WEB**: Electron/Chromium-профили: что важно, а что кэш (§4.6).
- Предварительная классификация по маркерам с confidence (§4.7).

### 2.2 Не входит
- Вычисление `FolderSummary` и маркеров (SPEC-03 §4.1, `summarize`).
- Классификация LLM (SPEC-08).
- Итоговый score и выбор по умолчанию (SPEC-09).
- Сверка с NSRL (SPEC-16, backlog).

## 3. Требования

### 3.1 Функциональные
- **FR-07-01** — Реализован как `PostCollector` (SPEC-01 §4.3) с `id = "heuristics"`, работает после `rules`, `games`, `system` и учитывает `PriorResults::claimed`.
- **FR-07-02** — Каждая top-level папка в зонах §4.2, не покрытая `claimed`, даёт ровно одну находку: классифицированную (`confidence ≥ 0.6`) или `Unknown`.
- **FR-07-03** — Каждая находка содержит `Evidence { source: Heuristic { heuristic_id }, .. }` с ключом i18n и аргументами (P3).
- **FR-07-04** — Пользовательские файлы группируются в «папки-кандидаты», а не в находки на каждый файл (§4.3.3).
- **FR-07-05** — Git-репозитории с незакоммиченными изменениями, незапушенными коммитами, stash или без remote получают тег `git-unsaved` и высокий приоритет. Чистые репозитории с remote помечаются тегом `git-clean` (reinstallable через `git clone`).
- **FR-07-06** — Для git-репозиториев с тяжёлыми игнорируемыми каталогами (`node_modules`, `target`, `.venv` …) находка содержит `exclude` для них. Дополнительно предлагается альтернатива `git bundle` (§4.4.4).
- **FR-07-07** — Мусор (`Cache`) и переустанавливаемое (`Reinstallable`) выдаются как находки (чтобы пользователь видел, что они учтены), но с явным evidence «почему не нужно».
- **FR-07-08** — Список находок `Unknown` и соответствующих `FolderSummary` передаётся в `ScanReport::unknown_summaries` и в фазу `Classify`.
- **FR-07-09** — Все эвристики можно отключить по отдельности через конфиг (§4.9).

### 3.2 Нефункциональные
- **NFR-07-01** — H-UNK + H-WEB + H-JUNK на типичном профиле (≈ 300 папок в AppData) ≤ 10 с на SSD.
- **NFR-07-02** — H-USR на диске 1 ТБ / 2 млн файлов ≤ 90 с (параллельный обход SPEC-03), память ≤ 300 МБ.
- **NFR-07-03** — H-GIT: ≤ 500 мс на репозиторий среднего размера. Общий лимит 200 репозиториев, дальше они помечаются как `truncated`.
- **NFR-07-04** — Детерминизм: одинаковая ФС даёт одинаковые находки в одинаковом порядке.
- **NFR-07-05** — Только чтение (P1). В частности, gix открывается в режиме, который не пишет в `.git` (без обновления index stat-кэша).

## 4. Дизайн

### 4.1 Публичный API

```rust
// sk-heuristics
pub struct HeuristicsCollector { cfg: HeuristicsConfig }

impl HeuristicsCollector {
    pub fn new(cfg: HeuristicsConfig) -> Self;
}

#[async_trait]
impl PostCollector for HeuristicsCollector {
    fn id(&self) -> &'static str { "heuristics" }
    async fn collect(&self, ctx: &CollectContext, prior: &PriorResults<'_>)
        -> Result<CollectOutput, CollectorError>;
}

/// Отдельные эвристики — внутренний трейт, публичен для тестов.
pub trait Heuristic: Send + Sync {
    fn id(&self) -> &'static str;                       // "h-unk", "h-usr", "h-git", "h-junk", "h-web"
    fn run(&self, hctx: &HeuristicContext) -> HeuristicOutput;  // синхронный, вызывается в spawn_blocking
}

pub struct HeuristicContext<'a> {
    pub env: &'a Environment,
    pub scanner: &'a dyn FsScanner,                    // SPEC-03 §4.1
    pub claimed: &'a PathSet,
    pub prior: &'a [Finding],
    pub cfg: &'a HeuristicsConfig,
    pub cancel: &'a CancellationToken,
    pub progress: &'a dyn Fn(u64, Option<&str>),
    pub excludes: &'a Arc<ExcludeSet>,                  // глобальные исключения скана (SPEC-03 §4.5): встроенные + scan.exclude_globs; при некорректных глобах — только встроенные (issue выдаёт фаза Measure)
    pub roots_walk: &'a RootsWalkCache,                 // общий обход корней §4.3.1 (OnceLock): выполняет первая эвристика, которой он нужен
}

pub struct RootsWalkCache;
impl RootsWalkCache { pub fn new(extra_roots: Vec<PathBuf>, max_depth: u32) -> Self; } // Default: без доп. корней, глубина 32

pub struct HeuristicOutput {
    pub findings: Vec<Finding>,
    pub unknown: Vec<FolderSummary>,                    // для LLM
    pub claimed: Vec<PathBuf>,                          // чтобы следующие эвристики не дублировали
    pub issues: Vec<ScanIssue>,
}

/// Классификация по маркерам (§4.7) с дефолтными порогами (§4.9); «сейчас» для правила 8 — текущее время UTC.
pub fn classify_by_markers(s: &FolderSummary, env: &Environment) -> MarkerVerdict;
/// То же с порогами `HeuristicsConfig::thresholds` и моментом `now`, от которого отсчитывается «недавно» (правило 8).
/// Чистая функция, покрыта таблицей тестов; H-UNK вызывает её с одним `now` на скан.
pub fn classify_by_markers_with(s: &FolderSummary, env: &Environment, thresholds: &HeuristicThresholds, now: OffsetDateTime) -> MarkerVerdict;

/// H-JUNK (§4.5) только по summary: строки «target рядом с Cargo.toml» и «bin/obj рядом с *.csproj» здесь не срабатывают.
pub fn junk_verdict(s: &FolderSummary, env: &Environment) -> Option<MarkerVerdict>;
/// То же, когда известен листинг родителя: `siblings` — имена остальных записей родительской папки.
pub fn junk_verdict_with_siblings<S: AsRef<str>>(s: &FolderSummary, env: &Environment, siblings: &[S]) -> Option<MarkerVerdict>;

pub struct MarkerVerdict {
    pub category: Category,
    pub confidence: f32,
    pub reason_key: &'static str,                       // "heuristic.config_like" ...
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}
```

Порядок выполнения внутри коллектора: `h-git` → `h-web` → `h-junk` → `h-unk` → `h-usr`.
Каждая следующая эвристика получает `claimed`, расширенный результатами предыдущих.
Так git-репозиторий внутри `Documents` не станет ещё и «пользовательскими файлами», а
Electron-профиль не станет `Unknown`. `h-git` и `h-usr` делят один обход корней (§4.3.1),
результат обхода кэшируется в `HeuristicContext` через `RootsWalkCache` (`OnceLock`); issues обхода возвращаются только первому вызову.

### 4.2 H-UNK: неизвестные папки

#### 4.2.1 Зоны поиска

| Зона | Глубина кандидата | Примечание |
|---|---|---|
| `{APPDATA}\*` | 1 | Для вендорных контейнеров (§4.2.2) — 2 |
| `{LOCALAPPDATA}\*` | 1 (вендоры — 2) | Исключения: `Temp`, `Microsoft\Windows\INetCache`, `CrashDumps`, `Packages` (отдельно, §4.2.3) |
| `{LOCALLOW}\*\*` | 2 | Unity: `Company\Product` |
| `{PROGRAMDATA}\*` | 1 | Только если есть файлы с mtime < 365 дней **и** не `HasExecutables` **и** папка не принадлежит `installed_programs` через InstallLocation. Иначе пропуск без находки. |
| `{DOCUMENTS}\*` | 1 | Папки программ в Documents (`My Games`, `Klei`, `Rockstar Games`, `Image-Line` …) |
| `{SAVED_GAMES}\*` | 1 | Почти всегда `GameSave`, confidence 0.7 при отсутствии правила |
| `{HOME}\.*` | 1 | dot-папки и dot-файлы (`.ssh`, `.gitconfig`, `.cargo`, `.aws`, `.kube`, `.docker` …), если не покрыты правилами |
| `{HOME}\*` (не Known Folders) | 1 | Нестандартные папки в корне профиля (`source`, `projects`, `Zomboid` …) |

Уточнения: кандидатами не считаются папки, исключённые глобально (SPEC-03 §4.5; отсюда же исключения `{LOCALAPPDATA}`: `Temp`, `Microsoft\Windows\INetCache`, `CrashDumps`), ссылки/junction, а также папки `{HOME}\*`, равные Known Folder или содержащие его (`{HOME}\AppData`, `{HOME}\OneDrive` с перенаправленными Documents). Корни облаков (`{HOME}\Dropbox`) — обычные кандидаты, тег `cloud-synced` по маркеру (§5). Вендорные контейнеры (§4.2.2) — только в `{APPDATA}` и `{LOCALAPPDATA}`. `{PROGRAMDATA}`: «есть файлы с mtime < 365 дней» = `summary.newest_mtime` моложе 365 дней. Dot-файлы `{HOME}\.*` (обычные файлы): < 10 МиБ — `Target::File`, `DevEnvironment`, confidence 0.65, `heuristic.dotfile`; иначе `Unknown` без summary (в LLM не отправляется); прочие файлы корня профиля не рассматриваются.

#### 4.2.2 Вендорные контейнеры
Если имя top-level папки есть в списке вендоров (`Microsoft`, `Google`, `Adobe`, `JetBrains`,
`Mozilla`, `Autodesk`, `NVIDIA Corporation`, `Packages`, `Programs`, `Ubisoft`, `EpicGamesLauncher` …,
файл `crates/sk-heuristics/data/vendors.txt`), кандидатами становятся её дочерние папки.
Глубина при этом увеличивается на 1.

Формат `vendors.txt`: одно имя на строку, `#` начинает комментарий, пустые строки пропускаются; сравнение без учёта регистра, дубли и разделители пути запрещены.

#### 4.2.3 UWP / Microsoft Store (`{LOCALAPPDATA}\Packages\<PFN>`)
Для каждого пакета берутся только `LocalState`, `RoamingState`, `Settings`, `SystemAppData\wgs`
(сохранения Xbox/Game Pass). `LocalCache`, `TempState`, `AC` отправляются в exclude.
Пакеты `Microsoft.Windows.*` и `MicrosoftWindows.*` пропускаются. Категория: `GameSave`, если
найден `SystemAppData\wgs`, иначе `AppData` с confidence 0.5 (кандидат для LLM).
Уточнения: GameSave — confidence 0.8, `heuristic.uwp_saves`; AppData — `heuristic.uwp_package`. Include: `LocalState/**`, `RoamingState/**`, `Settings/**`, `SystemAppData/wgs/**`. Пакет без этих папок — claimed без находки.

#### 4.2.4 Алгоритм
```
for zone in zones:
  for cand in list_candidates(zone):                     // по §4.2.1-§4.2.3
    if claimed.covers(cand) or claimed.has_descendant(cand) and fully_covered(cand): continue
    if cancel: return
    summary = scanner.summarize(cand, limits.summary)     // SPEC-03 §4.1
    verdict = classify_by_markers(&summary, env)          // §4.7
    finding = make_finding(cand, verdict, summary)
    if verdict.confidence < cfg.unknown_threshold (0.6):
        finding.category = Unknown; out.unknown.push(summary)
    out.findings.push(finding); out.claimed.push(cand)
```
- `fully_covered`: если `claimed` содержит потомков кандидата, а их суммарный размер ≥ 90% размера
  кандидата (по summary), кандидат пропускается. Иначе создаётся находка на **остаток**
  с `exclude` = относительные пути покрытых потомков.
- `title`: имя папки (для `{LOCALLOW}` — `Company / Product`).
- `app`: `AppRef { id: slug(name), name, kind: Application, installed: match_installed(name) }`,
  где `match_installed` — нечёткое сравнение с `installed_programs.display_name/publisher`
  (нормализация + совпадение токенов ≥ 0.8 Jaccard). `installed`: `Some(true)` при совпадении, иначе `None` (промах нечёткого сравнения не доказывает удаление, SPEC-09).

Уточнения (T-07-05): порог остатка при частичном покрытии — §5 (≥ 10% или ≥ 1 МиБ, размеры покрытых потомков — через `summarize`/метаданные); если summary кандидата `truncated`, остаток не оценивается и находка на остаток создаётся всегда. `exclude` — относительные пути верхних покрытых потомков, разделитель `/`, глоб-символы экранируются. Пустая папка (правило 11 §4.7) находки не даёт, но заносится в `claimed` (объяснена для FR-07-02).

### 4.3 H-USR: пользовательские файлы вне стандартных мест

#### 4.3.1 Корни обхода
- `config.scan.extra_roots` + `ScanOptions::roots`.
- По умолчанию: корни всех `Fixed`-дисков (`DriveInfo`), кроме сетевых и съёмных.
- **Исключаются** (помимо глобальных исключений SPEC-03): `{WINDIR}`, `{PROGRAMFILES}`,
  `{PROGRAMFILES_X86}`, `{PROGRAMDATA}`, `X:\$Recycle.Bin`, `X:\System Volume Information`,
  `X:\Recovery`, `X:\PerfLogs`, `X:\Windows.old`, `X:\MSOCache`, `X:\pagefile.sys` и т.п.,
  все библиотеки лаунчеров из `env.launchers` (`steamapps`, `Epic Games`, `GOG Games`, `XboxGames` …),
  папки `InstallLocation` из `installed_programs`, а также `{HOME}\AppData` (покрыто H-UNK).
- Стандартные пользовательские папки (`{DOCUMENTS}`, `{DESKTOP}`, `{PICTURES}`, `{MUSIC}`,
  `{VIDEOS}`, `{DOWNLOADS}`) **обходятся**, но находка на них создаётся одна — целая папка
  (правило SPEC-04 `user-folders` обычно уже её «заклеймило». Тогда H-USR ищет в них только
  git-репозитории и тяжёлые подпапки-мусор для exclude).

Уточнения (T-07-07): корни — `Fixed`-диски (при `usr.scan_all_fixed_drives = false` — только диск `{WINDIR}`) и `scan.extra_roots`; сетевые и съёмные диски — только явным корнем. `ScanOptions::roots` и `ScanOptions::max_depth` в коллекторы пока не попадают (`CollectContext` их не содержит, SPEC-01 §4.3), глубина — `scan.max_depth`. Корни сортируются, дубликаты (без учёта регистра) удаляются. Вложенный корень отбрасывается, только если обход ближайшего объемлющего корня действительно в него войдёт (на пути нет пропускаемой папки §4.3.1, глобального исключения, `.git`, ссылки, нечитаемой папки, и глубина < `max_depth`); тогда он обходится в составе внешнего корня с его лимитом глубины. Иначе он обходится как самостоятельный явный корень, а внешний обход его пропускает: каждая папка учитывается один раз. Пропускаемая папка, заданная корнем явно, обходится; исключения под ней действуют, глобальные исключения по пути к корню — нет (SPEC-03 §4.5 `explicit_root`). Пропускаются также `{APPDATA}`, `{LOCALAPPDATA}`, `{LOCALLOW}` (их покрывает H-UNK); «библиотеки лаунчеров» — `LauncherInfo.root`, `install_dir` каждой игры и папка `steamapps` над ним; общий родитель игр (`Epic Games`, `GOG Games`) обходится. Неспецифичные пути лаунчеров и программ (корень диска, Known Folder или содержащая его папка) не исключаются (как в §4.5). Папки уровня диска (`$Recycle.Bin`, `System Volume Information` …) — глобальные исключения SPEC-03 §4.5. Корень-ссылка не обходится (Info `issue.scan.reparse_root`); корень — не папка или нечитаем — `heuristic.read_failed` / `heuristic.access_denied`; ошибки чтения папок — Warning `issue.scan.dirs_unreadable {count}` на корень; source у всех — `heuristics`; отсутствующий корень пропускается молча. `.git` не обходится: папка с `.git` (папка или файл) помечается как рабочая копия, с `HEAD` + `objects` + `refs` — как bare-репозиторий.

#### 4.3.2 Таблица расширений → вид контента

Файл `crates/sk-heuristics/data/extensions.yaml`, встраивается в бинарник. Формат: `groups: [{ kind, category, weight, extensions }]` — `kind` (`ContentKind`) и `category` (`Category`) в snake_case, `weight` ∈ [−1, 1], `extensions` — строчные, без точки (`[a-z0-9_-]+`). Каждый `ContentKind` ровно один раз, расширение — не более чем в одной группе. Валидация при старте; данные проверяются тестами T-07-02.

| Группа (`ContentKind`) | Расширения (фрагмент) | Категория | Вес |
|---|---|---|---|
| `document` | doc docx odt rtf pdf txt md xls xlsx ods csv ppt pptx odp one epub | UserFiles | 1.0 |
| `photo` | jpg jpeg png heic webp gif tif tiff bmp | UserFiles | 0.6 |
| `photo_raw` | cr2 cr3 nef arw dng orf rw2 raf srw pef | UserFiles | 1.0 |
| `video` | mp4 mov mkv avi m2ts mts | UserFiles | 0.4 |
| `audio` | mp3 flac wav m4a ogg aiff | UserFiles | 0.4 |
| `creative_project` | psd psb kra xcf clip ai indd afphoto afdesign blend c4d max ma mb ztl spp sbs aep prproj drp veg fcpbundle | UserFiles | 1.0 |
| `audio_project` | flp als ptx cpr rpp logicx song aup3 | UserFiles | 1.0 |
| `dev_project` | sln csproj vcxproj uproject unity godot | DevEnvironment | 0.9 |
| `code` | rs py js ts cs cpp c h go java kt swift lua gd | DevEnvironment | 0.5 |
| `credentials` | kdbx kdb pfx p12 pem key ppk gpg asc ovpn | Credentials | 1.0 |
| `ebook_mail` | pst ost mbox eml | UserFiles | 0.9 |
| `archive` | zip 7z rar tar gz | UserFiles | 0.2 |
| `vm_disk` | vhd vhdx vmdk vdi qcow2 | UserFiles | 0.3 (очень большие → предупреждение) |
| `binary` | exe dll msi sys cab | Reinstallable | −1.0 |

Неперечисленные расширения получают вес 0.

#### 4.3.3 Кластеризация в папки-кандидаты
Нужно, чтобы пользователь увидел `D:\Work\Clients` как одну находку, а не 12 000 файлов.

1. Однопроходный обход (`scanner.walk`) с агрегацией по директориям: для каждой директории считаются `user_weight = Σ(weight(ext) × min(size, 50 МБ)/1 МБ)`, `bytes`, `files`, `binary_bytes`, `newest_mtime`. Уточнения (T-07-07): МБ = 2^20 байт; файлы считаются по SPEC-03 §4.3 шаг 3 (ссылки не учитываются); вес применяется к суммам `min(size, 50 МиБ)` по группам в фиксированном порядке, поэтому результат не зависит от порядка обхода (NFR-07-04); файлы групп `credentials`/`ebook_mail` собираются при обходе для п. 5.
2. Агрегаты сворачиваются вверх по дереву (post-order), чтобы получить поддерево.
3. **Выбор кластера:** директория `D` становится кандидатом, если:
   - `subtree.user_weight ≥ cfg.usr.min_weight` (по умолчанию 20), **и**
   - `subtree.binary_bytes / subtree.bytes < 0.3`, **и**
   - ни один предок `D` (до корня обхода, не включая корень диска) не является кандидатом, **и**
   - «доминирование»: если среди не покрытых `claimed` детей `D` ровно один `C` с `C.user_weight ≥ 0.9 × D.user_weight`, `C` сам проходит пороги п. 3, а положительный вес остатка `D` без `C` (собственные файлы `D` и прочие дети; сумма вкладов групп с весом > 0) меньше `min_weight`, кандидатом вместо `D` становится `C`, и проверка повторяется для `C`. Иначе кандидат — `D`: спуск не теряет соседей `C`, которые могли бы стать кластером.
4. Корень диска (`D:\`) никогда не становится кандидатом. Вместо него кандидатами становятся его дети по тем же правилам.
5. Одиночные ценные файлы (`credentials`, `ebook_mail`) вне кластеров дают находку типа `File`.
6. Лимит: максимум `cfg.usr.max_candidates` (по умолчанию 200) кластеров на скан. Кластеры сортируются по `user_weight` desc, остальные попадают в issue `heuristic.usr_truncated`.

Категория кластера — категория доминирующей по весу группы (§4.3.2). Если доля `credentials` > 0,
`sensitivity = High` и добавляется тег `credentials-inside`. `evidence.message_args`
содержит топ-3 группы с количеством файлов («1 240 фото RAW, 36 проектов Photoshop»).

Уточнения (T-07-08): H-JUNK (`junk_verdict_with_siblings`, §4.5) проверяется для каждой рассматриваемой папки до порогов п. 3 и для каждого доминирующего ребёнка до спуска в него; `siblings` — имена остальных подпапок родителя (обход хранит только папки; `target`/`bin`/`obj` — условные глобальные исключения SPEC-03 §4.5). Сводка строится из агрегатов (путь, размер, число файлов, `newest_mtime`, без маркеров), поэтому строки 2, 4, 5 §4.5 здесь не срабатывают. В мусорную папку H-USR не спускается: если она проходит пороги п. 3, она даёт находку с вердиктом §4.5 (FR-07-07) без тега `credentials-inside`, а ценные файлы внутри (п. 5) получают свои находки и попадают в её `exclude`; иначе находки нет. Папка с весом положительных групп < `min_weight` не рассматривается вместе с поддеревом. Кандидатом никогда не становится папка, равная Known Folder или содержащая её (`C:\Users`, `{HOME}`): рассматриваются её дети. Стандартная папка §4.3.1, не покрытая `claimed` и с файлами, даёт одну находку на всю папку (`heuristic.usr.user_folder`, confidence 0.7, вне лимита п. 6), внутрь кластеры не ищутся. Покрытые `claimed` потомки вычитаются из агрегатов и попадают в `exclude` (как §4.2.4, без порога остатка). Находка кластера: `FileSet` всей папки, confidence 0.7, `heuristic.usr.cluster`, `app = None`, `message_args`: `files`, `bytes`, `group1..3` (kind из `extensions.yaml`), `group1..3_files`, `cloud_only_files` (если есть); категория — группа с наибольшим вкладом `weight × Σmin(size, 50 МиБ)` (> 0), иначе `UserFiles`; теги `credentials-inside`, `cloud-synced`, `huge` (> 50 ГиБ). Одиночный файл (п. 5): `Target::File`, категория его группы, confidence 0.9, `heuristic.usr.valuable_file`, `message_args`: `bytes`, `group`; ценные файлы кластеров, отброшенных лимитом, тоже получают такие находки. Лимит п. 6 считает и мусорные кластеры (при равном весе — порядок обхода); issue `heuristic.usr_truncated` — Warning, source `heuristics`, `{count, limit}`. Порядок выдачи: кластеры в порядке обхода, затем файлы по пути. Обход отменён или неполон — H-USR находок не даёт.

### 4.4 H-GIT: git-репозитории

#### 4.4.1 Поиск
- Во время обхода §4.3.1 **и** в `{HOME}` (без AppData, до глубины `cfg.git.max_depth = 6`)
  ищутся каталоги, содержащие `.git` (папку или файл-gitlink worktree).
- Спуск внутрь найденного репозитория не делается (сабмодули читаются через gix).
- Bare-репозитории (`HEAD` + `objects` + `refs` в каталоге) тоже распознаются.

#### 4.4.2 Анализ (gix, только чтение)

```rust
pub struct GitState {
    pub head_branch: Option<String>,
    pub detached: bool,
    pub remotes: Vec<String>,            // имена; URL обезличиваются (user:pass@ удаляется)
    pub dirty_files: u32,                // изменённые отслеживаемые
    pub untracked_files: u32,            // не игнорируемые неотслеживаемые (лимит подсчёта 10 000)
    pub unpushed_commits: u32,           // Σ по локальным веткам: ahead относительно upstream; ветка без upstream считается целиком (лимит 1000)
    pub local_only_branches: u32,        // ветки без upstream
    pub stash_entries: u32,              // refs/stash reflog
    pub has_lfs: bool,
    pub ignored_heavy_dirs: Vec<(String, u64)>, // игнорируемые каталоги > 50 МБ: node_modules, target, .venv, build, dist ...
    pub last_commit_time: Option<OffsetDateTime>,
}
```

Если gix не справился (повреждённый репозиторий, неподдерживаемое расширение), выдаётся
`ScanIssue::Warning` и находка без `GitState` с confidence 0.5.

#### 4.4.3 Классификация

| Условие | Тег | Категория | Confidence | Смысл для SPEC-09 |
|---|---|---|---|---|
| нет remotes | `git-no-remote` | DevEnvironment | 0.95 | незаменимо |
| `dirty + untracked + unpushed + stash > 0` | `git-unsaved` | DevEnvironment | 0.95 | незаменимо (частично) |
| всё запушено, чисто | `git-clean` | Reinstallable | 0.85 | можно `git clone` |

`evidence.message_args`: `dirty`, `untracked`, `unpushed`, `stash`, `branches`, `remote` (имя хоста без учётных данных).

Уточнения (T-07-09): репозитории ищутся в общем обходе §4.3.1; `{HOME}` обходится отдельно (до `git.max_depth`, без AppData), только если общий обход до него не дошёл; из этого обхода отбрасываются лишь репозитории, уже найденные общим обходом (и папки внутри его bare-репозиториев). `parent` репозитория — ближайший объемлющий репозиторий по результатам обоих обходов. Внутрь bare-репозитория поиск не идёт; папки из `claimed` пропускаются. Приоритет строк §4.4.3: нет remotes > unsaved > clean; репозиторий без remotes получает оба тега `git-no-remote` и `git-unsaved` (`heuristic.git.no_remote`), unsaved — `heuristic.git.unsaved`, clean — `heuristic.git.clean`. Всегда тег `git`, а также `cloud-synced` (есть файлы только в облаке, `message_args.cloud_only_files`) и `huge` (> 50 ГиБ); `app = None`, `Target::FileSet { include: ["**"] }`. Рабочая копия с файлами только в облаке не проверяется (status не запускается, чтобы не скачивать файлы) и считается unsaved. Коммиты detached `HEAD`, которых нет ни в одной ветке, считаются незапушенными. `refs/stash` без reflog считается одной записью. Ошибка gix: находка `heuristic.git.failed` (confidence 0.5, DevEnvironment, без `GitState`) и `ScanIssue::Warning heuristic.git_failed {error}`, source `heuristics`, путь и текст ошибки обезличены. Сверх `git.max_repos` репозитории не читаются: находка `heuristic.git.not_analyzed` (confidence 0.5, DevEnvironment, тег `truncated`) и одна `ScanIssue::Warning heuristic.git_truncated {count, limit}`. Сабмодуль (gitlink по его пути в индексе внешнего репозитория) своей находки не даёт: его счётчики прибавляются к внешнему, тяжёлые каталоги — с префиксом пути сабмодуля; вложенные сабмодули сливаются от самых глубоких. Внешний репозиторий получает `exclude` для вложенных репозиториев, не являющихся сабмодулями (в т.ч. вложенных в его сабмодули), и для потомков из `claimed`. `ignored_heavy_dirs` — игнорируемые каталоги > 50 МиБ с именем (без учёта регистра) из списка `node_modules`, `bower_components`, `.pnpm-store`, `target`, `.venv`/`.venv*`, `venv`, `build`, `dist`, `out`, `bin`, `obj`, `__pycache__`, `.gradle`, `.next`, `.nuxt`, `.svelte-kit`, `.parcel-cache`, `.turbo`, `.angular`, `.dart_tool`, `.tox`, `.mypy_cache`; остальные игнорируемые каталоги могут содержать данные пользователя и остаются в находке.

#### 4.4.4 Что сохранять
- `Target::FileSet { root: <repo>, include: ["**"], exclude: ignored_heavy_dirs }` — «рабочая копия без тяжёлых артефактов».
- Альтернатива для `git-unsaved`/`git-no-remote`: тег `git-bundle-available` и `Target::SystemExport { exporter_id: "git-bundle", params: { "repo": "<template>" } }` как **дочерняя находка** (не выбрана по умолчанию). Выполняет её `sk-backup` через `git bundle create <out> --all` плюс отдельно архив незакоммиченных файлов (`git stash create` без записи не подходит, поэтому просто копируются dirty и untracked файлы). Экспортёр `git-bundle` регистрируется в SPEC-06 (см. §9).

### 4.5 H-JUNK: мусор и переустанавливаемое

Работает над кандидатами H-UNK и H-USR (вызывается как функция `junk_verdict(&FolderSummary, &Environment) -> Option<MarkerVerdict>` до `classify_by_markers`) и
над подпапками уже найденных находок (генерирует `exclude`).

`FolderSummary` не содержит сведений о соседях папки, поэтому условия «рядом» (`Cargo.toml`, `*.csproj`) проверяются только в `junk_verdict_with_siblings`; H-USR и генерация `exclude` передают имена записей родителя. Имена сравниваются без учёта регистра с последними сегментами пути (шаблона и каждого его резолва) целиком.

| Признак | Результат | Confidence |
|---|---|---|
| Имя совпадает (без учёта регистра) с `cache`, `caches`, `cache2`, `gpucache`, `code cache`, `shadercache`, `dxcache`, `glcache`, `temp`, `tmp`, `crashdumps`, `crashpad`, `logs`, `log`, `blob_storage`, `service worker\cachestorage`, `webcache` | `Cache` | 0.9 |
| Маркер `CacheLike` без `ConfigLike` | `Cache` | 0.75 |
| Имя `node_modules`, `.venv`, `venv`, `__pycache__`, `target` (при наличии `Cargo.toml` рядом), `bin`/`obj` (при наличии `*.csproj`), `.gradle`, `.m2\repository`, `.nuget\packages`, `pip\cache` | `Cache` (restorable) | 0.95 |
| `HasExecutables` и путь внутри/равен `InstallLocation` установленной программы | `Reinstallable` | 0.9 |
| `HasExecutables`, нет `ConfigLike`/`SqliteFiles`, mtime всех файлов ≈ одинаковый (разброс < 1 час) | `Reinstallable` | 0.7 |
| Папка лаунчера или игровой библиотеки | `Reinstallable` | 0.95 |
| Имя совпадает с известным «скачиваемым контентом»: `Steam\steamapps\shadercache`, `NVIDIA\DXCache`, `Package Cache`, `Downloaded Installations`, `SquirrelTemp`, `*\Updates`, `*\update` | `Cache` | 0.9 |

Если совпало несколько строк, побеждает строка с наибольшим confidence, при равенстве — по порядку таблицы. «Папка лаунчера» — `LauncherInfo.root`; «игровая библиотека» — `install_dir` игры, папка с последними сегментами `steamapps\common` или общий родитель `install_dir` не менее двух разных известных игр (родитель единственной игры библиотекой не считается — в нём могут быть пользовательские файлы). Сравнение на равенство: подпапки (например `userdata`) под строку не попадают. `InstallLocation` и папки лаунчеров/игр без именованного компонента, а также равные известной папке или содержащие её (`C:\`, `C:\Users`, `{HOME}`, `{PROGRAMFILES}`, `{LOCALAPPDATA}` …), игнорируются.

Ключи причин (`reason_key`) по строкам таблицы: 1 — `heuristic.junk.cache_name`, 2 — `heuristic.junk.cache_like`, 3 — `heuristic.junk.restorable`, 4 — `heuristic.junk.installed_program`, 5 — `heuristic.junk.uniform_binaries`, 6 — `heuristic.junk.launcher`, 7 — `heuristic.junk.downloadable`.

Маленькие файлы-исключения, найденные внутри «мусора» (например `settings.json` в папке
`logs`), не спасают папку: сохранение конфигов — задача правил SPEC-04.

### 4.6 H-WEB: профили Electron/Chromium

Детект: маркер `ElectronApp` или `ChromiumProfile` (SPEC-02 §4.1).

| Подпапка / файл | Роль | Решение |
|---|---|---|
| `Local Storage`, `IndexedDB`, `Session Storage`, `databases` | данные приложения | include |
| `Preferences`, `Local State`, `settings.json`, `config.json`, `*.json` в корне | настройки | include |
| `Cookies`, `Login Data`, `Web Data`, `Network\Cookies` | учётные данные | include, `sensitivity = High`, тег `browser-credentials` (шифруются DPAPI, на другой машине не расшифруются, предупреждение `heuristic.dpapi_bound`) |
| `Cache`, `Code Cache`, `GPUCache`, `DawnCache`, `GrShaderCache`, `ShaderCache`, `Service Worker\CacheStorage`, `Crashpad`, `blob_storage`, `logs` | кэш | exclude |
| `extensions`, `Extensions` | переустанавливаемое | exclude, если рядом есть манифест синхронизации, иначе include |

Итог: одна находка на профиль с `include`/`exclude` из таблицы, категория `AppData`,
confidence 0.8. `title = "<AppName> — данные приложения"`. Полноценные браузеры (Chrome, Edge,
Firefox, Brave, Opera) покрываются правилами SPEC-04, и H-WEB их не трогает, так как они в `claimed`.

Уточнения (T-07-06): кандидаты — кандидаты H-UNK вида «папка» (§4.2.1–§4.2.2, включая `{PROGRAMDATA}` без фильтра свежести; UWP-пакеты и dot-файлы — нет), не покрытые `claimed`, и их подпапка `User Data` (не ссылка, не исключена глобально, не в `claimed`). Детект — по маркерам сводки глубины 2; `ChromiumProfile` важнее `ElectronApp`; `reason_key` — `heuristic.web.chromium` / `heuristic.web.electron`. Корень профиля — папка с маркером. Папка, все записи корня которой — кэш-строки таблицы, профилем не считается (остаётся H-UNK). `include` пуст (вся папка): строки «include» сохраняются вместе с данными вне таблицы (`sql\`, `attachments\`) — корень уходит в `claimed`, и невключённое не увидела бы ни одна эвристика. `exclude`: кэш-строки как `**/<имя>` (корень и любая глубина: `User Data\<профиль>`, `Partitions\<имя>`), затем `Extensions` при манифесте синхронизации, затем верхние потомки из `claimed` (относительные пути как в §4.2.4, без порога остатка). Манифест синхронизации — папка `Sync Data` рядом с `Extensions`; `Extensions`/`Sync Data` и учётные данные ищутся в корне профиля и в каждой его некэшевой подпапке. Учётные данные → `sensitivity = High`, тег `browser-credentials`, `notes_key = "heuristic.dpapi_bound"` (не ScanIssue). `title` = `<имя папки-кандидата> — heuristic.title.app_data` (суффикс GUID/хэш и −0.1 к confidence по §5), `message_args = { app }`, теги — из маркеров сводки. Ошибки чтения H-WEB сообщает только при выключенном H-UNK; нечитаемый кандидат не попадает в `claimed`. Если H-WEB взял `User Data`, H-UNK создаёт находку на остаток папки приложения по §4.2.4.

### 4.7 Классификация по маркерам

`classify_by_markers` — упорядоченный список правил, первое совпадение побеждает:

| # | Условие на `FolderSummary` | Категория | Confidence | reason_key |
|---|---|---|---|---|
| 1 | `junk_verdict` вернул Some | из §4.5 | из §4.5 | `heuristic.junk.*` |
| 2 | путь под `{SAVED_GAMES}` | GameSave | 0.7 | `heuristic.saved_games` |
| 3 | маркер `UnrealSaveGames` | GameSave | 0.8 | `heuristic.unreal_saves` |
| 4 | маркер `UnityGame` и есть файлы без расширения/`.dat`/`.sav`/`.json` | GameSave | 0.65 | `heuristic.unity_game` |
| 5 | расширения `.sav`, `.save`, `.sl2`, `.ess` > 20% файлов | GameSave | 0.75 | `heuristic.save_ext` |
| 6 | путь `{HOME}\.*` и `ConfigLike`, размер < 10 МБ | DevEnvironment | 0.65 | `heuristic.dotfolder` |
| 7 | `ConfigLike`, размер < 5 МБ, нет `HasExecutables` | AppConfig | 0.6 | `heuristic.config_like` |
| 8 | `SqliteFiles` и `newest_mtime` < 90 дней | AppData | 0.55 | `heuristic.sqlite_recent` |
| 9 | `DocumentHeavy` или `MediaHeavy` вне AppData | UserFiles | 0.7 | `heuristic.user_content` |
| 10 | `ProjectLike` | DevEnvironment | 0.7 | `heuristic.project_like` |
| 11 | `total_bytes == 0` или `file_count == 0` | Cache | 0.95 | `heuristic.empty` (находка **не создаётся**, только claimed) |
| 12 | иначе | Unknown | 0.0 | `heuristic.unknown` |

Пороги правил 5, 7 и 8 (`20%`, `5 МБ`, `90 дней`) вынесены в `HeuristicsConfig::thresholds` с дефолтами из таблицы; порог правила 6 (10 МБ) фиксирован. Уточнения: сравнения строгие, как в таблице; МБ = 2^20 байт. Правило 2 — путь равен `{SAVED_GAMES}` или лежит под ним (по резолвленному пути, а если папка неизвестна — по токену в начале шаблона). Правило 5 — доля по `ext_histogram` (top-15) от `file_count`. Правило 6 — прямой потомок `{HOME}`, имя начинается с точки. Правило 8 — `now − newest_mtime < sqlite_recent_days`; mtime в будущем считается недавним, без `newest_mtime` правило не срабатывает. Правило 9 — «AppData» = под `{APPDATA}`, `{LOCALAPPDATA}`, `{LOCALLOW}` или `{HOME}\AppData`. Правило 11 не срабатывает при `truncated = true`: обрезанная сводка не доказывает пустоту, папка уходит в правило 12.

### 4.8 Формирование Finding

```rust
fn make_finding(root: &Path, v: &MarkerVerdict, s: &FolderSummary, h: &'static str, env: &Environment) -> Finding {
    Finding {
        id: FindingId::for_target(&target),                // SPEC-02 §2.7
        target: Target::FileSet { root: PathTemplate::from_path(root, env), resolved: root.into(),
                                  include: v.include.clone(), exclude: v.exclude.clone() },
        category: v.category, app: guess_app(root, env),
        title: display_name(root),
        evidence: vec![Evidence { source: EvidenceSource::Heuristic { heuristic_id: h.into() },
                                  message_key: v.reason_key.into(), message_args: args_from(s), confidence: v.confidence }],
        stats: None,                                        // заполнит фаза Measure (summary уже содержит размер, но источник истины — Measure)
        sensitivity: sensitivity_for(v.category, s),
        score: None, default_selected: false, requires_elevation: false,
        tags: tags_from(s), children: vec![],
    }
}
```
Теги из маркеров: `CloudSynced` → `cloud-synced`, `GitRepo` → `git`, `UnityGame` → `unity`.

### 4.9 Конфигурация

Секция `heuristics` в `savekeeper.config.json` (SPEC-01 §4.8.2). Тип `HeuristicsConfig` определён в `sk-core::config` (SPEC-01 §4.8.2) и реэкспортируется из `sk-heuristics`; поля и дефолты ниже нормативны:
```jsonc
"heuristics": {
  "enabled": { "unk": true, "usr": true, "git": true, "junk": true, "web": true },
  "unknown_threshold": 0.6,
  "usr": { "min_weight": 20, "max_candidates": 200, "scan_all_fixed_drives": true },
  "git": { "max_depth": 6, "max_repos": 200 },
  "thresholds": { "save_ext_ratio": 0.2, "config_max_bytes": 5242880, "sqlite_recent_days": 90 }
}
```

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Нет доступа к папке-кандидату (ACL) | Находка `Unknown` с `stats.truncated = true`, `ScanIssue::Warning heuristic.access_denied`. В LLM не отправляется (нет summary). |
| Иная ошибка чтения кандидата | Как при ACL, но `ScanIssue::Warning heuristic.read_failed {error}`. Папка исчезла (NotFound) — пропуск. Нечитаемая зона — только issue (source `heuristics`). |
| Символьные ссылки и junction (`Application Data` → `AppData\Roaming`) | Не следуем (SPEC-03), кандидатом не считаем. |
| OneDrive Files On-Demand (плейсхолдеры) | SPEC-03 не читает содержимое плейсхолдеров. Кластер получает тег `cloud-synced`, а evidence поясняет, что файлы уже в облаке. |
| Огромный кандидат (> 50 ГБ) | Создаётся, тег `huge`, SPEC-09 не выберет его по умолчанию. |
| Репозиторий внутри другого репозитория (не сабмодуль) | Обе находки. Внешний получает `exclude` для внутреннего. |
| `.git` — файл (worktree / submodule gitlink) | Анализ через gix по gitdir. Если gitdir вне обхода, всё равно открываем. |
| Отмена | Эвристики проверяют токен на каждом кандидате / каждые 1000 записей обхода, возвращают частичный `HeuristicOutput`, пайплайн отдаёт `Cancelled`. |
| Кандидат уже в `claimed` частично (правило взяло `Foo\config`) | Находка на остаток с `exclude` (§4.2.4), если остаток ≥ 10% или ≥ 1 МБ. Иначе пропуск. |
| Встроенные `vendors.txt`/`extensions.yaml` не прошли валидацию (сломанная сборка, исключается тестами T-07-02) | `ScanIssue::Error heuristic.data_invalid {error}`, source `heuristics`; эвристики работают без этих данных. |
| Имя папки — GUID/хэш (`{3F2504E0-...}`, `a8f3c9...`) | Title получает суффикс `(неизвестно)`, confidence понижается на 0.1. Кандидат для LLM. |

## 6. Тестирование

- **Unit (кроссплатформенные)**, на `Environment::fake` + фикстурах `fixtures/fs/heuristics/*` (SPEC-12):
  - `classify_by_markers`: таблица ≥ 25 кейсов (по каждому правилу §4.7 + граничные пороги).
  - `junk_verdict`: все имена §4.5, регистронезависимость, `target` без `Cargo.toml` → не мусор.
  - Кластеризация §4.3.3: дерево `D:\Work\{ClientA,ClientB}\*.docx` → один кластер `D:\Work`. Дерево с доминирующим ребёнком → спуск. Корень диска → никогда. `max_candidates` → issue.
  - H-UNK: вендорный контейнер `Adobe\*` → кандидаты второго уровня. Частичное покрытие → exclude. UWP-пакет с `wgs` → GameSave.
  - H-WEB: Electron-фикстура → `include` пуст, `exclude` = кэш-строки таблицы; Chromium-фикстура (`User Data` с профилями, синхронизируемые расширения, учётные данные); вместе с H-UNK — без `Unknown` и без дублей.
  - Детерминизм: два прогона дают одинаковый JSON (insta-снапшот).
- **Git** (кроссплатформенно, репозитории создаются в tempdir через `gix`/`git` CLI в тесте): no-remote, dirty, untracked, unpushed, stash, clean, gitlink worktree, повреждённый HEAD → issue. Проверяется, что mtime файлов в `.git` не меняется после анализа (NFR-07-05).
- **Windows-only**: реальный `Environment::detect()` + скан `{LOCALAPPDATA}` без паник. Каждая top-level папка представлена ровно одной находкой или покрыта `claimed` (FR-07-02).
- **Бенчмарк** (`criterion`, ручной запуск): синтетическое дерево 1 млн файлов для H-USR (NFR-07-02).

## 7. Задачи

- [x] **T-07-01** — Каркас `sk-heuristics`: `HeuristicsCollector`, трейт `Heuristic`, `HeuristicContext`, порядок выполнения и накопление `claimed`, реэкспорт `HeuristicsConfig` (тип и дефолты §4.9 реализуются в SPEC-01 T-01-04). *Зависит:* T-01-03, T-01-04. *Готово, когда:* коллектор с пустыми эвристиками встроен в `ScanPipeline` и проходит интеграционный тест.
- [x] **T-07-02** — Данные: `vendors.txt`, `extensions.yaml` + загрузчик (include_bytes, валидация при старте). *Готово, когда:* unit-тест: все группы парсятся, нет дублей расширений.
- [x] **T-07-03** — `junk_verdict` (§4.5). *Зависит:* T-07-01. *Готово, когда:* тесты §6.
- [x] **T-07-04** — `classify_by_markers` (§4.7) с конфигурируемыми порогами. *Зависит:* T-07-03, T-02-07. *Готово, когда:* таблица ≥ 25 кейсов зелёная.
- [x] **T-07-05** — H-UNK: зоны, вендоры, UWP, частичное покрытие, `make_finding`, сбор `unknown`. *Зависит:* T-07-04, SPEC-03 `summarize`. *Готово, когда:* тесты H-UNK + FR-07-02 на фикстуре.
- [x] **T-07-06** — H-WEB. *Зависит:* T-07-01. *Готово, когда:* тест на Electron- и Chromium-фикстуре.
- [x] **T-07-07** — Общий обход корней (§4.3.1) с исключениями и агрегацией по директориям. *Зависит:* SPEC-03 `walk`. *Готово, когда:* агрегаты совпадают с эталоном на фикстуре.
- [x] **T-07-08** — H-USR: кластеризация §4.3.3, одиночные ценные файлы, лимиты. *Зависит:* T-07-07, T-07-02.
- [x] **T-07-09** — H-GIT: поиск репозиториев, `GitState` через gix (только чтение), классификация §4.4.3, exclude тяжёлых каталогов. *Зависит:* T-07-07. *Готово, когда:* git-тесты §6 + проверка неизменности `.git`.
- [ ] **T-07-10** — Дочерняя находка `git-bundle` (§4.4.4) + согласование экспортёра с SPEC-06. *Зависит:* T-07-09.
- [ ] **T-07-11** — Ключи i18n `heuristic.*` (ru, en) в общем каталоге UI (SPEC-11), включая `heuristic.junk.*` (§4.5), `heuristic.data_invalid`, `heuristic.read_failed` (§5), `heuristic.dotfile` (§4.2.1), `heuristic.uwp_saves`, `heuristic.uwp_package` (§4.2.3), `heuristic.title.unrecognized` (суффикс заголовка для GUID/хэш-имён, §5), `heuristic.web.electron`, `heuristic.web.chromium`, `heuristic.title.app_data`, `heuristic.dpapi_bound` (§4.6), `heuristic.usr.cluster`, `heuristic.usr.user_folder`, `heuristic.usr.valuable_file`, `heuristic.usr_truncated {count, limit}` (§4.3.3), `heuristic.git.no_remote`, `heuristic.git.unsaved`, `heuristic.git.clean`, `heuristic.git.failed`, `heuristic.git.not_analyzed`, `heuristic.git_failed {error}`, `heuristic.git_truncated {count, limit}` (§4.4). *Зависит:* T-07-04.
- [ ] **T-07-12** — Windows-интеграционный тест и бенчмарк H-USR. *Зависит:* T-07-05, T-07-08.
- [ ] **T-07-13** — H-JUNK над подпапками находок (§4.5, второй абзац; §4.3.1): тяжёлые подпапки-мусор внутри находок H-USR и H-UNK (в т.ч. стандартных папок) уходят в их `exclude`. *Зависит:* T-07-08. *Готово, когда:* тест: кластер и стандартная папка с `logs`/`cache`/`node_modules` внутри → `exclude` содержит их, отдельных находок-дублей нет.

## 8. Критерии приёмки

- [ ] FR-07-02: на эталонной Windows-машине каждая top-level папка зон §4.2.1 либо в `claimed`, либо даёт находку.
- [ ] На эталонной машине найдены все git-репозитории с незапушенными изменениями из вручную составленного списка.
- [ ] Ни одна папка `node_modules`/`Cache`/`GPUCache` не попадает в выбранное по умолчанию (проверяется совместно со SPEC-09).
- [ ] При выключенной LLM доля `Unknown` среди находок H-UNK ≤ 40% на эталонной машине (иначе нужно улучшать правила и маркеры).
- [ ] NFR-07-01..03 выполнены на эталонной машине.

## 9. Открытые вопросы

- ~~**Предложение к SPEC-01 §4.8.2:** добавить секцию `heuristics` (§4.9) в схему конфига.~~ **Принято:** секция есть в SPEC-01 §4.8.2, тип — в `sk-core::config`.
- **Предложение к SPEC-06:** зарегистрировать экспортёр `git-bundle` (params: `repo`), который выполняется при бэкапе, требует `git.exe` в PATH, иначе использует gix-реализацию bundle (если появится).
- **Предложение к SPEC-03 §4.1:** в `summarize` нужен флаг `SummaryLimits::max_entries` и признак `truncated`, а для H-USR — `walk` с колбэком post-order или возможность агрегировать по директориям без хранения всех записей. Нужна ли в `FsScanner` функция `walk_dirs_aggregated`?
- **Предложение к SPEC-02 §4.1:** маркер `UwpPackage` для `{LOCALAPPDATA}\Packages\*` упростил бы §4.2.3.
- Нужно ли H-USR по умолчанию обходить **все** фиксированные диски? Это медленно на больших HDD. Альтернатива: только системный диск + явный выбор дисков в UI (SPEC-11). Предварительно — все Fixed, с возможностью снять галочку.
- Нужны ли в агрегатах общего обхода §4.3.1 маркеры (`CacheLike`, `ConfigLike`, `HasExecutables`) или имена файлов папки, чтобы для кандидатов H-USR срабатывали строки 2, 4, 5 §4.5 и «рядом с `Cargo.toml`/`*.csproj`»? Сейчас сводка строится из агрегатов без маркеров. Связано: H-USR не спускается в мусор по имени (`D:\Temp\Проект`), и пользовательские файлы там видны только как находка-мусор (FR-07-07) — достаточно ли этого?
- Нужна ли находка `FileSet { include: ["*"] }` на собственные файлы корня диска / `{HOME}` с весом ≥ `min_weight` (например `{HOME}\big.pdf`)? Сейчас такие файлы не покрываются.
- Хранить ли для кластеров H-USR список топ-файлов для UI-превью? Предварительно нет, UI запрашивает листинг лениво через IPC (SPEC-11).
