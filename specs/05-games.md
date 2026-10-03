# SPEC-05: Сохранения игр (Ludusavi-манифест и лаунчеры)

| Поле | Значение |
|---|---|
| ID | SPEC-05 |
| Статус | in-progress |
| Фаза | P1 |
| Крейт(ы) | `sk-games` |
| Зависит от | SPEC-01, SPEC-02, SPEC-03 |
| Используется в | SPEC-04 (токены `{STEAM}`, `{STEAM_USERID}`), SPEC-07, SPEC-09, SPEC-11 |
| Последнее изменение | 2026-10-03 (T-05-01: API разбора манифеста, конкретные типы, параллельный разбор, бенч; §5: манифест с 0 игр; T-04-05: специализация `{STEAM_USERID}` в шаблоне находки, §4.7, §5); 2026-10-02 (§6: пути фикстур `fixtures/samples/...` и 20 игр, как в SPEC-12 §4.3; 2026-10-01: §4.3: `<game>`, `<storeGameId>` → токены по SPEC-02 §3.2; решение по лицензии манифеста) |

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
- **FR-05-05** — Учитываются только записи `when` без ограничений или с `os: windows`. `store` в `when` учитывается, если соответствующий лаунчер есть, либо store не указан.
- **FR-05-06** — Теги Ludusavi `save` → `Category::GameSave`, `config` → `Category::GameConfig`. Без тегов → `GameSave` с confidence 0.7.
- **FR-05-07** — Реестровые записи манифеста (`registry:`) → `Target::Registry` (только HKCU. HKLM → issue Info, не сохраняем).
- **FR-05-08** — Если у игры в манифесте `cloud: { steam: true, ... }`, у находки тег `cloud-steam` (и т.п.). **Находка всё равно создаётся**: облако бывает выключено или неполно. SPEC-09 учитывает это в скоринге.
- **FR-05-09** — Все корни найденных сохранений и каталоги установки игр (`{GAME_DIR}`) попадают в `claimed_paths`. Каталоги установки дополнительно дают находку `Category::Reinstallable` (не выбрана по умолчанию), чтобы UI показал «игра X, 60 ГБ, переустанавливается из Steam».
- **FR-05-10** — Атрибуция: в UI «О программе», в `report.html` и в `THIRD_PARTY_NOTICES.md` указывается «Game save data: Ludusavi Manifest (MIT, github.com/mtkennerly/ludusavi-manifest) / PCGamingWiki (CC BY-NC-SA 3.0)», со ссылкой на текст лицензии и с пометкой, что встроенный снапшот не изменялся (или перечнем изменений, если он сжат или отфильтрован).
- **FR-05-11** — Условия CC BY-NC-SA для встроенного снапшота: (1) SaveKeeper распространяется **бесплатно и некоммерчески**; (2) снапшот лежит в бинарнике отдельным ресурсом (`assets/ludusavi-manifest.yaml.zst`) и сохраняет свою лицензию (ShareAlike относится к данным, а не к коду SaveKeeper); (3) атрибуция по FR-05-10. Если проект станет коммерческим, сборка выполняется с `--no-default-features` без фичи `embedded-manifest`, и манифест только скачивается.

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
pub enum GamesError { ManifestParse(Box<serde_saphyr::Error>) /* варианты HTTP/IO добавляет T-05-02 */ }

pub struct ManifestStore { cache_dir: PathBuf, url: String }
impl ManifestStore {
    pub fn new(cfg: &Config, data_dir: &Path) -> Self;
    pub async fn load(&self, allow_network: bool, cancel: &CancellationToken) -> Result<Arc<Manifest>, GamesError>;
    pub async fn update(&self, force: bool) -> Result<UpdateOutcome, GamesError>;   // CLI `manifest update`
}
pub enum UpdateOutcome { NotModified, Updated { games: usize }, Failed { reason: String, fallback: ManifestSource } }

// Лаунчеры
pub trait LauncherDetector: Send + Sync {
    fn id(&self) -> &'static str;                               // "steam", "epic", "gog", "ubisoft", "ea", "battlenet", "xbox"
    fn detect(&self, fs: &dyn FsScanner, env: &Environment) -> Option<LauncherInfo>;
}
pub fn enrich(env: &mut Environment, fs: &dyn FsScanner);        // вызывает все детекторы (SPEC-02 §3.3)

// Типы для Environment.launchers (определяются здесь, реэкспортируются из sk-core — см. §9)
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

pub struct GamesCollector { store: ManifestStore }
impl Collector for GamesCollector { /* id() = "games" */ }
```

`Manifest::parse` разбирает полный файл манифеста: `meta.etag` и `meta.fetched_at` = `None` (их заполняет `ManifestStore`), `meta.games` = число записей, включая алиасы. `GamesError` — `#[non_exhaustive]`: сейчас только `ManifestParse(Box<serde_saphyr::Error>)`, варианты HTTP/IO добавляет T-05-02.

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

Пути манифеста используют `/`, мы нормализуем в `\`. Звёздочки (`*`, `**`) в пути:
статическая часть до первого `*` → корень `Target::FileSet.root`, остаток → `include`-glob.
Пример: `<winDocuments>/My Games/Skyrim/Saves/*.ess` → root `{DOCUMENTS}\My Games\Skyrim\Saves`, include `["*.ess"]`.
Если путь без `*` указывает на файл → `Target::File`.

Функция: `fn translate(path: &str, ctx: &GameCtx) -> Option<(PathTemplate, Vec<String> /*include*/)>`.

### 4.4 Детект лаунчеров

| Лаунчер | Корень | Игры | Пользователи | Ограничения |
|---|---|---|---|---|
| **Steam** | `HKCU\Software\Valve\Steam\SteamPath` → fallback `{PROGRAMFILES_X86}\Steam` | `steamapps\libraryfolders.vdf` → библиотеки → `steamapps\appmanifest_<appid>.acf` (`appid`, `name`, `installdir`, `SizeOnDisk`); install dir = `<lib>\steamapps\common\<installdir>` | `<root>\userdata\<id3>` (каталоги-числа), имена из `config\loginusers.vdf` (id64 → id3 = id64 − 76561197960265728) | VDF парсер `keyvalues-parser` |
| **Epic** | `{PROGRAMDATA}\Epic\EpicGamesLauncher\Data\Manifests\*.item` (JSON) | поля `AppName`, `DisplayName`, `InstallLocation` | — | Сохранения Epic Cloud — только тег `cloud-epic`, если в манифесте |
| **GOG** | `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\<id>` (`gameName`, `path`) | + Galaxy: `{PROGRAMDATA}\GOG.com\Galaxy\storage\galaxy-2.0.db` (SQLite, read-only, `?immutable=1`) — P2, желательно | — | |
| **Ubisoft Connect** | `HKLM\SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs\<id>\InstallDir` | каталог `<UbisoftRoot>\savegames\<userid>\<gameid>` — отдельная находка `ubisoft.savegames` на весь каталог | подпапки savegames | сопоставление gameid→имя не требуется |
| **EA app** | `{PROGRAMDATA}\EA Desktop\InstallData\*\*.json` / `HKLM\SOFTWARE\WOW6432Node\Electronic Arts\<Game>\Install Dir` | | — | best effort |
| **Battle.net** | `{PROGRAMDATA}\Battle.net\Agent\product.db` (protobuf) — MVP: только `HKLM\...\Uninstall\*` с Publisher «Blizzard Entertainment» | | — | Сохранения в облаке, конфиги в `{DOCUMENTS}` — из манифеста |
| **Xbox / MS Store** | `{LOCALAPPDATA}\Packages\*` с `SystemAppData\wgs` | — | — | **Сохранения `wgs` хранятся в облаке и зашифрованы по контейнерам.** Создаём находку `xbox.wgs` (`game_save`, confidence 0.5, note «обычно синхронизируются через Xbox Cloud»), без расшифровки |

Результат детекторов → `Environment.launchers`. Ошибки детектора → `ScanIssue::Info` с `source: "games.<launcher>"`.

### 4.5 Сопоставление установленных игр с манифестом
1. Индекс `steam_id → key`, `gog_id → key` из манифеста (включая `id.steamExtra`, `id.gogExtra`).
2. Steam/GOG: по id. Epic/остальные: по нормализованному имени (`AppRef` id-нормализация SPEC-02 §2.4, без `™®©`, `: - _`) и по имени каталога установки (`installDir` в манифесте).
3. Неоднозначность (несколько совпадений по имени) → берём запись, у которой совпадает `installDir`, иначе первую + confidence 0.6.
4. Несопоставленная установленная игра → находка `Reinstallable` каталога установки + в `CollectOutput` пометка для SPEC-07 (эвристика `Saved\SaveGames` внутри `GAME_DIR`, маркер `UnrealSaveGames`).

### 4.6 Индекс для неустановленных игр (FR-05-04)
Перебор 20 000+ игр × их путей с `exists()` — это ~100 000 обращений к ФС. Так делать нельзя.

1. При построении индекса для каждой записи без `<base>`, `<game>`, `<root>`, `<storeGameId>` вычисляем **якорь** = токен + первые 1–2 статических сегмента: `{APPDATA}\EldenRing`, `{DOCUMENTS}\My Games\Skyrim`, `{LOCALLOW}\Team Cherry\Hollow Knight`. Для `{DOCUMENTS}\My Games\*` и `{SAVED_GAMES}\*` берём 2 сегмента (1-й слишком общий).
2. `anchor_index: HashMap<AnchorKey /*lowercase*/, Vec<(game_key, rule_idx)>>`.
3. На скане для каждого базового токена (`{APPDATA}`, `{LOCALAPPDATA}`, `{LOCALLOW}`, `{DOCUMENTS}`, `{DOCUMENTS}\My Games`, `{SAVED_GAMES}`, `{HOME}`, `{PUBLIC}\Documents`, `{PROGRAMDATA}`) делаем `read_dir` 1–2 уровней (≈ 10–30 вызовов), собираем множество существующих якорей.
4. Пересечение с индексом → кандидаты. Только для них — полная проверка пути (`resolve` + `exists`/`measure` include).
5. Якорь со `*` в первом сегменте (`<winAppData>/*/Saves`) → в список «широких» записей, проверяются только если игра установлена.

Сложность: O(число записей в базовых каталогах + кандидатов), обычно < 2000 обращений.

### 4.7 Алгоритм `GamesCollector::collect`
1. `manifest = store.load(allow_network = !offline)` (параллельно с фазой Environment через `tokio::spawn` в `sk-engine`).
2. Для каждой установленной игры (`env.launchers[*].games`): сопоставить (§4.5) → для каждой `files`-записи с учётом `when` → `translate` с `GameCtx { game_dir, store_user_ids, store_game_id }` → `resolve` → существующие пути → Finding. Перед созданием находки мульти-значные токены специализируются, как в SPEC-04 §4.5 и SPEC-02 §3.2: `{STEAM_USERID}` → конкретный id3 (стабилен для аккаунта); `FindingId` считается от специализированного шаблона, поэтому id находок SPEC-04 и SPEC-05 для одного пути совпадают. Каждый аккаунт Steam даёт отдельную находку.
3. Неустановленные: индекс §4.6 → Finding с тегом `not-installed` (UI: «игра удалена, но сохранения остались»).
4. `registry`-записи (HKCU) → проверка `registry_exists` → `Target::Registry`.
5. Группировка: все files-записи одной игры с одинаковым корнем объединяются в один Finding (union include). Разные корни дают разные находки с одним `AppRef`.
6. `AppRef { id: normalize(key), name: key, kind: Game, source_ids: {ludusavi: key, steam: id?, gog: id?}, installed: Some(bool) }`.
7. Evidence: `EvidenceSource::Ludusavi { game: key, manifest_version: etag|snapshot_date }`, `message_key: "evidence.ludusavi_match"`, + для установленных `EvidenceSource::Launcher { launcher }`.
8. `claimed_paths`: корни находок + install dirs + корни лаунчеров (`{STEAM}` целиком, кроме `userdata`, который объясняется SPEC-04).
9. Title: `"{name} — сохранения"` / `"{name} — настройки"` (i18n `games.title.save` / `games.title.config`).

### 4.8 Конфигурация
`games.manifest_url`, `games.auto_update`, `games.update_interval_hours` (SPEC-01 §4.8.2). Флаг `ScanOptions.collectors.games`.
Кэш: `savekeeper-data/cache/ludusavi-manifest.yaml`, `.etag`, `ludusavi-index.bin`.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Нет сети / таймаут 15 с / HTTP ≠ 200/304 | Кэш → снапшот. `ScanIssue::Info { key: issue.games.manifest_offline }`. |
| Скачанный манифест не парсится | Не заменяем кэш, используем предыдущий, Warning. |
| Манифест разобран, но 0 игр (пустой/обрезанный ответ) | Как «не парсится»: кэш не заменяем, Warning (T-05-02). |
| Частично скачанный файл | Пишем во временный `*.tmp`, атомарный rename после успешного парсинга. |
| Steam установлен, но `libraryfolders.vdf` отсутствует или битый | Только основная библиотека `<root>\steamapps`. Info. |
| Библиотека Steam на отключённом диске | Пропуск, Info с буквой диска. |
| Несколько Steam-аккаунтов | `{STEAM_USERID}` раскрывается во все; в шаблоне каждой находки токен специализирован в конкретный id3 (§4.7, SPEC-02 §3.2), `FindingId` — от специализированного шаблона. Title получает суффикс имени аккаунта из `loginusers.vdf`. |
| Сохранения внутри каталога игры (`<base>/saves`) | Находка `game_save` внутри `Reinstallable`-находки каталога. SPEC-09 merge не должен поглотить её родителем (правило «Reinstallable не поглощает»). |
| Одинаковый путь у двух игр (общий движок, `<winDocuments>/My Games`) | Один FindingId → одна находка, два Evidence и `AppRef` первой игры + тег `multi-game`. |
| Огромные сохранения (> 2 ГБ: симуляторы, Minecraft-миры) | Создаём, скоринг решает про `default_selected` (SPEC-09). |
| Игра запущена | Тег `app-running` по процессу из `install_dir` (см. SPEC-04 §4.3 process snapshot). |
| Манифест содержит путь с `..` | Отбрасываем запись (защита от выхода за корень) + debug-лог. |

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

- [ ] **T-05-01** — Serde-модель манифеста §4.2, парсинг полного файла, бенч. *Зависит:* T-01-01. *Готово, когда:* реальный манифест парсится ≤ 3 с.
- [ ] **T-05-02** — `ManifestStore`: HTTP с ETag, атомарная запись, кэш индекса (postcard), встроенный снапшот (zstd, `build.rs` скачивает или берёт из `third_party/ludusavi/manifest.yaml` в репо). *Зависит:* T-05-01, T-01-04.
- [ ] **T-05-03** — Детектор Steam (VDF, ACF, userdata, loginusers) + токены `{STEAM}`, `{STEAM_USERID}` в `PathTemplate::resolve`. *Зависит:* T-03-06, T-02-03.
- [ ] **T-05-04** — Детекторы Epic, GOG (реестр), Ubisoft, EA, Battle.net (Uninstall), Xbox (`wgs`-находка). *Зависит:* T-05-03.
- [ ] **T-05-05** — `enrich(env)` и интеграция в фазу Environment `sk-engine`. *Зависит:* T-05-04, T-01-06.
- [ ] **T-05-06** — `translate` + фильтр `when` + `{GAME_DIR}`. *Зависит:* T-05-01, T-02-03.
- [ ] **T-05-07** — Сопоставление установленных игр с манифестом (§4.5). *Зависит:* T-05-05, T-05-06.
- [ ] **T-05-08** — Индекс якорей для неустановленных игр (§4.6). *Зависит:* T-05-06. *Готово, когда:* тест на счётчик `exists`.
- [ ] **T-05-09** — `GamesCollector` (§4.7): группировка, реестр, cloud-теги, claimed_paths, Reinstallable-находки каталогов. *Зависит:* T-05-07, T-05-08, T-01-03.
- [ ] **T-05-10** — CLI `manifest update` + вывод источника манифеста в `savekeeper-cli env`. *Зависит:* T-05-02, T-01-07.
- [ ] **T-05-11** — Атрибуция (FR-05-10, FR-05-11): `THIRD_PARTY_NOTICES.md`, строки в i18n «О программе», cargo-фича `embedded-manifest` (включена по умолчанию). Перед первым релизом сверить актуальную лицензию в README `ludusavi-manifest` и зафиксировать её в `THIRD_PARTY_NOTICES.md`. *Зависит:* T-05-02.

## 8. Критерии приёмки

- [ ] На машине разработчика найдены сохранения всех установленных игр, у которых есть записи в манифесте (сверка с `ludusavi backup --preview` — эталон).
- [ ] Удалённая игра с оставшимися сохранениями в AppData находится и помечена `not-installed`.
- [ ] Офлайн-запуск без кэша работает на встроенном снапшоте, UI показывает дату снапшота.
- [ ] NFR-05-01..03 выполнены.

## 9. Открытые вопросы

- ~~Лицензирование снапшота~~ **Решено (2026-10-01):** проект некоммерческий, поэтому снапшот встраивается на условиях FR-05-11. Перепроверка текста лицензии входит в T-05-11.
- **Предложение к SPEC-02 §3.3:** типы `LauncherInfo`, `StoreUser`, `InstalledGame` должны жить в `sk-core` (поле `Environment.launchers`), иначе `sk-core` зависит от `sk-games`. Предлагаю перенести их определения в SPEC-02 §3.3 как есть из §4.1.
- **Предложение к SPEC-02 §3.2:** `ResolveContext` дополнить `store_game_id: Option<String>` и `game_dir_name: Option<String>` (для `<storeGameId>`, `<game>`), либо подставлять их строкой до `PathTemplate::parse` (текущий план). Нужно зафиксировать одно решение.
- **Предложение к SPEC-02 §2.2:** `Target::FileSet.include` поддерживает несколько глобов — ок. Но для union нескольких записей манифеста с разными `tags` в одной находке теряется разбиение save/config. Допустимо (категория = save, если есть хоть один save).
- GOG Galaxy SQLite: нужна ли зависимость `rusqlite` (bundled) ради одного лаунчера? Отложить на P2, реестра достаточно для MVP.
- Нужен ли режим «проверить все 20 000 игр полностью» (как Ludusavi preview)? Предложение: скрытый флаг CLI `--games-exhaustive` для сверки качества индекса.
