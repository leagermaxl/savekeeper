# SPEC-05: Сохранения игр (Ludusavi-манифест и лаунчеры)

| Поле | Значение |
|---|---|
| ID | SPEC-05 |
| Статус | in-progress |
| Фаза | P1 |
| Крейт(ы) | `sk-games` |
| Зависит от | SPEC-01, SPEC-02, SPEC-03 |
| Используется в | SPEC-04 (токены `{STEAM}`, `{STEAM_USERID}`), SPEC-07, SPEC-09, SPEC-11 |
| Последнее изменение | 2026-10-03 (T-05-13: include `G` + `G/**` в §4.3, подстановка `{STORE_GAME_ID}`/`{GAME_DIR_NAME}` в §4.7 п. 2; T-05-12: §4.7 п. 1 — загрузка в Collect, открытые вопросы §9; T-05-11: `UpdateOutcome::Failed.fallback: Option`, поведение без фичи `embedded-manifest`, проброс фичи, ключи `about.ludusavi.*`; T-05-10: `take_issues` публичный; T-05-09: `{GAME_DIR}` → шаблон каталога установки, builder `GamesCollector`, HKCU/HKLM в FR-05-07, cloud-теги FR-05-08, §4.7 п. 3, 8, 10, 11, `app-running` отложен, задачи T-05-12, T-05-13; T-05-08: §4.6 — глубина якоря, корни для `read_dir`, API `AnchorIndex`; T-05-07: §4.5 — алиасы, fallback Steam/GOG на имя, нормализация без транслитерации, порядок при неоднозначности, пометка `game-unmatched` для SPEC-07, API `MatchIndex`; T-05-06: `GameCtx`, глобы, `<home>/AppData`, `Saved Games` и причины пропуска в §4.3, соответствие store → лаунчер в FR-05-05; T-05-05: `enrich_with_registry` в §4.1; T-05-04: условие обнаружения EA, тексты ключей games в T-05-09, детекторы Epic/GOG/Ubisoft/EA/Battle.net/Xbox в §4.1 и §4.4, находки лаунчеров, issue `launcher_file_unreadable` в §5; T-05-02: FR-05-11 ресурс снапшота, варианты `GamesError`, детали `ManifestStore` в §4.1, ключи issue манифеста в §5, встроенный снапшот из `third_party/ludusavi`; T-02-10: общий RegistryReader и PathTemplate::specialize в sk-core; §4.1: реестр из sk-core, T-05-04 и T-05-09 зависят от T-02-10; T-05-03: detect_with_issues, SteamDetector, RegistryReader, детали Steam §4.4, ключи issue §5; T-05-01: API разбора манифеста, конкретные типы, параллельный разбор, бенч; §5: манифест с 0 игр; T-04-05: специализация `{STEAM_USERID}` в шаблоне находки, §4.7, §5); 2026-10-02 (§6: пути фикстур `fixtures/samples/...` и 20 игр, как в SPEC-12 §4.3; 2026-10-01: §4.3: `<game>`, `<storeGameId>` → токены по SPEC-02 §3.2; решение по лицензии манифеста) |

## 1. Цель

Найти сохранения и конфиги игр на машине, используя открытый манифест
[Ludusavi](https://github.com/mtkennerly/ludusavi-manifest) (данные PCGamingWiki, более
20 000 игр) и сведения из лаунчеров (Steam, Epic, GOG, Ubisoft, EA, Battle.net, Xbox).
Результат — находки `game_save` / `game_config` с привязкой к игре (`AppRef`) и понятным
объяснением.

## 2. Область

### 2.1 Входит
- Загрузка, кэширование и парсинг манифеста Ludusavi. Встроенный офлайн-снапшот.
- Маппинг плейсхолдеров Ludusavi на токены SPEC-02 §3.1.
- Детект лаунчеров и установленных игр → `Environment.launchers` (обогатитель `enrich`).
- Токены `{STEAM}`, `{STEAM_USERID}`, `{GAME_DIR}`.
- `GamesCollector: Collector`: файловые и реестровые записи манифеста.
- Пометки облачных сохранений (Steam Cloud и др.).

### 2.2 Не входит
- Эмуляторы, Minecraft, Steam userdata/скриншоты: правила SPEC-04 §4.7.9.
- Бэкап самих установленных игр (они `reinstallable`, SPEC-07/09 их только помечают).
- Linux/Proton-пути манифеста (`<xdgData>`, `<xdgConfig>`, `when: os: linux`): игнорируются.

## 3. Требования

### 3.1 Функциональные
- **FR-05-01** — Манифест скачивается с `config.games.manifest_url` с условным запросом (`If-None-Match: <etag>`). Обновление не чаще `update_interval_hours`, если `auto_update: true`.
- **FR-05-02** — При недоступности сети используется кэш. Если кэша нет, используется встроенный снапшот (`include_bytes!` сжатого zstd манифеста). Версия и дата снапшота видны в UI и отчёте.
- **FR-05-03** — Для установленных игр (обнаруженных лаунчерами) проверяются все их записи, включая `<base>`/`<game>`/`<storeUserId>`.
- **FR-05-04** — Для остальных игр манифеста проверяются только записи, которые раскрываются без `<base>`/`<game>`/`<storeGameId>` (сохранения в AppData/Documents остаются после удаления игры, и это частый сценарий). Проверка идёт через индекс §4.6, не перебором.
- **FR-05-05** — Учитываются только записи `when` без ограничений или с `os: windows`. `store` в `when` учитывается, если соответствующий лаунчер есть, либо store не указан. Соответствие store → лаунчер: steam→steam, epic→epic, gog/gogGalaxy→gog, ea/origin→ea, uplay→ubisoft, microsoft→xbox; prime, heroic, legendary, lutris, other и неизвестные — лаунчера нет, условие не выполняется. `os` кроме `windows` (в т.ч. `dos`) — не выполняется.
- **FR-05-06** — Теги Ludusavi `save` → `Category::GameSave`, `config` → `Category::GameConfig`. Без тегов → `GameSave` с confidence 0.7.
- **FR-05-07** — Реестровые записи манифеста (`registry:`) → `Target::Registry` (только HKCU. HKLM → issue Info, не сохраняем). HKCU-записи проверяются и у неустановленных игр (находка с тегом `not-installed`). HKLM: если ключ существует у установленной игры — Info `issue.games.registry_hklm_skipped` {game, key}, source `games`; у неустановленных игр HKLM молча пропускается. Ключи с `<…>`, `*`, `?` пропускаются.
- **FR-05-08** — Если у игры в манифесте `cloud: { steam: true, ... }`, у находки тег `cloud-steam` (и т.п.). **Находка всё равно создаётся**: облако бывает выключено или неполно. SPEC-09 учитывает это в скоринге. Теги: `cloud-steam`, `cloud-epic`, `cloud-gog`, `cloud-ea` (origin), `cloud-ubisoft` (uplay) — по id лаунчеров, как `cloud-xbox`.
- **FR-05-09** — Все корни найденных сохранений и каталоги установки игр (`{GAME_DIR}`) попадают в `claimed_paths`. Каталоги установки дополнительно дают находку `Category::Reinstallable` (не выбрана по умолчанию), чтобы UI показал «игра X, 60 ГБ, переустанавливается из Steam».
- **FR-05-10** — Атрибуция: в UI «О программе», в `report.html` и в `THIRD_PARTY_NOTICES.md` указывается «Game save data: Ludusavi Manifest (MIT, github.com/mtkennerly/ludusavi-manifest) / PCGamingWiki (CC BY-NC-SA 3.0)», со ссылкой на текст лицензии и с пометкой, что встроенный снапшот не изменялся (или перечнем изменений, если он сжат или отфильтрован).
- **FR-05-11** — Условия CC BY-NC-SA для встроенного снапшота: (1) SaveKeeper распространяется **бесплатно и некоммерчески**; (2) снапшот лежит в бинарнике отдельным ресурсом (zstd-файл, который `build.rs` собирает из `third_party/ludusavi/manifest.yaml`, §4.1) и сохраняет свою лицензию (ShareAlike относится к данным, а не к коду SaveKeeper); (3) атрибуция по FR-05-10. Если проект станет коммерческим, сборка выполняется с `--no-default-features` без фичи `embedded-manifest`, и манифест только скачивается.

### 3.2 Нефункциональные
- **NFR-05-01** — Парсинг полного манифеста (~40 МБ YAML) ≤ 3 с, в фоне, параллельно фазе Environment. Кэш распарсенного индекса в бинарном виде (`bincode`/`postcard`) в `cache/ludusavi-index.bin`, инвалидация по etag. Файл ≥ 2 МБ режется по строкам ключей верхнего уровня и разбирается в нескольких потоках, только если вся раскладка проходит белый список: строки режутся по `\n`, нет `\r` без следующего `\n` и байтов NUL; до первой записи — только BOM в начале, пустые строки, комментарии и один `---`; дальше каждая строка, начинающаяся не с пробела, — начало записи (ключ в колонке 0, затем `:` и пробел/таб/конец строки), комментарий или пустая. Иначе — последовательный разбор. Он же выполняется при ошибке чанка, ошибке или панике потока, несовпадении числа игр в чанке с числом строк-ключей и повторе имени игры; результат и ошибки идентичны последовательному. Бюджет парсера (последовательный): события/узлы ≤ 2 × размер входа + 64 КиБ, байты скаляров/комментариев ≤ размер + 64 КиБ, лимиты алиасов — по умолчанию. Бюджет чанка — последовательный, умноженный на долю чанка во входе; лимиты алиасов, якорей и merge-ключей у чанка 0 (любой из них → последовательный разбор).
- **NFR-05-02** — Весь коллектор ≤ 5 с при 200 установленных играх и прогретом кэше ФС.
- **NFR-05-03** — Память под индекс ≤ 150 МБ.

## 4. Дизайн

### 4.1 Публичный API

```rust
pub struct Manifest { pub games: HashMap<String, GameEntry>, pub meta: ManifestMeta }
pub struct ManifestMeta { pub source: ManifestSource, pub etag: Option<String>, pub fetched_at: Option<OffsetDateTime>, pub games: usize }
pub enum ManifestSource { Downloaded, Cache, Embedded { snapshot_date: String } }
impl Manifest {
    pub fn parse(yaml: &[u8], source: ManifestSource) -> Result<Manifest, GamesError>;   // разбор полного файла
}
#[non_exhaustive]
pub enum GamesError { ManifestParse(Box<serde_saphyr::Error>), Io(std::io::Error), Cancelled } // сетевые сбои — не ошибки, а UpdateOutcome::Failed / issues

pub struct ManifestStore { cache_dir: PathBuf, url: String, /* + приватные: auto_update, интервал, таймаут, встроенный снапшот, накопленные issues */ }
impl ManifestStore {
    pub fn new(cfg: &Config, data_dir: &Path) -> Self;
    pub async fn load(&self, allow_network: bool, cancel: &CancellationToken) -> Result<Arc<Manifest>, GamesError>;
    pub async fn update(&self, force: bool) -> Result<UpdateOutcome, GamesError>;   // CLI `manifest update`
    pub fn take_issues(&self) -> Vec<ScanIssue>;  // issues последних load/update (§5) для GamesCollector и CLI (`manifest update`, код 3)
}
pub enum UpdateOutcome { NotModified, Updated { games: usize }, Failed { reason: String, fallback: Option<ManifestSource> } }
// fallback — что load использует вместо загрузки: кэш или снапшот; None, если нет ни того, ни другого (только сборка без фичи embedded-manifest, FR-05-11)

// Лаунчеры
pub trait LauncherDetector: Send + Sync {
    fn id(&self) -> &'static str;                               // "steam", "epic", "gog", "ubisoft", "ea", "battlenet", "xbox"
    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo>;
    fn detect_with_issues(&self, fs: &dyn FsScanner, env: &Environment) -> (Option<LauncherInfo>, Vec<ScanIssue>); // detect + проблемы по пути (ScanIssue::Info, source "games.<id>", §4.4); по умолчанию (self.detect(..), vec![])
}
pub struct SteamDetector;                                       // new() — системный реестр; with_registry(Arc<dyn RegistryReader>) — тесты
pub struct EpicDetector; pub struct XboxDetector;               // new(); реестр не нужен
pub struct GogDetector; pub struct UbisoftDetector; pub struct EaDetector; pub struct BattleNetDetector; // new() — системный реестр; with_registry(..) — тесты
// все — LauncherDetector, реэкспорт из корня крейта
pub const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;
pub fn enrich(env: &mut Environment, fs: &dyn FsScanner) -> Vec<ScanIssue>; // вызывает все детекторы (SPEC-02 §3.3), возвращает их issues
pub fn enrich_with_registry(env: &mut Environment, fs: &dyn FsScanner, registry: Arc<dyn RegistryReader>) -> Vec<ScanIssue>; // то же с заданным реестром (MemRegistry в тестах); enrich = с SystemRegistry. Детекторы по порядку steam, epic, gog, ubisoft, ea, battlenet, xbox видят env до вызова; записи env.launchers с id детекторов заменяются найденным (не найден — удаляются), записи с другими id сохраняются; повторный вызов даёт тот же результат.

// Чтение реестра. После T-02-10 RegistryReader / SystemRegistry / MemRegistry — из sk-core::registry
// (SPEC-02 §3.4: key_state, string_value, dword_value, subkeys); свои registry.rs, win.rs и winreg
// в sk-games удаляются, SteamDetector::with_registry принимает Arc<dyn sk_core::registry::RegistryReader>.
pub trait RegistryReader: Send + Sync { fn string_value(&self, hive: RegHive, key: &str, name: &str) -> Option<String>; } // REG_SZ/REG_EXPAND_SZ, иначе None; только KEY_READ (будет перенесён в sk-core, SPEC-02 T-02-10)
pub struct SystemRegistry;                                      // вне Windows значений нет
pub struct MemRegistry;                                         // new(), set_string(); имена без учёта регистра

// Типы для Environment.launchers (определены в `sk-core::env`, SPEC-02 §3.3; `sk-games` их реэкспортирует)
pub struct LauncherInfo {
    pub id: String,
    pub root: Option<PathBuf>,
    pub user_ids: Vec<StoreUser>,                              // Steam: id3 + id64
    pub games: Vec<InstalledGame>,
}
pub struct StoreUser { pub id: String, pub alt_id: Option<String>, pub name: Option<String> }
pub struct InstalledGame {
    pub store_game_id: String,                                 // steam appid, epic AppName, gog productId
    pub name: String,                                          // как в лаунчере
    pub install_dir: PathBuf,
    pub size_bytes: Option<u64>,                               // SizeOnDisk из acf
    pub manifest_key: Option<String>,                          // сопоставленное имя в Ludusavi (§4.5)
}

pub struct GamesCollector { store: ManifestStore }            // new(store) — реестр SystemRegistry, сеть разрешена; with_registry(Arc<dyn RegistryReader>); with_network(bool) — allow_network для load (§4.7 п.1)
impl Collector for GamesCollector { /* id() = "games" */ }
```

`Manifest::parse` разбирает полный файл манифеста: `meta.etag` и `meta.fetched_at` = `None` (их заполняет `ManifestStore`), `meta.games` = число записей, включая алиасы. `GamesError` — `#[non_exhaustive]`: `ManifestParse`, `Io` (кэш, снапшот), `Cancelled`. HTTP-варианта нет: сетевые сбои дают `UpdateOutcome::Failed` или issue §5, а не ошибку.

`ManifestStore` (T-05-02):
- `new(cfg, data_dir)` принимает корень `DataDir`; кэш — `data_dir/cache` (§4.8).
- `games.auto_update: false` — `load` не ходит в сеть; сеть только через `update`.
- Время последней проверки — mtime файла `.etag`; при 304 файл перезаписывается. Проверка не чаще `update_interval_hours`.
- `update(force: true)` не шлёт `If-None-Match`.
- Таймаут 15 с — на соединение и на каждое чтение, не на всю загрузку. Размер загрузки ограничен 512 МиБ.
- Индекс `ludusavi-index.bin` (postcard) ключуется версией программы + etag + размером и mtime YAML; для встроенного снапшота — датой снапшота (индекс строится и для него). Несовпадение ключа или битый индекс — перестроение из YAML.
- Встроенный снапшот: в репозитории хранится исходник `third_party/ludusavi/manifest.yaml` и дата `third_party/ludusavi/manifest.date`; `build.rs` сжимает YAML zstd (уровень 19) в `$OUT_DIR`, крейт подключает его `include_bytes!`. Без YAML сборка падает с подсказкой. Скачивание в `build.rs` (системный `curl` в `OUT_DIR`) — только при `SK_LUDUSAVI_DOWNLOAD=1`; по умолчанию сборка офлайн.
- Без фичи `embedded-manifest` (FR-05-11) снапшота нет и `build.rs` не требует YAML: `load` использует загрузку и кэш; если нет ни того, ни другого — `GamesError::Io` с `ErrorKind::NotFound` (issue `manifest_offline` при этом сохраняется). `GamesCollector` в этом случае завершается ошибкой коллектора, скан продолжается без находок игр. CLI `manifest update` при неудаче без кэша: «scans have no manifest (no cache, no embedded snapshot)».
- Фича пробрасывается: workspace-зависимости `sk-games`/`sk-engine` подключаются без default-features; у `sk-engine` и `sk-cli` своя фича `embedded-manifest` (по умолчанию), так что `cargo build -p sk-cli --no-default-features` собирает бинарник без снапшота. Крейт `app/` (SPEC-11/14) делает так же.
- Тексты «О программе» (FR-05-10) — пространство ключей `about.ludusavi.*` (`app/src/i18n/<lang>/about.json`), проверяются тестом `sk-games/tests/attribution.rs`.
- TLS: `reqwest` с `rustls` и провайдером `ring` (бэкенд `aws-lc` не используется из-за лицензии OpenSSL).

### 4.2 Формат манифеста (подмножество, которое мы читаем)

```yaml
ELDEN RING:
  files:
    <winAppData>/EldenRing:
      tags: [save]
      when: [{ os: windows }]
    <base>/Game/*.ini:
      tags: [config]
  registry:
    HKEY_CURRENT_USER/Software/FromSoftware/ELDEN RING:
      tags: [config]
  installDir:
    ELDEN RING: {}
  steam: { id: 1245620 }
  gog: { id: 1234567890 }
  cloud: { steam: true }
  id: { flatpak: ..., gogExtra: [...], steamExtra: [...] }
  alias: "Elden Ring"          # игра-алиас → ссылка на другую запись, пропускаем
```

Serde-модель: `GameEntry { files: BTreeMap<String, FileRule>, registry: BTreeMap<String, RegRule>, install_dir: BTreeMap<String, ()>, steam: Option<{id: u32}>, gog: Option<{id: u64}>, cloud: Option<CloudFlags>, alias: Option<String>, id: Option<Ids> }`. Неизвестные поля игнорируются (манифест эволюционирует).
`FileRule { tags: Vec<String>, when: Vec<When { os, store }> }`.
Конкретные типы: `steam: Option<SteamRef { id: u32 }>`, `gog: Option<GogRef { id: u64 }>` (без `id` → `None`); `RegRule` = `FileRule`; `When { os: Option<Os>, store: Option<Store> }`; `Os` = `windows` | `linux` | `mac` | `dos` | `Unknown(String)`, `Store` = `steam` | `epic` | `gog` | `gogGalaxy` | `ea` | `origin` | `uplay` | `microsoft` | `prime` | `heroic` | `legendary` | `lutris` | `other` | `Unknown(String)` — неизвестное значение не роняет разбор; `CloudFlags { origin, epic, gog, steam, uplay: bool }`; `Ids { flatpak: Option<String>, gog_extra: Vec<u64>, steam_extra: Vec<u32> }`. `null` у записи и у коллекций = пусто. Пустой документ = пустой манифест.

### 4.3 Маппинг плейсхолдеров

| Ludusavi | Наш токен / значение | Примечание |
|---|---|---|
| `<home>` | `{HOME}` | |
| `<winAppData>` | `{APPDATA}` | |
| `<winLocalAppData>` | `{LOCALAPPDATA}` | |
| `<winLocalAppDataLow>` | `{LOCALLOW}` | |
| `<winDocuments>` | `{DOCUMENTS}` | учитывает OneDrive-перенаправление (SPEC-02) |
| `<winPublic>` | `{PUBLIC}` | |
| `<winProgramData>` | `{PROGRAMDATA}` | |
| `<winDir>` | `{WINDIR}` | исключение SPEC-03 не действует (явный корень) |
| `<base>` | `{GAME_DIR}` | каталог установки игры (`ResolveContext.game_dir`) |
| `<game>` | `{GAME_DIR_NAME}` = имя каталога установки (`install_dir.file_name()`, `ResolveContext.game_dir_name`) | токен (SPEC-02 §3.2) |
| `<root>` | корень библиотеки лаунчера (`steamapps\common`’s parent и т.п.) | подставляется строкой |
| `<storeUserId>` | `{STEAM_USERID}` для Steam, иначе `*` (glob-сегмент) | |
| `<storeGameId>` | `{STORE_GAME_ID}` = `store_game_id` установленной игры (`ResolveContext.store_game_id`) | токен (SPEC-02 §3.2) |
| `<osUserName>` | `Environment.user_name` | подставляется строкой |
| `<xdgData>`, `<xdgConfig>`, `<regHkcu>`, `<regHklm>` | не поддерживаются | запись пропускается |

Пути манифеста используют `/` (и иногда `\`), мы нормализуем в `\`. `<home>/AppData/Roaming|Local|LocalLow/…` → `{APPDATA}|{LOCALAPPDATA}|{LOCALLOW}\…`, `<home>/Saved Games/…` → `{SAVED_GAMES}\…` (без учёта регистра), чтобы якоря §4.6 и FindingId совпадали с правилами SPEC-04.

Глоб-символы Ludusavi (`*`, `**`, `?`, класс `[…]`; `[` без `]` — текст) в пути:
статическая часть до первого сегмента с глоб-символом → корень `Target::FileSet.root`, остаток через `/` → include-глобы `G` и `G/**` (глоб может совпасть с каталогом, тогда включается его содержимое: `*/saves`, `**/Saved`; `include` в SPEC-03 фильтрует только файлы); `G/**` не добавляется, если последний сегмент — `**`. В include `{`, `}` экранируются (`[{]`, `[}]`), значения `<game>`/`<storeGameId>`/`<osUserName>` подставляются экранированным текстом (токены-значения внутри include не допускаются), `<storeUserId>` → `*`.
Пример: `<winDocuments>/My Games/Skyrim/Saves/*.ess` → root `{DOCUMENTS}\My Games\Skyrim\Saves`, include `["*.ess", "*.ess/**"]`.
Если путь без `*` указывает на файл → `Target::File`.

Функция (crate-private): `translate(path, ctx: &GameCtx) -> Option<(PathTemplate, Vec<String> /*include*/)>`.
`GameCtx { game_dir: Option<PathBuf>, launcher: Option<String> /* id лаунчера игры */, root: Option<PathTemplate> /* <root>, напр. {STEAM} */, store_user_ids: Vec<String> /* id3 и id64 */, store_game_id: Option<String>, os_user_name: Option<String> }`; `GameCtx::resolve_context()` даёт `ResolveContext` (`game_dir_name` = `game_dir.file_name()`). `GameCtx::default()` — неустановленная игра. `root` — шаблон, а не путь, чтобы FindingId не зависел от машины.
`None` (запись пропускается): неподдерживаемый/неизвестный плейсхолдер; путь не с корневого плейсхолдера, `<root>` или буквы диска `X:`; корневой плейсхолдер не первым сегментом; сегменты `.`/`..`; отсутствующее в ctx значение (`<base>`/`<game>` без `game_dir`, `<root>`, `<storeGameId>`, `<osUserName>`); текст, похожий на токен (`{APPDATA}` в имени); невалидный шаблон.

Фильтр `when` (FR-05-05, crate-private `when_applies(&[When], &[LauncherInfo]) -> bool`).

### 4.4 Детект лаунчеров

| Лаунчер | Корень | Игры | Пользователи | Ограничения |
|---|---|---|---|---|
| **Steam** | `HKCU\Software\Valve\Steam\SteamPath` → fallback `{PROGRAMFILES_X86}\Steam` | `steamapps\libraryfolders.vdf` → библиотеки → `steamapps\appmanifest_<appid>.acf` (`appid`, `name`, `installdir`, `SizeOnDisk`); install dir = `<lib>\steamapps\common\<installdir>` | `<root>\userdata\<id3>` (каталоги-числа), имена из `config\loginusers.vdf` (id64 → id3 = id64 − 76561197960265728) | VDF парсер `keyvalues-parser` |
| **Epic** | `{PROGRAMDATA}\Epic\EpicGamesLauncher\Data\Manifests\*.item` (JSON) | поля `AppName`, `DisplayName`, `InstallLocation` | — | Сохранения Epic Cloud — только тег `cloud-epic`, если в манифесте |
| **GOG** | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>` (`gameName`, `path`) | + Galaxy: `{PROGRAMDATA}\GOG.com\Galaxy\storage\galaxy-2.0.db` (SQLite, read-only, `?immutable=1`) — P2, желательно | — | |
| **Ubisoft Connect** | `HKLM\…\Ubisoft\Launcher\InstallDir` → fallback `{PROGRAMFILES_X86}\Ubisoft\Ubisoft Game Launcher`; игры — `HKLM\SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs\<id>\InstallDir` | каталог `<UbisoftRoot>\savegames\<userid>\<gameid>` — отдельная находка `ubisoft.savegames` на весь каталог | подпапки savegames | сопоставление gameid→имя не требуется |
| **EA app** | `{PROGRAMDATA}\EA Desktop\InstallData\*\*.json` / `HKLM\SOFTWARE\WOW6432Node\Electronic Arts\<Game>\Install Dir` | | — | best effort |
| **Battle.net** | `{PROGRAMDATA}\Battle.net\Agent\product.db` (protobuf) — MVP: только `HKLM\...\Uninstall\*` с Publisher «Blizzard Entertainment» | | — | Сохранения в облаке, конфиги в `{DOCUMENTS}` — из манифеста |
| **Xbox / MS Store** | `{LOCALAPPDATA}\Packages\*` с `SystemAppData\wgs` | — | — | **Сохранения `wgs` хранятся в облаке и зашифрованы по контейнерам.** Создаём находку `xbox.wgs` (`game_save`, confidence 0.5, note «обычно синхронизируются через Xbox Cloud»), без расшифровки |

Steam, детали: корень — первый существующий каталог (или junction/symlink на каталог) из `SteamPath` (`/`→`\`) и `{PROGRAMFILES_X86}\Steam`; нет — лаунчер не найден, без issue. Библиотеки: корень первым, затем пути `libraryfolders.vdf` по возрастанию числового ключа (формат `"N" { "path" "…" }` и старый `"N" "…"`), повторы без учёта регистра отбрасываются; неосновная библиотека на диске, которого нет в `Environment.drives`, пропускается без обращения к ФС; относительные пути библиотек пропускаются. Игры: `appmanifest_<цифры>.acf`; `appid` из файла, иначе из имени файла; `name`, иначе `installdir`; `installdir` — одно имя каталога (не пустое, не `.`/`..`, без `\`, `/`, `:`), иначе файл отбрасывается; один `appid` в нескольких библиотеках — берётся первая. Пользователи: подкаталоги `userdata` с каноническим десятичным именем u32, по возрастанию, `StoreUser { id: id3, alt_id: Some(id64), name: PersonaName }` (`AccountName` не читается). Лимиты: `.vdf` ≤ 4 МиБ, `.acf` ≤ 1 МиБ, вложенность `{` > 64 — файл невалиден.

Другие лаунчеры, детали (T-05-04):
- **Epic:** лаунчер есть, если `{PROGRAMDATA}\Epic\EpicGamesLauncher` — каталог (он же корень). Игры — `Data\Manifests\*.item` по имени файла; нет `Manifests` — нет игр. id = `AppName`, имя = `DisplayName`, иначе `AppName`. `InstallLocation` должен быть абсолютным; `size_bytes` = `InstallSize`. Дополнение (`MainGameAppName` ≠ `AppName`) пропускается, повтор `AppName` — берётся первый.
- **GOG:** есть, если существует ключ `GOG.com\Games`. Подключ — id, `gameName` — имя (иначе имя каталога), `path` должен быть абсолютным; root = None.
- **Ubisoft:** root = `HKLM\…\Ubisoft\Launcher\InstallDir`, иначе `{PROGRAMFILES_X86}\Ubisoft\Ubisoft Game Launcher`. Игры — `Installs\<id>\InstallDir`, имя по каталогу. Пользователи — подкаталоги `<root>\savegames`. Лаунчер есть, если есть корень или ключ `Installs`.
- **EA:** root = `{PROGRAMDATA}\EA Desktop`. JSON `InstallData\*\*.json`: каталог установки — первое из `installLocation|installPath|baseInstallPath|installDir`, имя — `displayName|gameName|title`, id — `softwareId|contentId|offerId|productId`, иначе имя папки. JSON без каталога установки — не игра, без issue. Затем игры из реестра (`Install Dir`, имя `DisplayName`, иначе подключ), дедуп по каталогу установки. Формат JSON — best effort. Лаунчер есть, если есть корень или игра в реестре.
- **Battle.net:** ключи Uninstall сначала WOW6432Node, затем 64-битные, дедуп по имени записи. Publisher начинается с «Blizzard Entertainment» без учёта регистра. Запись `Battle.net` даёт корень; прочие записи с абсолютным `InstallLocation` — игры, `size_bytes` = `EstimatedSize`×1024.
- **Xbox:** есть, если у какого-либо `Packages\*` есть каталог `SystemAppData\wgs`. root = None, игр нет (нет каталога установки; каталог Packages не заявляется).
- Все реестровые детекторы: записи без допустимого каталога установки пропускаются молча.

Находки лаунчеров (`xbox.wgs`, `ubisoft.savegames`) строит `launchers::findings` (crate-private); `GamesCollector` (T-05-09) добавляет их в вывод. Общее: FileSet на весь каталог (`PathTemplate::from_path`), title `games.title.save`, `EvidenceSource::Launcher`.
- `xbox.wgs`: шаблон `{PACKAGE:<name>}\SystemAppData\wgs`; evidence `evidence.games.xbox_wgs` {package}, confidence 0.5; note `games.note.xbox_wgs`; теги [xbox, cloud-xbox]; AppRef Game, id — ASCII-slug имени из PFN, `source_ids` {xbox: PFN}.
- `ubisoft.savegames`: evidence `evidence.games.ubisoft_savegames`, confidence 0.9; теги [ubisoft]; AppRef «Ubisoft Connect» (`ubisoft-connect`, Application).

Результат детекторов → `Environment.launchers`. Ошибки детектора → `ScanIssue::Info` с `source: "games.<launcher>"`.

### 4.5 Сопоставление установленных игр с манифестом
1. Индекс `steam_id → key`, `gog_id → key` из манифеста (включая `id.steamExtra`, `id.gogExtra`). Записи-алиасы в индекс id и `installDir` не входят; их имя ведёт на целевую запись, если она есть и сама не алиас.
2. Steam/GOG: по id (`steam.id`/`gog.id`, затем `id.steamExtra`/`id.gogExtra`). Если id нет в индексе или он не число, а также для Epic и остальных — по нормализованному имени, затем по имени каталога установки (`installDir` в манифесте, без учёта регистра). Нормализация для сопоставления: удалить `™®©`, lowercase, каждая серия символов кроме букв и цифр → `-`, trim `-`. Для ASCII это id `AppRef` (SPEC-02 §2.4); не-ASCII буквы сохраняются как есть (без транслитерации). Пустой результат ни с чем не совпадает.
3. Неоднозначность (несколько записей на шаге id, имени или `installDir`) → берём запись, у которой `installDir` совпадает с именем каталога установки (без учёта регистра). Если такая одна — confidence 1.0, иначе первая из оставшихся по порядку ключей манифеста (байтовое сравнение) + confidence 0.6.
4. Несопоставленная установленная игра → находка `Reinstallable` каталога установки с тегом `game-unmatched`; этот тег и есть пометка для SPEC-07 (`PriorResults.findings`). Каталог установки такой игры **не** добавляется в `claimed_paths`, чтобы эвристики SPEC-07 исследовали его (маркер `UnrealSaveGames` — `Saved\SaveGames` внутри `GAME_DIR`). Отдельного поля в `CollectOutput` нет.

Реализация (crate-private, T-05-07): `MatchIndex::new(&Manifest)`, `match_game(launcher_id, &InstalledGame) -> Option<GameMatch { key, confidence, by: StoreId|Name|InstallDir }>`, `annotate(&mut [LauncherInfo])` заполняет `InstalledGame.manifest_key`. Находки п. 4 строит `GamesCollector` (T-05-09).

### 4.6 Индекс для неустановленных игр (FR-05-04)
Перебор 20 000+ игр × их путей с `exists()` — это ~100 000 обращений к ФС. Так делать нельзя.

1. При построении индекса для каждой записи без `<base>`, `<game>`, `<root>`, `<storeGameId>` вычисляем **якорь** = корень шаблона (токен или `X:`) + первые 2 статических сегмента, если их ≥ 2, иначе 1: `{DOCUMENTS}\My Games\Skyrim`, `{SAVED_GAMES}\Publisher\Game`, `{LOCALLOW}\Team Cherry\Hollow Knight`, `{APPDATA}\EldenRing`. Записи без статического сегмента после корня — «широкие» (п. 5).
2. `anchor_index: HashMap<AnchorKey /*lowercase*/, Vec<(game_key, rule_idx)>>`.
3. На скане `read_dir` каждого корня, у которого есть якоря в индексе (`{APPDATA}`, `{LOCALAPPDATA}`, `{LOCALLOW}`, `{DOCUMENTS}`, `{SAVED_GAMES}`, `{HOME}`, `{PUBLIC}`, `{PROGRAMDATA}`, `{WINDIR}`, буквы дисков), и `read_dir` тех его подкаталогов, с которых начинаются двухсегментные якоря (так получаются `{DOCUMENTS}\My Games`, `{PUBLIC}\Documents`); собираем множество существующих якорей. Нечитаемый или отсутствующий каталог пропускается.
4. Пересечение с индексом → кандидаты. Только для них — полная проверка пути (`resolve` + `exists`/`measure` include).
5. Якорь со `*` в первом сегменте (`<winAppData>/*/Saves`) → в список «широких» записей, проверяются только если игра установлена.

Сложность: O(число записей в базовых каталогах + кандидатов), обычно < 2000 обращений. Крупнейший якорь реального манифеста — `{APPDATA}\Godot\app_userdata` (~180 игр); при необходимости для таких якорей можно добавить третий сегмент.

Реализация (crate-private, T-05-08): `AnchorIndex::new(&Manifest)` — записи `translate(.., &GameCtx::default())`; записи, у которых `when` не выполним на Windows ни при каком наборе лаунчеров, не индексируются; `find(fs, env, installed: &HashSet<&str>, cancel) -> Result<Vec<AnchorHit { rule, paths }>, GamesError>` — `when` проверяется по `env.launchers`, игры из `installed` пропускаются, отмена → `GamesError::Cancelled`, порядок — по ключу игры, затем по ключу `files` (байтово); `wide()` — широкие записи.

### 4.7 Алгоритм `GamesCollector::collect`
1. `manifest = store.load(allow_network = !offline)` — вызывает `GamesCollector::collect` в фазе Collect (`offline` = `!ScanPipeline::with_network`, SPEC-01 §4.4). Параллельная загрузка с фазой Environment отложена (§9).
2. Для каждой установленной игры (`env.launchers[*].games`): сопоставить (§4.5) → для каждой `files`-записи с учётом `when` → `translate` с `GameCtx` (§4.3) → `resolve` → существующие пути → Finding. `{GAME_DIR}` в начале шаблона находки заменяется шаблоном каталога установки (`PathTemplate::from_path(install_dir)`, напр. `{STEAM}\steamapps\common\Celeste\Saves`): иначе записи `<base>/…` разных игр дают один `FindingId`, и находка не раскрывается без контекста игры. Перед этим `{STORE_GAME_ID}`/`{GAME_DIR_NAME}` заменяются значениями как текст (иначе `<game>`/`<storeGameId>` разных игр дают один `FindingId`); значение, которое не может остаться текстом шаблона (пустое, с `\`/`/`, `.`/`..`, похожее на токен), — запись пропускается. Перед созданием находки мульти-значные токены специализируются, как в SPEC-04 §4.5 и SPEC-02 §3.2: `{STEAM_USERID}` → конкретный id3 (стабилен для аккаунта); `FindingId` считается от специализированного шаблона, поэтому id находок SPEC-04 и SPEC-05 для одного пути совпадают. Каждый аккаунт Steam даёт отдельную находку.
3. Неустановленные: индекс §4.6 → Finding с тегом `not-installed` (UI: «игра удалена, но сохранения остались»). Для записей с include-глобами коллектор проверяет, что есть хотя бы один подходящий файл (останов на первом).
4. `registry`-записи (HKCU) → проверка `registry_exists` → `Target::Registry`.
5. Группировка: все files-записи одной игры с одинаковым корнем объединяются в один Finding (union include). Разные корни дают разные находки с одним `AppRef`.
6. `AppRef { id: normalize(key), name: key, kind: Game, source_ids: {ludusavi: key, steam: id?, gog: id?}, installed: Some(bool) }`.
7. Evidence: `EvidenceSource::Ludusavi { game: key, manifest_version: etag|snapshot_date }`, `message_key: "evidence.ludusavi_match"`, + для установленных `EvidenceSource::Launcher { launcher }`.
8. `claimed_paths`: корни находок + install dirs сопоставленных игр (несопоставленные — нет, §4.5 п. 4) + корни лаунчеров (`{STEAM}` целиком, кроме `userdata`, который объясняется SPEC-04, и `steamapps\common`, где заявляются только каталоги сопоставленных игр).
9. Title: `"{name} — сохранения"` / `"{name} — настройки"` (i18n `games.title.save` / `games.title.config`).
10. Evidence и категория: `evidence.ludusavi_match` {game}, confidence = 1.0 (0.7 без тегов) × confidence сопоставления; у установленной игры + `evidence.games.installed` {launcher, name}. Категория группы — `GameSave`, если хоть одна запись save/без тегов; у находок установленной игры — тег id лаунчера. Находка каталога установки: title `{name} — games.title.install_dir`, evidence `evidence.games.install_dir` {launcher, name}, note `games.note.reinstallable`, теги [лаунчер(, `game-unmatched`)]. Аргумент `launcher` переводится через `$t(games.launcher.{{launcher}})`, `reason` в issue лаунчеров — через `$t(issue.games.file_reason.{{reason}})`.
11. Сбои: ошибка `load`, кроме `Cancelled`, → `Err(CollectorError::Other)`; при отмене вывод пуст.

### 4.8 Конфигурация
`games.manifest_url`, `games.auto_update`, `games.update_interval_hours` (SPEC-01 §4.8.2). Флаг `ScanOptions.collectors.games`.
Кэш: `savekeeper-data/cache/ludusavi-manifest.yaml`, `.etag`, `ludusavi-index.bin`.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Нет сети / таймаут 15 с / HTTP ≠ 200/304 | Кэш → снапшот. `ScanIssue::Info { key: issue.games.manifest_offline, args: { reason } }`, source `games`. |
| Скачанный манифест не парсится | Не заменяем кэш, используем предыдущий, Warning `issue.games.manifest_invalid` { reason }. |
| Кэш не читается/не пишется или индекс не сохраняется | Работаем с тем, что есть (скачанное/снапшот), Warning `issue.games.manifest_cache_failed` { reason }. |
| Манифест разобран, но 0 игр (пустой/обрезанный ответ) | Как «не парсится»: кэш не заменяем, Warning (T-05-02). |
| Частично скачанный файл | Пишем во временный `*.tmp`, атомарный rename после успешного парсинга. |
| Steam установлен, но `libraryfolders.vdf` отсутствует или битый | Только основная библиотека `<root>\steamapps`. Info `issue.games.steam_libraryfolders_unreadable`. |
| Библиотека Steam на отключённом диске | Пропуск без обращения к ФС, `issue.games.steam_library_unavailable` { reason: drive_missing, drive: <буква> }; каталог библиотеки на имеющемся диске не читается — тот же ключ, reason по ошибке ФС; нет `steamapps` у основной библиотеки — не проблема. |
| `appmanifest_*.acf` не читается/битый/недопустимый `installdir` | Игра пропускается, `issue.games.steam_appmanifest_unreadable` { reason }. |
| Файл/каталог данных другого лаунчера (не Steam) не читается или невалиден | Запись пропускается, Info `issue.games.launcher_file_unreadable` { reason } (reason как у Steam), source `games.<id>`. |
| Несколько Steam-аккаунтов | `{STEAM_USERID}` раскрывается во все; в шаблоне каждой находки токен специализирован в конкретный id3 (§4.7, SPEC-02 §3.2), `FindingId` — от специализированного шаблона. Title получает суффикс имени аккаунта из `loginusers.vdf`. |
| Сохранения внутри каталога игры (`<base>/saves`) | Находка `game_save` внутри `Reinstallable`-находки каталога. SPEC-09 merge не должен поглотить её родителем (правило «Reinstallable не поглощает»). |
| Одинаковый путь у двух игр (общий движок, `<winDocuments>/My Games`) | Один FindingId → одна находка, два Evidence и `AppRef` первой игры + тег `multi-game`. |
| Огромные сохранения (> 2 ГБ: симуляторы, Minecraft-миры) | Создаём, скоринг решает про `default_selected` (SPEC-09). |
| Игра запущена | Тег `app-running` по процессу из `install_dir` (см. SPEC-04 §4.3 process snapshot). **Отложено:** `Environment.running_processes` хранит только имена exe без путей, привязать к `install_dir` нельзя; см. §9. |
| Манифест содержит путь с `..` | Отбрасываем запись (защита от выхода за корень) + debug-лог. |

Issue Steam: reason ∈ not_found, access_denied, locked, too_large, cloud_only, cancelled, io, invalid, drive_missing; path — `PathTemplate::from_path`; source `games.steam`.

## 6. Тестирование

- Unit: `translate` для каждого плейсхолдера §4.3, `*`-разбиение на root+include, фильтр `when`.
- VDF/ACF-парсинг на фикстурах `fixtures/samples/steam/{libraryfolders.vdf, appmanifest_1245620.acf, loginusers.vdf}`, Epic `.item`.
- Индекс якорей: мини-манифест из 20 игр (`fixtures/samples/ludusavi/manifest-mini.yaml`, SPEC-12 §4.3) + `MemFs` → ожидаемые кандидаты. Проверка, что число `exists`-вызовов ≤ порога (счётчик MemFs).
- Сопоставление по имени: `"ELDEN RING™"` ↔ `"ELDEN RING"`, неоднозначность.
- `ManifestStore`: mock HTTP (`wiremock`): 200 + etag, 304, 500 → fallback, битый YAML → старый кэш.
- Snapshot находок `insta` для фикстуры `fixtures/fs/gamer-profile.yaml` (Steam с 3 играми, одна удалённая с сохранениями в `{LOCALLOW}`, Epic-игра, Skyrim в Documents).
- Windows-интеграционный (ручной чек-лист + `#[ignore]` тест): реальный Steam на машине разработчика.
- Бенч: парсинг полного манифеста → NFR-05-01. `cargo bench -p sk-games --bench manifest`; реальный файл — переменная `SK_LUDUSAVI_MANIFEST` (без неё пропуск), синтетика в памяти `SK_SYNTHETIC_MB` (по умолчанию 40). Проверка на реальном манифесте выполняется вручную (в среде агентов нет сети).

## 7. Задачи

- [x] **T-05-01** — Serde-модель манифеста §4.2, парсинг полного файла, бенч. *Зависит:* T-01-01. *Готово, когда:* реальный манифест парсится ≤ 3 с. *Проверено* 2026-10-03 пользователем: бенч на реальном манифесте — NFR-05-01 PASS.
- [x] **T-05-02** — `ManifestStore`: HTTP с ETag, атомарная запись, кэш индекса (postcard), встроенный снапшот (zstd, `build.rs` скачивает или берёт из `third_party/ludusavi/manifest.yaml` в репо). *Зависит:* T-05-01, T-01-04.
- [x] **T-05-03** — Детектор Steam (VDF, ACF, userdata, loginusers) + токены `{STEAM}`, `{STEAM_USERID}` в `PathTemplate::resolve`. *Зависит:* T-03-06, T-02-03.
- [x] **T-05-04** — Детекторы Epic, GOG (реестр), Ubisoft, EA, Battle.net (Uninstall), Xbox (`wgs`-находка). *Зависит:* T-05-03, T-02-10.
- [x] **T-05-05** — `enrich(env, fs)` (возвращает issues детекторов: `pub fn enrich(env: &mut Environment, fs: &dyn FsScanner) -> Vec<ScanIssue>`, §4.1) и интеграция в фазу Environment `sk-engine`. *Зависит:* T-05-04, T-01-06.
- [x] **T-05-06** — `translate` + фильтр `when` + `{GAME_DIR}`. *Зависит:* T-05-01, T-02-03.
- [x] **T-05-07** — Сопоставление установленных игр с манифестом (§4.5). *Зависит:* T-05-05, T-05-06.
- [x] **T-05-08** — Индекс якорей для неустановленных игр (§4.6). *Зависит:* T-05-06. *Готово, когда:* тест на счётчик `exists`.
- [x] **T-05-09** — `GamesCollector` (§4.7): группировка, реестр, cloud-теги, claimed_paths, Reinstallable-находки каталогов, находки лаунчеров (§4.4); ru/en тексты всех ключей `issue.games.*`, `evidence.games.*`, `games.*` (формат ресурсов как в SPEC-04 T-04-11) с тестом покрытия ключей. *Зависит:* T-05-07, T-05-08, T-01-03, T-02-10.
- [x] **T-05-10** — CLI `manifest update` + вывод источника манифеста в `savekeeper-cli env`. *Зависит:* T-05-02, T-01-07.
- [x] **T-05-11** — Атрибуция (FR-05-10, FR-05-11): `THIRD_PARTY_NOTICES.md`, строки в i18n «О программе», cargo-фича `embedded-manifest` (включена по умолчанию). Перед первым релизом сверить актуальную лицензию в README `ludusavi-manifest` и зафиксировать её в `THIRD_PARTY_NOTICES.md`. *Зависит:* T-05-02.
- [x] **T-05-12** — Регистрация `GamesCollector` в конвейере: `sk-engine` создаёт `ManifestStore` из `Config`/`DataDir` и добавляет коллектор (id `games`, `CollectorToggles.games`), `with_network(!offline)`; `MemRegistry` в тестах через `with_games_registry`; `issues` стора попадают в отчёт. Заголовок находок лаунчеров — `{app.name} — games.title.save`. *Зависит:* T-05-09, T-01-06. *Готово, когда:* `sk-cli scan` на профиле `gamer` выдаёт находки игр, интеграционный тест в `sk-engine`.
- [x] **T-05-13** — Глобы Ludusavi по каталогам (`*/saves`, `**/Saved`): include сейчас считает только файлы; сопоставлять глоб с каталогом и включать его содержимое (в `translate` — глоб + `/**`, либо поддержка в `measure`, SPEC-03). Также `{STORE_GAME_ID}`/`{GAME_DIR_NAME}` в шаблонах находок установленных игр подставлять текстом (как `{GAME_DIR}`, §4.7 п. 2), чтобы `<game>`/`<storeGameId>` разных игр не давали один `FindingId`. *Зависит:* T-05-09.

## 8. Критерии приёмки

- [ ] На машине разработчика найдены сохранения всех установленных игр, у которых есть записи в манифесте (сверка с `ludusavi backup --preview` — эталон).
- [ ] Удалённая игра с оставшимися сохранениями в AppData находится и помечена `not-installed`.
- [ ] Офлайн-запуск без кэша работает на встроенном снапшоте, UI показывает дату снапшота.
- [ ] NFR-05-01..03 выполнены.

## 9. Открытые вопросы

- Загрузка манифеста параллельно с фазой Environment (§4.7 п. 1): нужен API `GamesCollector`/`ManifestStore` для предзагрузки; сейчас — в Collect.
- Без фичи `embedded-manifest` и без кэша `load` падает: `GamesCollector` возвращает `Err`, и issue `manifest_offline` стора теряется (в отчёт попадает только `collector.failed`). Решение — вернуть issues вместе с ошибкой (изменение `Collector`, SPEC-01 §4.3) или выдавать пустой вывод с issues вместо `Err`.
- Флаг `scan --offline` в CLI (SPEC-01 §4.9) пока не предусмотрен; сеть ограничивает только `games.auto_update`.

- Тег `app-running` (§5): нужен путь exe в `Environment.running_processes` (SPEC-02 §3.3). До этого не реализуется.

- ~~Лицензирование снапшота~~ **Решено (2026-10-01):** проект некоммерческий, поэтому снапшот встраивается на условиях FR-05-11. Перепроверка текста лицензии входит в T-05-11.
- ~~**Предложение к SPEC-02 §3.3:** типы `LauncherInfo`, `StoreUser`, `InstalledGame` должны жить в `sk-core` (поле `Environment.launchers`), иначе `sk-core` зависит от `sk-games`. Предлагаю перенести их определения в SPEC-02 §3.3 как есть из §4.1.~~ **Решено: SPEC-02 §3.3** (типы определены в `sk-core::env`, `sk-games` их реэкспортирует).
- **Предложение к SPEC-02 §3.2:** `ResolveContext` дополнить `store_game_id: Option<String>` и `game_dir_name: Option<String>` (для `<storeGameId>`, `<game>`), либо подставлять их строкой до `PathTemplate::parse` (текущий план). Нужно зафиксировать одно решение.
- **Предложение к SPEC-02 §2.2:** `Target::FileSet.include` поддерживает несколько глобов — ок. Но для union нескольких записей манифеста с разными `tags` в одной находке теряется разбиение save/config. Допустимо (категория = save, если есть хоть один save).
- GOG Galaxy SQLite: нужна ли зависимость `rusqlite` (bundled) ради одного лаунчера? Отложить на P2, реестра достаточно для MVP.
- Нужен ли режим «проверить все 20 000 игр полностью» (как Ludusavi preview)? Предложение: скрытый флаг CLI `--games-exhaustive` для сверки качества индекса.
