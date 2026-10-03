# Чек-лист приёмки SPEC-05: сохранения игр на машине разработчика

| Поле | Значение |
|---|---|
| Основание | SPEC-05 §8, критерии 1–4 |
| Объём | 1 — сверка с `ludusavi backup --preview`; 2 — удалённая игра → `not-installed`; 3 — офлайн без кэша на встроенном снапшоте; 4 — NFR-05-01..03 (замеры бенчем `sk-games --bench games`) |
| Когда выполнять | один раз перед закрытием SPEC-05; результат (отметки и цифры, без личных путей и имён аккаунтов) — в описание PR / коммита закрытия |
| Где | реальный Windows-ПК разработчика с установленными играми (Steam, Epic и др.), обычный пользователь (без «Запуск от имени администратора»), PowerShell 7 (`pwsh`) |

Принцип P1: SaveKeeper и Ludusavi в режиме `--preview` только читают. Во время проверки ничего не
удалять, не перемещать и не «чинить» в профиле. Отчёты `acc05*.json` и `ludusavi-preview.json`
содержат реальные пути — в репозиторий их не коммитить.

## 0. Подготовка

```powershell
# В корне репозитория
cargo build --release -p sk-cli
# Новая папка не на диске C:, без savekeeper-data (значит, без кэша манифеста)
New-Item -ItemType Directory E:\sk-acc05 -Force | Out-Null
Copy-Item .\target\release\savekeeper-cli.exe E:\sk-acc05\ -Force
Set-Location E:\sk-acc05
Test-Path .\savekeeper-data\cache       # False: кэша нет
```

Ludusavi (эталон критерия 1): portable-сборка с github.com/mtkennerly/ludusavi/releases, например
в `E:\ludusavi\ludusavi.exe`. При первом запуске он создаёт `%APPDATA%\ludusavi\config.yaml` и
скачивает свой манифест. Проверить, что в `roots` есть все лаунчеры этой машины (Steam, Epic, GOG,
Ubisoft, EA, …); если нет — добавить в GUI Ludusavi («Other» → «Roots» → «Find roots»).

## 1. Что уже доказано автотестами (вручную не повторять)

| Что | Тест |
|---|---|
| Профиль `gamer` через весь конвейер (`RealFs`): находки ELDEN RING (Steam, тег `steam`, `installed = true`), каталог установки `reinstallable`, Hollow Knight не установлен → все его находки с тегом `not-installed` | `crates/sk-engine/tests/games.rs` `gamer_profile_gives_game_findings` |
| Снапшот находок коллектора на `gamer-profile` (Steam, Epic, удалённая игра, Skyrim в Documents, HKCU) | `crates/sk-games/tests/games_collector.rs` `gamer_profile_findings` (insta) |
| Остатки неустановленной игры: `not-installed`, `installed = Some(false)`, HKCU-ключ | `crates/sk-games/src/collector_leftover_tests.rs` `leftovers_of_games_that_are_not_installed` |
| Без кэша загружается встроенный снапшот (`source = embedded`, дата `YYYY-MM-DD`, > 10 000 игр), второй раз — из индекса | `crates/sk-games/tests/manifest_store.rs` `real_snapshot_round_trips_through_the_index` |
| Сбой сети / 500 / таймаут → кэш → снапшот, issue `manifest_offline` | `crates/sk-games/src/store/tests.rs` `server_error_falls_back_to_cache_then_snapshot`, `stalled_server_times_out_to_the_snapshot` |
| `savekeeper-cli env` показывает встроенный снапшот | `crates/sk-cli/tests/manifest.rs` `env_prints_the_environment_and_the_embedded_snapshot` |
| Issues стора попадают в отчёт; без сети загрузки нет | `crates/sk-engine/tests/games.rs` `store_issues_reach_the_report_and_offline_skips_the_download` |
| NFR-05-01..03 на снапшоте и фейковом профиле | `cargo bench -p sk-games --bench games` (§5) |

## 2. Критерий 1: сверка с Ludusavi

### 2.1 Скан SaveKeeper

```powershell
# Свежий манифест в кэш (тот же источник, что у Ludusavi); без сети — пропустить, будет снапшот
.\savekeeper-cli.exe manifest update
.\savekeeper-cli.exe env | ConvertFrom-Json | ForEach-Object {
  $_.manifest | Format-List; $_.launchers | ForEach-Object { "{0}: {1} games" -f $_.id, $_.games.Count } }

# Скан может идти дольше минуты: коллектор игр сейчас обходит C:\Users целиком (§5.1)
.\savekeeper-cli.exe scan --llm off --out acc05.json --pretty
"exit code: $LASTEXITCODE"          # 0 или 3 (3 — есть предупреждения, смотреть issues)
$r = Get-Content acc05.json -Raw | ConvertFrom-Json
```

### 2.2 Превью Ludusavi

```powershell
E:\ludusavi\ludusavi.exe backup --preview --api | Set-Content -Encoding utf8 ludusavi-preview.json
$lu = Get-Content ludusavi-preview.json -Raw | ConvertFrom-Json -AsHashtable
$lu.overall
```

### 2.3 Сводная таблица по играм

```powershell
function GamesOf($f) {
  $f.evidence | Where-Object { $_.source.kind -eq 'ludusavi' } | ForEach-Object { $_.source.game } }
$ours = @{}
foreach ($f in $r.findings) {
  foreach ($g in @(GamesOf $f)) { if (-not $ours.ContainsKey($g)) { $ours[$g] = @() }; $ours[$g] += $f }
}
# Установленные игры: находка каталога установки (reinstallable) с AppRef игры
$inst = @{}
foreach ($f in $r.findings | Where-Object { $_.category -eq 'reinstallable' -and $_.app.kind -eq 'game' }) {
  $inst[$f.app.name] = $f
}
$luGames = @($lu.games.Keys | Where-Object { $lu.games[$_].decision -ne 'Ignored' })

$cmp = foreach ($g in (@($luGames) + @($ours.Keys) + @($inst.Keys) | Sort-Object -Unique)) {
  $o = @($ours[$g] | Where-Object { $_ -and $_.category -ne 'reinstallable' })
  $l = $lu.games[$g]
  [pscustomobject]@{
    game      = $g
    installed = $inst.ContainsKey($g)
    unmatched = $inst.ContainsKey($g) -and ($inst[$g].tags -contains 'game-unmatched')
    lu_files  = if ($l) { $l.files.Count } else { 0 }
    lu_reg    = if ($l) { $l.registry.Count } else { 0 }
    sk        = $o.Count
    sk_tags   = ($o.tags | Sort-Object -Unique) -join ','
    status    = if ($l -and $o.Count) { 'both' } elseif ($l) { 'ludusavi-only' } elseif ($o.Count) { 'sk-only' } else { 'none' }
  }
}
$cmp | Group-Object status | Format-Table Name, Count -AutoSize
$cmp | Where-Object installed | Sort-Object game | Format-Table -AutoSize -Wrap
$cmp | Where-Object { $_.status -ne 'both' -and $_.status -ne 'none' } | Sort-Object status, game | Format-Table -AutoSize -Wrap
```

`unmatched = True` — игру не удалось сопоставить с манифестом (§4.5 п. 4); она не входит в
критерий, если её нет и у Ludusavi, иначе — разобрать (имя в лаунчере, `installDir`).

Детали одной игры (пути SaveKeeper и Ludusavi рядом):

```powershell
$g = 'ELDEN RING'
$ours[$g] | ForEach-Object { [pscustomobject]@{ cat = $_.category; tags = $_.tags -join ','
  target = if ($_.target.root) { $_.target.root } elseif ($_.target.path) { $_.target.path } else { $_.target.key }
  include = $_.target.include -join ','; files = $_.stats.file_count } } | Format-Table -AutoSize -Wrap
$lu.games[$g].files.Keys; $lu.games[$g].registry.Keys
```

### 2.4 Как разбирать расхождения

| Статус | Ожидаемые причины (не ошибка) | Ошибка → задача |
|---|---|---|
| `ludusavi-only`, игра **установлена** | только HKLM-реестр (FR-05-07: не сохраняем, Info `registry_hklm_skipped`); лаунчер, которого нет в §4.4 (Heroic, Lutris, itch, другой root Ludusavi) — игра для нас не установлена | путь из `files` у Ludusavi есть на диске, а находки нет — **нарушение критерия 1** |
| `ludusavi-only`, игра **не установлена** | «широкая» запись (`*` в первом сегменте после корня, §4.6 п. 5) или запись с `<base>`/`<game>`/`<storeGameId>` (FR-05-04); Ludusavi проверяет все 20 000+ игр полностью | обычная запись AppData/Documents без `*` в первом сегменте, а находки нет — ошибка индекса §4.6 |
| `sk-only` | папка-корень без include существует, но пуста (Ludusavi считает только файлы); общий путь нескольких игр (тег `multi-game`, у Ludusavi — у одной игры) | находка на чужие файлы — разобрать запись манифеста и `translate` |

### 2.5 Таблица по установленным играм

| Игра | Лаунчер | В манифесте (`unmatched = False`) | Ludusavi нашёл (`lu_files`/`lu_reg`) | SaveKeeper нашёл (`sk`, теги) | Ок |
|---|---|---|---|---|---|
| | | | | | |

Критерий 1 выполнен, если у каждой установленной игры с записями в манифесте, для которой
Ludusavi нашёл файлы или HKCU-ключи, есть находка SaveKeeper (`game_save`/`game_config`), кроме
расхождений из колонки «ожидаемые причины» §2.4.

## 3. Критерий 2: удалённая игра → `not-installed`

```powershell
$left = $r.findings | Where-Object { $_.tags -contains 'not-installed' }
$left | ForEach-Object { [pscustomobject]@{ game = $_.app.name; installed = $_.app.installed; cat = $_.category
  target = if ($_.target.root) { $_.target.root } elseif ($_.target.path) { $_.target.path } else { $_.target.key }
  files = $_.stats.file_count } } | Sort-Object game | Format-Table -AutoSize -Wrap
"not-installed: $(@($left).Count) findings, $(@($left.app.name | Sort-Object -Unique).Count) games"
```

Ожидается: хотя бы одна игра, удалённая с этой машины, но с сохранениями в
`%APPDATA%`/`%LOCALAPPDATA%`/`LocalLow`/`Documents`/`Saved Games`; у её находок тег `not-installed`,
`installed = False`, теги лаунчера отсутствуют. Сверить со списком `ludusavi-only`/`both` §2.3, где
`installed = False`.

Если на машине нет ни одной удалённой игры с остатками, проверку можно провести на копии остатков,
созданной вручную (SaveKeeper её только читает; удалить после проверки вручную). Пример для Hollow
Knight (запись `<home>/AppData/LocalLow/Team Cherry/Hollow Knight/*.dat`), только если игра **не**
установлена и папки нет:

```powershell
$hk = Join-Path $env:USERPROFILE 'AppData\LocalLow\Team Cherry\Hollow Knight'
if (-not (Test-Path $hk)) { New-Item -ItemType Directory $hk | Out-Null; Set-Content "$hk\user1.dat" 'test' }
.\savekeeper-cli.exe scan --llm off --out acc05-left.json --pretty
(Get-Content acc05-left.json -Raw | ConvertFrom-Json).findings |
  Where-Object { $_.app.name -eq 'Hollow Knight' } | Format-List title, category, tags, @{n='installed';e={$_.app.installed}}
```

Ожидается: находка `Hollow Knight — games.title.save`, `game_save`, теги `not-installed`
(и `cloud-steam`, `cloud-gog` из манифеста), `installed = False`.

## 4. Критерий 3: офлайн без кэша на встроенном снапшоте

UI-часть критерия («UI показывает дату снапшота») ждёт SPEC-11; сейчас дата видна в
`savekeeper-cli env` и в отчёте (`evidence.source.manifest_version` у находок Ludusavi).

1. Новая папка без `savekeeper-data` (как в §0, например `E:\sk-acc05-offline`, скопировать туда exe).
2. Отключить сеть (режим «В самолёте» или отключить адаптеры). Если отключать сеть нельзя —
   вместо этого положить рядом с exe `savekeeper.config.json`, у которого загрузка всегда падает:

   ```powershell
   '{ "games": { "manifest_url": "unsupported://savekeeper.invalid/manifest.yaml", "update_interval_hours": 0 } }' |
     Set-Content -Encoding utf8 .\savekeeper.config.json
   ```

3. Проверка:

   ```powershell
   Set-Location E:\sk-acc05-offline
   Test-Path .\savekeeper-data\cache\ludusavi-manifest.yaml        # False
   (.\savekeeper-cli.exe env | ConvertFrom-Json).manifest | Format-List
   # source = embedded, snapshot_date = дата из third_party/ludusavi/manifest.date, games > 50000

   .\savekeeper-cli.exe scan --llm off --out acc05-offline.json --pretty
   "exit code: $LASTEXITCODE"
   $o = Get-Content acc05-offline.json -Raw | ConvertFrom-Json
   $o.issues | Where-Object source -eq 'games' | Format-Table severity, message_key, @{n='args';e={$_.message_args | ConvertTo-Json -Compress}} -AutoSize -Wrap
   $o.findings.evidence | Where-Object { $_.source.kind -eq 'ludusavi' } |
     ForEach-Object { $_.source.manifest_version } | Sort-Object -Unique   # одна дата снапшота
   @($o.findings | Where-Object { $_.category -in 'game_save', 'game_config' }).Count   # > 0
   Test-Path .\savekeeper-data\cache\ludusavi-index.bin              # True: индекс снапшота построен
   ```

Ожидается: скан завершается (код 0 или 3), есть находки игр, issue `issue.games.manifest_offline`
(Info, `source = games`) с причиной сбоя сети, нет `collector.failed`; `manifest_version` — дата
снапшота. Включить сеть обратно (или удалить вручную `savekeeper.config.json`).

## 5. Критерий 4: NFR-05-01..03

```powershell
# В корне репозитория; release-сборка, офлайн; индекс пишется в target\tmp\sk-games-bench (≈ 6 МБ, перезаписывается)
$env:SK_GAMES_BENCH_REAL = '1'
cargo bench -p sk-games --bench games
$env:SK_LUDUSAVI_MANIFEST = "$PWD\third_party\ludusavi\manifest.yaml"
cargo bench -p sk-games --bench manifest
Remove-Item Env:SK_GAMES_BENCH_REAL, Env:SK_LUDUSAVI_MANIFEST
```

Строки для протокола: `store cold`, `store warm`, `memory probe … index`, `memfs_200`,
`real_machine` (bench `games`), `real:` (bench `manifest`).

Замер агентом 2026-10-04 (Windows 10, release, снапшот 2026-10-03: 16,8 МБ YAML, 53 248 записей):

| NFR | Требование | Замер | Итог |
|---|---|---|---|
| NFR-05-01 | парсинг полного манифеста ≤ 3 с | `Manifest::parse` 0,28 с (медиана 0,37 с); синтетика 40 МБ — 0,60 с; `load` без кэша (распаковка zstd + разбор + запись индекса) 0,46–0,57 с; из индекса postcard 117–135 мс | выполнено |
| NFR-05-01 | «в фоне, параллельно фазе Environment» | загрузка в фазе Collect (§4.7 п. 1, отложено в §9) | **не выполнено** |
| NFR-05-02 | коллектор ≤ 5 с при 200 установленных играх, прогретый кэш ФС | `MemFs`, 200 игр Steam: 0,42–0,53 с (с загрузкой из индекса); реальная машина разработчика (20 игр): 72–85 с, второй прогон 84,8 с (§5.1) | MemFs — выполнено; реальная машина — **не выполнено** |
| NFR-05-03 | память под индекс ≤ 150 МБ | private bytes после загрузки из индекса +67,6 МБ, оценка кучи 60–69 МБ; пик всего коллектора (манифест + индексы сопоставления и якорей) +110 МБ над базой | выполнено |

### 5.1 Реальная машина

Замер агентом 2026-10-04 (`SK_GAMES_BENCH_REAL=1`): лаунчеры steam (17 игр), epic (2),
battlenet (1), ubisoft/ea/xbox (0); 99 находок, 183 заявленных пути, 0 issues; первый прогон
84,9 с, второй 84,8 с. Из них 84,2 с — 34 обхода include-проб (610 тыс. записей), в т.ч.
**6 обходов всего `C:\Users` по ≈ 99 тыс. записей, ≈ 13,8 с каждый** (≈ 83 с). Источник — 6 записей
манифеста неустановленных игр с текстом-заглушкой в квадратных скобках вместо имени пользователя:
`C:/Users/[User]/AppData/Roaming/Nitroplus` (2 игры), `C:/Users/[user]/AppData/Roaming/BlackBean/SBKX`
(и `/Saves`), `C:/Users/[User's Name]/AppData/LocalLow/Evil Tortilla Games/WhosYourDaddy/…` (2).
По §4.3 `[…]` — глоб-класс, статическая часть — `C:\Users`, якорь §4.6 — один сегмент `c:\users`
(существует всегда), проба ищет файл по `[User]/AppData/Roaming/Nitroplus/**` до глубины
`max_depth` и, не найдя, обходит весь `C:\Users`. Остальное: 2 обхода
`%LOCALAPPDATA%\Packages` (≈ 7 тыс. записей, ≈ 0,9 с каждый), `C:\Windows\System32` (0,25 с).
Число установленных игр на это время не влияет. Нужна задача (см. отчёт приёмки).

| Проверка | Результат | Примечание |
|---|---|---|
| Лаунчеры (`real:`) | | id и число игр |
| `real_machine`, второй прогон, мс | | ≤ 5000 |
| Самые медленные обходы (`slowest walks`) | | корни без личных путей |

## 6. Протокол (в описание PR / коммита закрытия SPEC-05)

| Проверка | Результат | Примечание |
|---|---|---|
| 2.1 `env`: источник манифеста, лаунчеры и число игр | | |
| 2.3 Статусы `both` / `ludusavi-only` / `sk-only` | N / M / K | |
| 2.5 Установленные игры с находками / всего с записями у Ludusavi | N из M | каждая без находки → задача или причина из §2.4 |
| 3 Игры `not-installed` | | название (или «создана копия Hollow Knight») |
| 4 Офлайн: `source`, `snapshot_date`, issue `manifest_offline`, находки игр | | UI — после SPEC-11 |
| 5 NFR-05-01 / 02 / 03 | | цифры из бенча; фон параллельно Environment — не выполнено (§9) |
| Windows (сборка), коммит | | |
