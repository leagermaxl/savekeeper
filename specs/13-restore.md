# SPEC-13: Восстановление из бэкапа

| Поле | Значение |
|---|---|
| ID | SPEC-13 |
| Статус | approved |
| Фаза | P4 (post-MVP) |
| Крейт(ы) | `sk-restore` (оркестрация — `sk-engine::RestoreJob`), экран в `app/` |
| Зависит от | SPEC-01, SPEC-02, SPEC-06, SPEC-10, SPEC-11, SPEC-14 |
| Используется в | — |
| Последнее изменение | 2026-10-01 (закрыт вопрос по `environment.drives`: SPEC-02 §6) |

## 1. Цель

На новой (переустановленной) системе вернуть данные из бэкапа SaveKeeper на свои места.
Шаблоны путей раскрываются под нового пользователя, букву диска и расположение OneDrive.
Настройки реестра, Wi-Fi и драйверы применяются, программы переустанавливаются через
`winget import`. Всё делается прозрачно: сначала **план** (что куда пойдёт и что
конфликтует), потом выполнение, отчёт в конце. Существующие файлы без явного решения
пользователя не перезаписываются.

## 2. Область

### 2.1 Входит
- Открытие бэкапа: zip, dir, `.zip.age` (расшифровка).
- Проверка манифеста (`schema_version`, `status`, целостность blake3).
- Построение плана восстановления: resolve `PathTemplate` на текущей машине, обнаружение конфликтов.
- Режимы: dry-run, стратегии конфликтов (skip / overwrite / rename / newer-wins), выборочное восстановление.
- Применение: файлы, `.reg` (с предварительным бэкапом текущих значений), `winget import`, Wi-Fi профили, драйверы (с повышением прав).
- Отчёт о восстановлении, журнал для отката файлов.
- Экран «Восстановление» в UI.

### 2.2 Не входит
- Восстановление бэкапов других программ (Ludusavi, GameSave Manager).
- Слияние содержимого файлов (например, объединение двух `settings.json`).
- Полный откат изменений реестра и драйверов (частичный — только для реестра, см. §4.6).
- Автоматическая установка лаунчеров, не доступных через winget.

## 3. Требования

### 3.1 Функциональные
- **FR-13-01** — Поддерживаются бэкапы `schema_version = 1` (SPEC-10 §4.4). Более новая версия — отказ с сообщением «Обновите SaveKeeper».
- **FR-13-02** — `status` ≠ `complete`: предупреждение (для `incomplete`/`verify_failed` — баннер, восстановление всё равно разрешено).
- **FR-13-03** — Перед применением проверяются blake3 всех выбранных записей (можно отключить для dir-бэкапов с опцией «быстро»). Несовпадение исключает запись из плана с ошибкой `corrupted`.
- **FR-13-04** — План строится без изменений системы (`plan()` — чистая операция чтения). Dry-run = только план + отчёт.
- **FR-13-05** — Для каждой записи определяется целевой путь через `PathTemplate::resolve` (SPEC-02 §3.2) на текущем `Environment`. Токен не раскрывается (нет Steam, нет OneDrive) → `unresolved`, запись предлагается восстановить в альтернативный путь (выбор пользователя) или пропустить.
- **FR-13-06** — Конфликты: цель существует. Классы: `identical` (тот же blake3 — пропуск без вопросов), `target_newer`, `target_older`, `type_mismatch` (файл/папка).
- **FR-13-07** — Стратегии (глобально + переопределение на находку): `skip` (по умолчанию), `overwrite` (старый файл переносится в `.savekeeper-restore-backup/<timestamp>/`), `rename` (восстанавливаемый как `name (restored).ext`), `newer_wins` (по mtime).
- **FR-13-08** — Выборочность: по находкам (чекбоксы как в SPEC-11 Results), с поиском.
- **FR-13-09** — Реестр: перед `reg import` текущее состояние ветки экспортируется в `restore-backup/<timestamp>/registry/<id>.reg`. Импорт — `reg.exe import <file>`. HKLM-ветки не импортируются никогда (в бэкапе HKLM нет, но проверяем).
- **FR-13-10** — Программы: показывается список из `winget-export.json` с чекбоксами. `winget import --import-file <filtered.json> --accept-package-agreements --accept-source-agreements --disable-interactivity` запускается только после явного согласия. Пакеты без источника winget попадают в отчёт как «установить вручную» (ссылки текстом).
- **FR-13-11** — Wi-Fi: `netsh wlan add profile filename="<xml>" user=current` для каждого выбранного профиля. Требуется служба WLAN (`wlansvc`). Если адаптера нет — `unavailable`.
- **FR-13-12** — Драйверы: `pnputil /add-driver "<dir>\*.inf" /subdirs /install` через elevation-хелпер (SPEC-14 §4.5). Отказ UAC = `elevation_denied`.
- **FR-13-13** — Порядок применения: (1) программы (winget) — опционально и первыми, потому что установщики создают свои папки конфигов, которые затем перезаписываются восстановлением; (2) файлы; (3) реестр; (4) Wi-Fi; (5) драйверы. Пользователь может запустить шаги по отдельности.
- **FR-13-14** — Предупреждение о запущенных процессах: если приложение находки запущено (по имени exe из `AppRef`/Restart Manager на целевых путях), восстановление этой находки откладывается с подсказкой «закройте X».
- **FR-13-15** — Журнал восстановления `restore-log.json` в data-dir + `.savekeeper-restore-backup/` рядом с бэкапом или в data-dir: список созданных, перезаписанных (с путём сохранённой копии) и пропущенных файлов. Команда «Откатить файлы» возвращает перезаписанные и удаляет созданные.
- **FR-13-16** — Восстановленные файлы получают исходные mtime и атрибуты из манифеста.

### 3.2 Нефункциональные
- **NFR-13-01** — Построение плана для 100 тыс. записей ≤ 10 с (без проверки хэшей).
- **NFR-13-02** — Восстановление стримит из zip, не распаковывая всё во временную папку. Для `.zip.age` расшифровка идёт во временный zip в data-dir (нужен seekable-доступ), затем он удаляется.
- **NFR-13-03** — Принцип P1 наоборот: ничего не удаляется, кроме откатываемых `created` при явном откате.

## 4. Дизайн

### 4.1 Публичный API

```rust
// sk-restore
pub struct BackupSource { pub path: PathBuf, pub passphrase: Option<SecretString> }

pub struct OpenedBackup { pub manifest: BackupManifest, /* reader: zip | dir, temp для age */ }
pub fn open(src: BackupSource) -> Result<OpenedBackup, RestoreError>;          // проверка format/schema_version

pub struct PlanOptions {
    pub selection: Option<BTreeSet<FindingId>>,   // None = всё
    pub default_strategy: ConflictStrategy,       // Skip
    pub overrides: BTreeMap<FindingId, ConflictStrategy>,
    pub alt_roots: BTreeMap<FindingId, PathBuf>,  // для unresolved
    pub verify_hashes: bool,
}

pub enum ConflictStrategy { Skip, Overwrite, Rename, NewerWins }

pub struct RestorePlan {
    pub findings: Vec<PlannedFinding>,
    pub programs: Option<ProgramsPlan>,           // из system/winget
    pub wifi: Vec<PlannedWifi>,
    pub drivers: Option<PlannedDrivers>,
    pub totals: PlanTotals,                       // bytes/files по действию
    pub warnings: Vec<PlanWarning>,               // DifferentUser, DifferentDriveLetter, OneDriveMissing, SchemaNewerMinor, IncompleteBackup, RunningApp{..}
}

pub struct PlannedFinding {
    pub id: FindingId, pub title: String, pub category: Category,
    pub status: PlanStatus,                        // Ready | Unresolved | Corrupted | Blocked{reason}
    pub target_root: Option<PathBuf>,
    pub actions: PlanActionsSummary,               // create N, overwrite N, skip N, rename N, identical N
    pub conflicts: Vec<Conflict>,                  // первые 200 для UI; полный список лениво (`plan_conflicts(id)`)
}

pub struct Conflict { pub entry: String /*archive_path*/, pub target: PathBuf, pub kind: ConflictKind, pub resolution: FileAction }
pub enum ConflictKind { Identical, TargetNewer, TargetOlder, TypeMismatch }
pub enum FileAction { Create, Overwrite, Rename(PathBuf), Skip, SkipIdentical }

pub fn plan(backup: &OpenedBackup, env: &Environment, opts: &PlanOptions) -> Result<RestorePlan, RestoreError>;

pub struct ApplySteps { pub programs: bool, pub files: bool, pub registry: bool, pub wifi: bool, pub drivers: bool }

pub async fn apply(backup: &OpenedBackup, plan: &RestorePlan, steps: ApplySteps,
                   elevation: &dyn ElevationBroker, events: EventSink, cancel: CancellationToken)
    -> Result<RestoreReport, RestoreError>;

pub async fn rollback_files(log: &RestoreLog, events: EventSink, cancel: CancellationToken) -> Result<RollbackReport, RestoreError>;
```
`ElevationBroker` — абстракция SPEC-14 §4.5 (запуск задачи в повышенном хелпере). В тестах мокается.

### 4.2 Сопоставление путей

1. Для каждой `ManifestFinding.target` (шаблон) выполняется `resolve(env)`:
   - одно значение → `target_root`;
   - много значений (`{STEAM_USERID}` с несколькими пользователями на новой машине) → выбор пользователя в UI (по умолчанию — пользователь с `MostRecent=1` в `loginusers.vdf`, SPEC-05);
   - пусто → `Unresolved`.
2. `ManifestEntry.source` (шаблон файла) → абсолютный путь. Относительная часть берётся из `archive_path` после `files/<id>/`, чтобы не зависеть от повторного resolve шаблонов внутри.
3. Специальные случаи:
   - Сменилось имя пользователя: шаблоны `{HOME}` раскрываются автоматически. Предупреждение `DifferentUser` информационное.
   - Документы были в OneDrive, теперь нет (или наоборот): `{DOCUMENTS}` раскрывается в актуальное место, предупреждение `OneDriveMissing`/`OneDriveNew`. Если в манифесте `known_folders.DOCUMENTS = "{ONEDRIVE}\\..."`, а у находки корень был `{ONEDRIVE}\...` (не через `{DOCUMENTS}`), и OneDrive нет → `Unresolved` с предложением `{DOCUMENTS}`.
   - `{DRIVE:D}` — диска D нет или это другой диск (метка/серийный номер отличается от `environment.drives` из манифеста) → предупреждение `DifferentDriveLetter`, предложить выбор альтернативного корня.
   - `{GAME_DIR}` — ищется установка игры по `app.source_ids` (steam appid через SPEC-05 на новой машине). Не найдена → `Unresolved` с подсказкой «установите игру и повторите».
4. Путь-цель в запрещённых местах (`{WINDIR}`, `{PROGRAMFILES}` для не-admin) → `Blocked{reason: "protected_location"}`.

### 4.3 Применение файлов
- Стрим из `zip::ZipArchive::by_name` / из файла dir → временный файл `<target>.sk-tmp` в той же папке → blake3 на лету → сверка → `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (атомарно в пределах тома).
- Для `Overwrite`: старый файл сначала **перемещается** в `.savekeeper-restore-backup/<ts>/<относительный путь от корня находки>` (на том же томе — rename, иначе копия).
- Создаются недостающие папки. Записывается `created_dirs` для отката.
- Выставляются mtime/ctime (`SetFileTime`) и атрибуты.
- Symlink-записи (`kind: symlink`) не восстанавливаются, а попадают в отчёт с `link_target`.
- Проверка отмены между файлами. При отмене текущий `.sk-tmp` удаляется, журнал остаётся консистентным (записи пишутся в `restore-log.jsonl` **до** rename, со статусом `pending` → `done`).

### 4.4 Реестр
- Для каждого `registry/<id>.reg`: (1) `reg.exe export "HKCU\<key>" <restore-backup>/<id>.reg /y` (если ключ существует); (2) `reg.exe import <file>`.
- Перед импортом `.reg` валидируется: только `HKEY_CURRENT_USER\...` в заголовках секций. Если есть другие ульи, файл отклоняется (`Blocked{reason:"foreign_hive"}`).
- Откат реестра (опционально, кнопка): `reg import` сохранённого `.reg`. Ключи, созданные импортом и отсутствовавшие раньше, при откате не удаляются (ограничение, предупреждение).

### 4.5 Программы, Wi-Fi, драйверы
- **winget:** проверка наличия `winget` (`winget --version`). Если нет, подсказка установить «App Installer» из Microsoft Store, путь отображается текстом. Фильтрация JSON по выбранным `PackageIdentifier`. Прогресс строится по stdout (`Found`, `Successfully installed`), парсинг best-effort. Итог по каждому пакету: installed / already_installed / failed (код).
- **Wi-Fi:** XML-профили из `system/wifi/*.xml` (экспорт с `key=clear`, SPEC-06). После импорта ничего не удаляется. UI предупреждает, что XML содержит пароли в открытом виде (они же и восстанавливаются).
- **Драйверы:** `system/drivers/` → задача хелпера `{ "task": "restore_drivers", "dir": "<abs>" }` (whitelist SPEC-14 §4.5). Результат — вывод `pnputil` (число добавленных/установленных пакетов). Требуется перезагрузка → флаг `reboot_required` в отчёте.

### 4.6 RestoreReport и журнал

```jsonc
// restore-log.json (data-dir/restores/<restore_id>.json)
{
  "restore_id": "uuid", "backup_id": "uuid", "started_at": "...", "finished_at": "...",
  "steps": { "programs": "done", "files": "done", "registry": "skipped", "wifi": "done", "drivers": "elevation_denied" },
  "files": { "created": 1200, "overwritten": 14, "renamed": 0, "skipped": 30, "identical": 512, "failed": 2 },
  "journal_path": "restores/<restore_id>.jsonl",   // построчно: {action, target, backup_copy?, finding_id, status}
  "restore_backup_dir": "{HOME}\\.savekeeper-restore-backup\\20261015-1010",
  "programs": [{ "id": "Microsoft.VisualStudioCode", "result": "installed" }],
  "reboot_required": false,
  "errors": [ { "code": "access_denied", "target": "{PROGRAMDATA}\\...", "message": "..." } ]
}
```

### 4.7 Экран «Восстановление» (SPEC-11)
- Welcome получает вторую кнопку «Восстановить из бэкапа». SPEC-11 FR-11-01 дополняется экраном `restore`.
- Шаги мастера: (1) выбрать файл/папку бэкапа, ввести пароль для `.age`; (2) сводка манифеста (машина-источник, дата, статус, предупреждения); (3) план: дерево находок с действиями и конфликтами, глобальная стратегия, переопределения, альтернативные пути для `Unresolved`; вкладки «Программы», «Wi-Fi», «Драйверы»; (4) «Проверить (dry-run)» / «Восстановить»; (5) прогресс по шагам; (6) итог + «Открыть отчёт» + «Откатить файлы».
- Команды Tauri: `restore_open(path, passphrase?)`, `restore_plan(options)`, `restore_conflicts(finding_id, offset, limit)`, `restore_apply(steps) -> JobId`, `restore_rollback(restore_id) -> JobId`, `list_restores()`.

```
┌ Восстановление: SaveKeeper-DESKTOP-01-20260928-1430.zip  (Windows 11, 28.09.2026) ┐
│ ⚠ Другое имя пользователя: max → maxim (пути будут скорректированы)               │
│ Стратегия при конфликте: [Пропустить ▾]                                           │
│ [x] ELDEN RING — сохранения   → {APPDATA}\EldenRing      создать 4                │
│ [x] VS Code — настройки       → {APPDATA}\Code\User      перезаписать 2 · ⚠ новее 1│
│ [ ] Steam — screenshots       ✖ Steam не установлен  [Выбрать папку…]             │
│ Программы (winget): 42 из 57 выбрано   Wi-Fi: 3   Драйверы: 18 (нужен админ)       │
│                          [Проверить (dry-run)]   [ Восстановить ]                 │
└───────────────────────────────────────────────────────────────────────────────────┘
```

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Неверный пароль age | `RestoreError::BadPassphrase`, повторный ввод без закрытия мастера. |
| Бэкап на медленной флешке | Предложить «скопировать бэкап на диск перед восстановлением» (опционально), иначе работать напрямую. |
| Целевой файл заблокирован (приложение запущено) | Ретраи как в SPEC-10 FR-10-05, затем `failed{locked}` + процессы (Restart Manager). |
| Нет места на целевом томе | Проверка в `plan()` (`PlanWarning::InsufficientSpace{volume}`), блокирует `apply` для файлов этого тома. |
| `entries` ссылается на отсутствующий в zip файл | `failed{code: missing_in_archive}`. |
| `archive_path` с `..` или абсолютный (злонамеренный бэкап) | Отклоняется при `open()` (zip-slip защита), весь бэкап помечается как `untrusted`, восстановление запрещено. |
| `.reg` с HKLM или чужим ульем | `Blocked{foreign_hive}` (§4.4). |
| winget установки требуют UAC у отдельных пакетов | Это поведение winget. Результат пакета «failed/cancelled» в отчёте. |
| Восстановление на ту же машину без переустановки | Работает. Большинство записей `identical`, пропускаются. |
| Отмена во время winget | Процесс winget завершается (job object), уже установленные пакеты остаются. Отчёт это указывает. |

## 6. Тестирование

- **Unit:** сопоставление путей (другой пользователь, OneDrive есть→нет, `{DRIVE:D}` отсутствует, несколько `{STEAM_USERID}`); классификация конфликтов; zip-slip; валидация `.reg`-заголовков.
- **Интеграционные (кросс):** бэкап фикстуры `gamer` (SPEC-10) → `FakeProfile` с другим корнем/пользователем → `plan` → `apply(files)` → `tree_hash` совпадает; повторный `apply` → всё `identical`; `Overwrite` → откат → исходное состояние.
- **Windows:** `reg import` в `RegTestKey` + откат; `SetFileTime`; заблокированная цель.
- **Manual/admin (`#[ignore]`):** winget import 2 маленьких пакетов в VM; Wi-Fi профиль; драйвер (тестовый INF) — на чистой VM перед релизом P4.
- **Совместимость:** golden-бэкапы `fixtures/backups/v1/*.zip`, созданные релизной v0.1.0, должны открываться и восстанавливаться всеми будущими версиями.

## 7. Задачи

- [ ] **T-13-01** — `open()`: zip/dir/age, проверка `format`, `schema_version`, zip-slip. *Зависит:* SPEC-10 T-10-01, T-10-12.
- [ ] **T-13-02** — Сопоставление путей §4.2 (+ предупреждения). *Зависит:* T-02-03, T-13-01.
- [ ] **T-13-03** — `plan()`: конфликты, стратегии, проверка хэшей, место на томах. *Зависит:* T-13-02.
- [ ] **T-13-04** — `apply(files)`: temp + rename, `.savekeeper-restore-backup`, атрибуты и время, журнал jsonl. *Зависит:* T-13-03.
- [ ] **T-13-05** — `rollback_files()`. *Зависит:* T-13-04.
- [ ] **T-13-06** — Реестр: экспорт текущего, валидация, `reg import`, откат. *Зависит:* T-13-03.
- [ ] **T-13-07** — Программы (winget фильтр + import + парсинг прогресса), Wi-Fi (netsh), драйверы (через `ElevationBroker`). *Зависит:* SPEC-06 §4.1, SPEC-14 T-14-05.
- [ ] **T-13-08** — `sk-engine::RestoreJob` + CLI `restore --from <path> [--dry-run] [--strategy skip|overwrite|rename|newer] [--steps files,registry,...]`. *Зависит:* T-13-04..07.
- [ ] **T-13-09** — UI: экран `restore` (мастер §4.7) и команды Tauri. *Зависит:* T-13-08, SPEC-11.
- [ ] **T-13-10** — Golden-бэкапы v1 в `fixtures/backups/v1` и тест совместимости. *Зависит:* выпуск v0.1.0.

## 8. Критерии приёмки

- [ ] Бэкап с Windows 11 (пользователь A, Documents в OneDrive) восстанавливается на чистой Windows 11 (пользователь B, без OneDrive): все файлы с разрешимыми шаблонами на своих местах, хэши совпадают.
- [ ] Dry-run не меняет систему (проверка: снапшот `tree_hash` профиля до/после и `reg export` до/после).
- [ ] Ни один существующий файл не перезаписан без стратегии `Overwrite`/`NewerWins`, а перезаписанные доступны в `.savekeeper-restore-backup` и возвращаются откатом.
- [ ] Злонамеренный zip с `../` отклоняется.

## 9. Открытые вопросы

- Где хранить `.savekeeper-restore-backup`: в `{HOME}` (скрытая папка) или в data-dir рядом с exe (может быть флешка)? Предварительно — data-dir, fallback `{LOCALAPPDATA}\SaveKeeper`.
- Предложение к SPEC-10: в `ManifestFinding` сохранять `app.process_names: Vec<String>` (имена exe) для FR-13-14 без Restart Manager по целевым путям. Требует поля в `AppRef` (SPEC-02) — предложение туда же.
- ~~Предложение к SPEC-10 §4.4: в `environment.drives` манифеста хранить серийный номер тома и метку, чтобы детектировать `DifferentDriveLetter` (сейчас `EnvironmentSnapshot` в SPEC-02 это явно не фиксирует).~~ **Принято:** `DriveSnapshot { letter, kind, fs, label, volume_serial }` в SPEC-02 §6, пример в SPEC-10 §4.4.
- Предложение к SPEC-06: экспортёр winget должен сохранять рядом `programs.csv` (Uninstall-ключи) для пакетов без winget-источника — используется в FR-13-10 отчёте «установить вручную».
- Восстанавливать ли файлы в `Unresolved` находках в `{DESKTOP}\SaveKeeper-unresolved\<title>` по умолчанию, вместо пропуска? Решить после первых пользовательских тестов.
