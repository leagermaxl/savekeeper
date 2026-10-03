# Чек-лист приёмки SPEC-04: правила §4.7.3–§4.7.9 на машине разработчика

| Поле | Значение |
|---|---|
| Основание | SPEC-04 §8, критерий 2: «На машине разработчика находки из правил покрывают все установленные программы из списка §4.7» |
| Объём | группы T-04-08 и T-04-09: `media-streaming`, `messengers`, `productivity`, `hardware-tuning`, `windows-shell`, `cloud-claims`, `games-extra`, `emulators` (56 правил). Группы T-04-07 (`browsers`, `dev`) проверены пользователем по `specs/checklists/T-04-07.md` |
| Когда выполнять | один раз перед закрытием SPEC-04; результат (отметки и цифры, без личных путей) — в описание PR / коммита закрытия |
| Где | реальный Windows-ПК разработчика, обычный пользователь (без «Запуск от имени администратора»), PowerShell 7 (`pwsh`) |

Принцип P1: SaveKeeper только читает. Во время проверки ничего не удалять, не перемещать и не
«чинить» в профиле, чтобы улучшить результат. Отчёт `acc04.json` содержит реальные пути — в
репозиторий его не коммитить.

## 0. Подготовка

```powershell
# В корне репозитория
cargo build --release -p sk-cli
# Папка не на диске C: (можно та же, что для T-04-07; рядом с exe появится savekeeper-data с логами)
New-Item -ItemType Directory E:\sk-manual -Force | Out-Null
Copy-Item .\target\release\savekeeper-cli.exe E:\sk-manual\ -Force
Set-Location E:\sk-manual
$repo = '<путь к репозиторию>'
```

Перед сканом запустить установленные программы, которые держат файлы открытыми (OBS, ShareX,
foobar2000, Telegram, Notepad++, Sublime Text, Outlook, qBittorrent, Logitech G HUB, Sticky
Notes): у их находок должен появиться тег `app-running`.

### 0.1 Встроенные файлы валидны

```powershell
.\savekeeper-cli.exe rules validate "$repo\rules"
"exit code: $LASTEXITCODE"
```

Ожидается: stdout пуст, в stderr `10 rule files checked, 0 errors, 0 warnings`, код `0`.

## 1. Ограничения текущей сборки (учитываются в ожиданиях)

| Ограничение | Затронутые правила | Ожидание сейчас |
|---|---|---|
| `Environment.launchers` в `scan` пуст до SPEC-05 T-05-05 (`enrich` в фазе Environment), поэтому `{STEAM}` и `{STEAM_USERID}` не раскрываются | `steam.userdata-config`, `steam.screenshots`, `wallpaper-engine.config` | находок нет даже при установленном Steam; правило проверено тестами `builtin_rules_games.rs`. Отметить «установлено», в «Ок» — «ждёт T-05-05» |
| `Environment.installed_programs` пуст до SPEC-06 T-06-03 | `retroarch.saves` (ветка `installed` условия) | находки есть, только если существует `%APPDATA%\RetroArch`; RetroArch только в `X:\RetroArch-Win64` без папки в Roaming — без находки, «ждёт T-06-03» |
| `claimed_paths` не выводятся в отчёт `scan` | правила `*.none` (только `claims`) | находок нет по построению; `claims` проверены тестами `builtin_rules.rs`. Проверяется только отсутствие находок |

Подтвердить первое ограничение (ожидается пустой список):

```powershell
(.\savekeeper-cli.exe env | ConvertFrom-Json).launchers
```

## 2. Скан и сводная таблица

### 2.1 Скан

```powershell
.\savekeeper-cli.exe scan --llm off --out acc04.json --pretty
"exit code: $LASTEXITCODE"          # 0 или 3 (3 — есть предупреждения, смотреть issues)
$r = Get-Content acc04.json -Raw | ConvertFrom-Json
```

### 2.2 «Установлено?» — признаки программ

Признак — наличие папки данных, файла или ключа, по которым срабатывает правило. Программа,
установленная, но ни разу не запущенная (папки данных нет), считается не установленной.

```powershell
$A = $env:APPDATA; $L = $env:LOCALAPPDATA
$D = [Environment]::GetFolderPath('MyDocuments'); $P = [Environment]::GetFolderPath('MyPictures')
$X86 = ${env:ProgramFiles(x86)}
$fixed = [IO.DriveInfo]::GetDrives() | Where-Object DriveType -eq 'Fixed' | ForEach-Object { $_.RootDirectory.FullName }
$steam = (Get-ItemProperty 'HKCU:\Software\Valve\Steam' -ErrorAction SilentlyContinue).SteamPath
function Has([string[]]$paths) { [bool]($paths | Where-Object { $_ -and (Test-Path $_) }) }
function Pkg($name) { [bool](Get-ChildItem "$L\Packages" -Directory -Filter "$($name)_*" -ErrorAction SilentlyContinue) }

$probe = [ordered]@{
  # media-streaming
  'obs.config'                   = { Has "$A\obs-studio" }
  'sharex.config'                = { Has "$D\ShareX" }
  'sharex.screenshots'           = { Has "$D\ShareX\Screenshots" }
  'vlc.config'                   = { Has "$A\vlc" }
  'mpc-hc.settings'              = { Has 'HKCU:\Software\MPC-HC' }
  'foobar2000.config'            = { Has "$A\foobar2000-v2", "$A\foobar2000" }
  'spotify.none'                 = { Has "$A\Spotify", "$L\Spotify" }
  # messengers
  'telegram.tdata'               = { Has "$A\Telegram Desktop\tdata" }
  'discord.settings'             = { Has "$A\discord\settings.json" }
  'slack.none'                   = { Has "$A\Slack" }
  'whatsapp.none'                = { Pkg '5319275A.WhatsAppDesktop' }
  'skype.none'                   = { (Has "$A\Skype", "$A\Microsoft\Skype for Desktop") -or (Pkg 'Microsoft.SkypeApp') }
  # productivity
  'obsidian.config'              = { Has "$A\obsidian\obsidian.json" }
  'obsidian.vaults'              = { Has "$A\obsidian\obsidian.json" }
  'keepass.config'               = { Has "$A\KeePass" }
  'keepassxc.config'             = { Has "$A\KeePassXC" }
  'notepadpp.config'             = { Has "$A\Notepad++" }
  'sublime.config'               = { Has "$A\Sublime Text\Packages\User", "$A\Sublime Text\Local\Session.sublime_session" }
  'office.templates'             = { Has "$A\Microsoft\Templates", "$A\Microsoft\UProof" }
  'outlook.pst'                  = { Has "$D\Outlook Files", "$L\Microsoft\Outlook\*.pst" }
  'autohotkey.scripts'           = { Has "$D\AutoHotkey" }
  'sevenzip.settings'            = { Has 'HKCU:\Software\7-Zip' }
  'total-commander.config'       = { Has "$A\GHISLER" }
  'everything.config'            = { Has "$A\Everything\Everything.ini" }
  'qbittorrent.config'           = { Has "$A\qBittorrent", "$L\qBittorrent\BT_backup" }
  'figma.settings'               = { Has "$A\Figma\settings.json" }
  'adobe.settings'               = { Has "$A\Adobe\Adobe Photoshop *\Adobe Photoshop * Settings", "$A\Adobe\Lightroom\Presets", "$A\Adobe\CameraRaw\Settings" }
  # hardware-tuning
  'msi-afterburner.profiles'     = { Has "$X86\MSI Afterburner\Profiles" }
  'rivatuner.profiles'           = { Has "$X86\RivaTuner Statistics Server\Profiles" }
  'rainmeter.skins'              = { Has "$D\Rainmeter\Skins", "$A\Rainmeter\Rainmeter.ini" }
  'wallpaper-engine.config'      = { $steam -and (Has "$steam\steamapps\common\wallpaper_engine\config.json") }
  'logitech-ghub.settings'       = { Has "$L\LGHUB\settings.db" }
  'razer-synapse.none'           = { Has "$L\Razer", "$env:ProgramData\Razer" }
  # windows-shell
  'windows.start-taskbar-pins'   = { Has "$A\Microsoft\Internet Explorer\Quick Launch\User Pinned" }
  'windows.sendto'               = { Has "$A\Microsoft\Windows\SendTo" }
  'windows.explorer-quickaccess' = { Has "$A\Microsoft\Windows\Recent\AutomaticDestinations\f01b4d95cf55d32a.automaticDestinations-ms" }
  'windows.sticky-notes'         = { Has "$L\Packages\Microsoft.MicrosoftStickyNotes_*\LocalState" }
  'windows.snipping-screenshots' = { Has "$P\Screenshots" }
  'windows.powertoys'            = { Has "$L\Microsoft\PowerToys" }
  # cloud-claims
  'onedrive.none'                = { Has "$L\Microsoft\OneDrive" }
  'dropbox.none'                 = { Has "$L\Dropbox", "$A\Dropbox" }
  'google-drive.none'            = { Has "$L\Google\DriveFS", "$L\Google\Drive" }
  'icloud.none'                  = { Pkg 'AppleInc.iCloud' }
  'yandex-disk.none'             = { Has "$A\Yandex\YandexDisk2", "$A\Yandex\YandexDisk" }
  # games-extra
  'steam.userdata-config'        = { $steam -and (Has "$steam\userdata\*\config") }
  'steam.screenshots'            = { $steam -and (Has "$steam\userdata\*\760\remote") }
  'minecraft.java'               = { Has "$A\.minecraft" }
  'prismlauncher.instances'      = { Has "$A\PrismLauncher\instances" }
  # emulators ({DRIVE:*} = all fixed drives)
  'retroarch.saves'              = { Has "$A\RetroArch" }   # see §1: drive folders need T-06-03 without it
  'dolphin.user'                 = { Has "$D\Dolphin Emulator", "$A\Dolphin Emulator" }
  'pcsx2.user'                   = { Has "$D\PCSX2" }
  'ppsspp.user'                  = { Has "$D\PPSSPP\PSP\SAVEDATA", "$D\PPSSPP\PSP\PPSSPP_STATE", "$D\PPSSPP\PSP\SYSTEM" }
  'yuzu-ryujinx.user'            = { Has "$A\yuzu\nand\user\save", "$A\yuzu\keys", "$A\Ryujinx\bis\user\save", "$A\Ryujinx\system" }
  'duckstation.user'             = { Has "$D\DuckStation", "$L\DuckStation" }
  'rpcs3.user'                   = { Has ($fixed | ForEach-Object { "$($_)*rpcs3*\dev_hdd0\home\*\savedata" }) }
  'cemu.user'                    = { Has "$A\Cemu\mlc01\usr\save" }
}
```

### 2.3 Сводка «установлено / находка / ок»

```powershell
$groups = 'media-streaming', 'messengers', 'productivity', 'hardware-tuning',
          'windows-shell', 'cloud-claims', 'games-extra', 'emulators'
$ids = Get-Content ($groups | ForEach-Object { "$repo\rules\$_.yaml" }) |
  Select-String '^\s*- id: (\S+)' | ForEach-Object { $_.Matches[0].Groups[1].Value }
"rules: $($ids.Count)"                                       # 56
$ids | Where-Object { -not $probe.Contains($_) }              # пусто: признак есть у каждого правила

$byRule = @{}
foreach ($f in $r.findings) {
  foreach ($rid in ($f.evidence | Where-Object { $_.source.kind -eq 'rule' }).source.rule_id) {
    $byRule[$rid] = @($byRule[$rid]) + $f | Where-Object { $_ }
  }
}
$waitSteam = 'steam.userdata-config', 'steam.screenshots', 'wallpaper-engine.config'
$summary = foreach ($id in $ids) {
  $inst = [bool](& $probe[$id])
  $n = if ($byRule.ContainsKey($id)) { @($byRule[$id]).Count } else { 0 }
  $expected, $ok = if ($id -like '*.none') { 'none (claims only)', ($n -eq 0) }
    elseif ($waitSteam -contains $id) { 'none until T-05-05', ($n -eq 0) }
    elseif ($inst) { 'finding', ($n -gt 0) }
    else { 'none', ($n -eq 0) }
  [pscustomobject]@{ rule = $id; installed = $inst; findings = $n; expected = $expected; ok = $ok }
}
$summary | Format-Table -AutoSize
"installed: {0}; with findings: {1}; ok: {2} of {3}" -f ($summary | Where-Object installed).Count,
  ($summary | Where-Object findings -gt 0).Count, ($summary | Where-Object ok).Count, $summary.Count
$summary | Where-Object { -not $_.ok }                         # разобрать каждую строку
```

Ожидается: `ok = True` во всех строках. Строка с `ok = False`:
- `installed = True`, `findings = 0` — пропуск правила (путь, include, условие) → задача на правило;
- `installed = False`, `findings > 0` — признак в §2.2 неточен или правило срабатывает на чужие
  файлы; открыть находку (§2.4) и решить;
- `*.none` с находками — ошибка правила.

### 2.4 Детали находок

```powershell
$rows = foreach ($id in $ids) {
  foreach ($f in (@($byRule[$id]) | Where-Object { $_ })) {
    [pscustomobject]@{
      rule   = $id
      title  = $f.title
      cat    = $f.category
      sens   = $f.sensitivity
      notes  = $f.notes_key
      tags   = $f.tags -join ','
      target = if ($f.target.resolved) { $f.target.resolved } else { $f.target.key }
      files  = $f.stats.file_count
      MB     = if ($f.stats) { [math]::Round($f.stats.total_bytes / 1MB, 2) }
      locked = $f.stats.locked_files
    }
  }
}
$rows | Sort-Object rule, target | Format-Table -AutoSize -Wrap
$r.issues | Where-Object source -eq 'rules' | Format-Table severity, message_key, path -AutoSize -Wrap
```

Ожидается: issues с `source = rules` нет (или каждое объяснимо: например,
`issue.rules.from_json_skipped` с `reason = missing` для vault'а Obsidian на отключённом диске).

## 3. Проверка по правилам

Для каждого правила заполнить: **Установлено?** (из §2.3, при сомнении — проверить вручную),
**Находка есть?** (`findings` > 0) и **Ок** (находки соответствуют колонке «Что проверить»;
для неустановленной программы — находок нет). Пропуск — задача на правило.

### 3.1 `media-streaming.yaml` (§4.7.3)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `obs.config` | `%APPDATA%\obs-studio`, в объёме `basic\**`, `global.ini`, `plugin_config\**`, без `logs`, `crashes`, `updates`; при запущенном OBS тег `app-running` | | | |
| `sharex.config` | `Documents\ShareX` без `Screenshots`, `Logs` | | | |
| `sharex.screenshots` | `Documents\ShareX\Screenshots`, `cat = user_files` | | | |
| `vlc.config` | `%APPDATA%\vlc` без `art` | | | |
| `mpc-hc.settings` | ветка `HKCU\Software\MPC-HC` (если MPC-HC хранит настройки в реестре, а не в `mpc-hc64.ini`) | | | |
| `foobar2000.config` | `%APPDATA%\foobar2000-v2` и/или `%APPDATA%\foobar2000` (метки v2 / v1) | | | |
| `spotify.none` | находок нет | | — | |

### 3.2 `messengers.yaml` (§4.7.4)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `telegram.tdata` | `%APPDATA%\Telegram Desktop\tdata`, `sens = high`, `notes = rules.telegram.notes`; объём без `user_data`, `user_data#*`, `emoji`, `dumps` (заметно меньше всей `tdata`) | | | |
| `discord.settings` | один файл `%APPDATA%\discord\settings.json`, `cat = app_config` | | | |
| `slack.none`, `whatsapp.none`, `skype.none` | находок нет | | — | |

### 3.3 `productivity.yaml` (§4.7.5)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `obsidian.config` | файл `%APPDATA%\obsidian\obsidian.json` | | | |
| `obsidian.vaults` | по находке на каждый vault из `obsidian.json` (`cat = user_files`, тег `notes`), без `.trash`; vault на отсутствующем диске — `issue.rules.from_json_skipped` `missing` | | | |
| `keepass.config`, `keepassxc.config` | `%APPDATA%\KeePass` / `%APPDATA%\KeePassXC`; сами `.kdbx` здесь не ищутся (SPEC-07) | | | |
| `notepadpp.config` | `%APPDATA%\Notepad++`: `*.xml`, `userDefineLangs`, `themes`, `plugins\config`, `backup` (несохранённые вкладки) | | | |
| `sublime.config` | `Packages\User` и `Local\Session.sublime_session` (две метки) | | | |
| `office.templates` | `%APPDATA%\Microsoft\Templates` и/или `UProof` (пользовательский словарь) | | | |
| `outlook.pst` | `Documents\Outlook Files` и `.pst` в `%LOCALAPPDATA%\Microsoft\Outlook` (по находке на файл), тег `large`; `.ost` не находка | | | |
| `autohotkey.scripts` | `Documents\AutoHotkey`, `cat = user_files` | | | |
| `sevenzip.settings` | ветка `HKCU\Software\7-Zip` | | | |
| `total-commander.config` | `%APPDATA%\GHISLER` | | | |
| `everything.config` | файл `%APPDATA%\Everything\Everything.ini` (только у установки с настройками в профиле) | | | |
| `qbittorrent.config` | `%APPDATA%\qBittorrent` и `%LOCALAPPDATA%\qBittorrent\BT_backup` (активные торренты), `cat = app_data` | | | |
| `figma.settings` | файл `%APPDATA%\Figma\settings.json` | | | |
| `adobe.settings` | по находке на каждую версию Photoshop (`Adobe Photoshop <версия> Settings`), пресеты Lightroom, настройки Camera Raw; без `Media Cache` | | | |

### 3.4 `hardware-tuning.yaml` (§4.7.6)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `msi-afterburner.profiles` | `Program Files (x86)\MSI Afterburner\Profiles` (явный корень внутри исключения), `files` > 0 | | | |
| `rivatuner.profiles` | `Program Files (x86)\RivaTuner Statistics Server\Profiles`, `files` > 0 | | | |
| `rainmeter.skins` | `Documents\Rainmeter\Skins` и `%APPDATA%\Rainmeter\Rainmeter.ini` | | | |
| `wallpaper-engine.config` | см. §1: находок нет до T-05-05; отметить, установлен ли Wallpaper Engine | | — | |
| `logitech-ghub.settings` | файл `%LOCALAPPDATA%\LGHUB\settings.db`; при работающем G HUB тег `app-running`, `locked` может быть 1 | | | |
| `razer-synapse.none` | находок нет | | — | |

### 3.5 `windows-shell.yaml` (§4.7.7)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `windows.start-taskbar-pins` | `…\Quick Launch\User Pinned`, `cat = system_settings` | | | |
| `windows.sendto` | `%APPDATA%\Microsoft\Windows\SendTo` | | | |
| `windows.explorer-quickaccess` | файл `f01b4d95cf55d32a.automaticDestinations-ms` | | | |
| `windows.sticky-notes` | `…\Packages\Microsoft.MicrosoftStickyNotes_8wekyb3d8bbwe\LocalState`, в объёме `plum.sqlite*` (с `-wal`, `-shm`), `cat = app_data` | | | |
| `windows.snipping-screenshots` | `Pictures\Screenshots`, `cat = user_files` | | | |
| `windows.powertoys` | `%LOCALAPPDATA%\Microsoft\PowerToys`, только `*.json`, без `Logs` и `Updates` | | | |

### 3.6 `cloud-claims.yaml` (§4.7.8)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `onedrive.none`, `dropbox.none`, `google-drive.none`, `icloud.none`, `yandex-disk.none` | находок нет; синхронизируемые папки клиентов не стали находками правил | | — | |

### 3.7 `games-extra.yaml` (§4.7.9)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `steam.userdata-config`, `steam.screenshots` | см. §1: находок нет до T-05-05; отметить, установлен ли Steam и сколько аккаунтов в `userdata` | | — | |
| `minecraft.java` | `%APPDATA%\.minecraft`, в объёме `saves`, `options.txt`, `servers.dat`, `resourcepacks`, `shaderpacks`, `screenshots`, `mods`; без `versions`, `libraries`, `assets` | | | |
| `prismlauncher.instances` | `%APPDATA%\PrismLauncher\instances` | | | |

### 3.8 `emulators.yaml` (§4.7.9)

| Правило | Что проверить | Установлено? | Находка есть? | Ок |
|---|---|---|---|---|
| `retroarch.saves` | `%APPDATA%\RetroArch` и папки `X:\RetroArch`, `X:\RetroArch-Win64` на фиксированных дисках (по находке на папку): `saves`, `states`, `config`, `retroarch.cfg`, без `cores`, `system`; `notes_key` про BIOS. Без папки в Roaming — см. §1 | | | |
| `dolphin.user` | `Documents\Dolphin Emulator` или `%APPDATA%\Dolphin Emulator`: `GC`, `Wii`, `StateSaves`, `Config` | | | |
| `pcsx2.user` | `Documents\PCSX2`: `memcards`, `sstates`, `inis` | | | |
| `ppsspp.user` | `SAVEDATA`, `PPSSPP_STATE`, `SYSTEM` (`cat = game_config`, без `CACHE`) | | | |
| `yuzu-ryujinx.user` | сохранения yuzu/Ryujinx (`game_save`) и ключи (`credentials`, `sens = high`, только `*.keys`), `notes_key` про дамп ключей | | | |
| `duckstation.user` | `Documents\DuckStation` или `%LOCALAPPDATA%\DuckStation`: `memcards`, `savestates`, `settings.ini` | | | |
| `rpcs3.user` | `X:\<папка с rpcs3 в имени>\dev_hdd0\home\<пользователь>\savedata`, по находке на пользователя эмулятора | | | |
| `cemu.user` | `%APPDATA%\Cemu\mlc01\usr\save` | | | |

## 4. Протокол (в описание PR / коммита закрытия SPEC-04)

| Проверка | Результат | Примечание |
|---|---|---|
| 0.1 `rules validate` | | |
| `launchers` пуст (§1) | да / нет | если не пуст — Steam-правила ожидают находки |
| 2.3 Установлено / с находками / `ok` | N / M / K из 56 | |
| 2.3 Строки `ok = False` | | каждая → задача или объяснение |
| 2.4 Issues `source = rules` | | |
| Ждут T-05-05 / T-06-03 (установлено, находок нет по §1) | | перечислить правила |
| Windows (сборка), коммит | | |

Критерий 2 SPEC-04 §8 выполнен, если у всех установленных программ из правил §4.7.3–§4.7.9
есть находки, кроме правил, ожидающих T-05-05 / T-06-03 по §1 (решение о закрытии критерия
с этой оговоркой принимает пользователь).
