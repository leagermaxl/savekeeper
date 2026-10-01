# SPEC-09: Слияние, оценка важности и выбор по умолчанию

| Поле | Значение |
|---|---|
| ID | SPEC-09 |
| Статус | approved |
| Фаза | P2 |
| Крейт(ы) | `sk-score` |
| Зависит от | SPEC-01, SPEC-02, SPEC-04, SPEC-05, SPEC-06, SPEC-07, SPEC-08 |
| Используется в | SPEC-10, SPEC-11 |
| Последнее изменение | 2026-10-01 (`ScoringConfig` — в `sk-core::config`) |

## 1. Цель

Превратить «сырые» находки всех коллекторов в итоговый список для пользователя:
1. **Слить** дубли и вложенные находки, разрешив конфликты категорий.
2. **Оценить** важность каждой находки числом 0..1 с объяснимыми компонентами.
3. **Выбрать по умолчанию** рекомендуемое к сохранению.
4. **Посчитать итоги** (`Totals`) для UI и отчёта.

Функция детерминирована и не делает ввода-вывода: одинаковые входы и одинаковое `now` дают
одинаковый результат.

## 2. Область

### 2.1 Входит
- Слияние по `FindingId` и по вложенности путей.
- Приоритет источников и разрешение конфликтов категорий.
- Формула score и её компоненты.
- Правила `default_selected`.
- Расчёт `Totals`.
- Конфигурация весов и порогов.

### 2.2 Не входит
- Ручной выбор пользователя в UI (SPEC-11): хранится отдельно как `Selection` поверх `default_selected`.
- Дедупликация файлов при записи бэкапа (SPEC-10). SPEC-09 минимизирует пересечения, но SPEC-10 всё равно обязан не копировать файл дважды.
- Обучение на решениях пользователя (backlog, §9).

## 3. Требования

### 3.1 Функциональные
- **FR-09-01** — Находки с одинаковым `FindingId` сливаются в одну: evidence объединяются, теги объединяются, категория выбирается по §4.3.
- **FR-09-02** — Находка-`FileSet`, полностью лежащая внутри другой (§4.4), становится её дочерней: id попадает в `parent.children`, у дочерней ставится тег `nested`. Дочерняя остаётся в списке (UI показывает дерево), но не учитывается в `Totals` дважды и не выбирается независимо.
- **FR-09-03** — Если родитель — `Cache`/`Reinstallable`/`Unknown`, а вложенная находка важнее, поглощения нет. Вместо этого родитель получает `exclude` на путь дочерней находки, чтобы при выборе обеих файлы не копировались дважды.
- **FR-09-04** — У каждой находки после фазы `Score` заполнены `score` (с компонентами §4.5) и `default_selected`.
- **FR-09-05** — `Cache` и `Reinstallable` никогда не выбираются по умолчанию.
- **FR-09-06** — Находка больше `scoring.max_default_item_bytes` (по умолчанию 2 ГиБ) не выбирается по умолчанию и получает тег `too_large_default`.
- **FR-09-07** — `Credentials` и находки с `Sensitivity::High` выбираются по умолчанию (если проходят порог), но порождают `ScanIssue::Warning scoring.sensitive_selected` и рекомендацию шифрования (SPEC-10).
- **FR-09-08** — Каждая причина невыбора отражается тегом (`too_large_default`, `needs_elevation`, `below_threshold`, `nested`, `restorable`) для UI-фильтров.
- **FR-09-09** — `Totals` считаются по «верхнеуровневым» находкам (без `nested`).

### 3.2 Нефункциональные
- **NFR-09-01** — 10 000 находок обрабатываются ≤ 100 мс (release).
- **NFR-09-02** — Детерминизм: результат не зависит от порядка входных находок. Все значения округлены до 3 знаков после запятой.
- **NFR-09-03** — Объяснимость: `Score.components` содержит все слагаемые формулы, и UI может показать «почему такая оценка».

## 4. Дизайн

### 4.1 Публичный API

```rust
// sk-core::config (SPEC-01 §4.8.2), реэкспорт из sk-score
pub struct ScoringConfig {
    pub category_weights: BTreeMap<Category, CategoryWeights>,   // §4.6, дефолты из таблицы
    pub select_threshold: f32,                                   // 0.40
    pub unknown_select_threshold: f32,                           // 0.30
    pub unknown_max_default_bytes: u64,                          // 200 МиБ
    pub max_default_item_bytes: u64,                             // 2 ГиБ (SPEC-01 §4.8.2)
    pub size_free_bytes: u64,                                    // 100 МиБ — до этого штрафа нет
    pub size_penalty_cap: f32,                                   // 0.40
    pub cloud_synced_penalty: f32,                               // 0.15
    pub installed_bonus: f32,                                    // 0.05
    pub uninstalled_penalty: f32,                                // 0.10
}

pub struct ScoreInput<'a> {
    pub findings: Vec<Finding>,
    pub env: &'a Environment,
    pub now: OffsetDateTime,                                     // передаётся извне → детерминизм
}

pub struct ScoreOutput {
    pub findings: Vec<Finding>,      // слиты, оценены, отсортированы (§4.8)
    pub totals: Totals,
    pub issues: Vec<ScanIssue>,
}

pub fn run(input: ScoreInput, cfg: &ScoringConfig) -> ScoreOutput;

// Отдельные шаги публичны для тестов и для пересчёта в UI:
pub fn merge_by_id(findings: Vec<Finding>) -> Vec<Finding>;
pub fn merge_nested(findings: &mut Vec<Finding>, env: &Environment);
pub fn score(f: &Finding, env: &Environment, now: OffsetDateTime, cfg: &ScoringConfig) -> Score;
pub fn default_selection(f: &Finding, env: &Environment, cfg: &ScoringConfig) -> SelectionDecision;
pub fn totals(findings: &[Finding], selected: impl Fn(&Finding) -> bool) -> Totals; // UI пересчитывает при ручном выборе

pub struct SelectionDecision { pub selected: bool, pub reason_tag: Option<&'static str> }
```

### 4.2 Приоритет источников

| Источник (`EvidenceSource`) | Приоритет |
|---|---|
| `User` | 6 |
| `Rule` | 5 |
| `Ludusavi`, `Launcher` | 4 |
| `System` | 3 |
| `Heuristic` | 2 |
| `Llm` | 1 |

`authority(f) = max over evidence of (priority, confidence)` — лексикографическое сравнение
кортежа. Это «самое авторитетное доказательство» находки.

### 4.3 Слияние по FindingId

Для группы находок с одинаковым `id`:
1. **Победитель** `W` — находка с максимальным `authority`. При равенстве побеждает находка с меньшим `Category` в порядке §4.8, затем с лексикографически меньшим `title`.
2. `category`, `app`, `title`, `target` берутся от `W`. Исключение: если `W.app` = None, берётся первый не-None `app` по убыванию authority.
3. `evidence` — объединение, дедупликация по `(source, message_key)` (остаётся с большей confidence), сортировка по убыванию authority.
4. `tags` — объединение (отсортировано, без дублей).
5. `sensitivity` = max. `requires_elevation` = OR.
6. `stats` = `W.stats`, иначе первый не-None.
7. Для `FileSet`: `include` — объединение, если у всех оно непустое; если хоть у одного пусто (всё), результат пуст. `exclude` — пересечение (исключаем только то, что исключили все).
   Примечание: `FindingId` включает include/exclude (SPEC-02 §2.7), поэтому совпадение id с разными глобами на практике невозможно. Правило нужно для защиты от коллизий.
8. **Конфликт категорий** логируется как `Evidence` без изменения победителя. Если проигравшая категория входит в «опасную пару» (победитель `Cache`/`Reinstallable`, проигравший `GameSave`/`UserFiles`/`Credentials`/`DevEnvironment` с confidence ≥ 0.7), выдаётся `ScanIssue::Warning scoring.category_conflict` и находке добавляется тег `conflict` (UI подсвечивает). Лучше лишний раз показать, чем потерять данные.

### 4.4 Слияние по вложенности

Только для `Target::FileSet` и `Target::File`.

```
sort by (resolved path components lowercase, depth)
for each finding B (по возрастанию глубины):
  A = ближайший предок-FileSet, такой что:
      starts_with_ci(B.resolved, A.resolved)                  // покомпонентно, SPEC-02 §5
      and A.include покрывает rel(B)                          // пустой include или glob match rel-пути (или любого его префикса для каталога)
      and not A.exclude покрывает rel(B)
  if A is None: continue
  if A.category in {Cache, Reinstallable, Unknown} and B.category not in {Cache, Reinstallable, Unknown}:
      A.target.exclude.push(rel(B) + "/**" | rel(B))          // FR-09-03; A.id не пересчитывается
      continue
  // поглощение
  A.children.push(B.id); B.tags.push("nested")
  A.evidence.extend(B.evidence ∩ ранее отсутствующие источники) с пометкой message_args["via"] = B.title
  A.sensitivity = max(A.sensitivity, B.sensitivity)
```

- `FindingId` вычисляется коллекторами **до** слияния и далее не меняется. Добавленные при слиянии `exclude` не влияют на id.
- Вложенность `Registry`/`SystemExport` не анализируется.
- Если B вложена в несколько кандидатов, берётся ближайший (самый глубокий) A.

### 4.5 Формула score

Обозначения (все в `[0, 1]`):

| Компонент | Ключ в `components` | Определение |
|---|---|---|
| I | `irreplaceability` | `category_weights[cat].irreplaceability` (§4.6) |
| A | `user_authored` | Первое применимое: `tags ∋ git-unsaved ∨ git-no-remote` → 1.0; есть `Evidence::Llm` и категория получена от LLM → `message_args["importance"]` (SPEC-08); иначе `category_weights[cat].authored` |
| R | `recency` | `d` = дней с `stats.newest_mtime` до `now`. `d ≤ 30` → 1.0; `d > 30` → `0.3 + 0.7·e^{−(d−30)/365}`. Нет данных → 0.5 |
| C | `confidence` | confidence самого авторитетного evidence (§4.2) |
| S | `size_penalty` | `b = stats.total_bytes`; `b ≤ size_free_bytes` → 0; иначе `min(cap, 0.1·log2(b / size_free_bytes))`; для категорий с `size_tolerant = true` умножается на 0.5. Нет stats → 0 |
| Adj | `adjustments` | `+installed_bonus`, если `app.installed == Some(true)`; `−uninstalled_penalty`, если `Some(false)` и категория ∈ {AppConfig, AppData, GameConfig}; `−cloud_synced_penalty`, если тег `cloud-synced` |

```
C_eff  = 0.5 + 0.5·C
base   = 0.55·I + 0.25·A + 0.20·R
value  = clamp01( C_eff · base − S + Adj )
```

`Score.components` содержит `irreplaceability, user_authored, recency, confidence, size_penalty,
adjustments, base, value`. Все значения округляются до 3 знаков (`(x·1000).round()/1000`).
Округление выполняется **после** вычисления `value`, чтобы не накапливать ошибку.

Для `Cache`/`Reinstallable` score тоже считается (для сортировки и UI), но на выбор не влияет (§4.7).

### 4.6 Таблица весов по категориям

| Категория | irreplaceability (I) | authored (A по умолч.) | size_tolerant |
|---|---|---|---|
| Credentials | 1.00 | 1.00 | нет |
| GameSave | 0.95 | 0.90 | нет |
| UserFiles | 0.85 | 0.90 | **да** |
| DevEnvironment | 0.80 | 0.60 | нет |
| AppData | 0.70 | 0.50 | нет |
| AppConfig | 0.65 | 0.50 | нет |
| SystemSettings | 0.60 | 0.40 | нет |
| GameConfig | 0.55 | 0.40 | нет |
| Unknown | 0.35 | 0.30 | нет |
| Reinstallable | 0.05 | 0.00 | да |
| Cache | 0.00 | 0.00 | да |

Веса переопределяются в `savekeeper.config.json` → `scoring.category_weights` (частично, недостающие берутся из дефолтов).

### 4.7 Правила default_selected

Проверяются по порядку, первое сработавшее правило решает:

| # | Условие | selected | reason_tag |
|---|---|---|---|
| 1 | тег `nested` | false | `nested` (выбор следует за родителем в UI) |
| 2 | категория `Cache` или `Reinstallable` | false | `restorable` |
| 3 | `requires_elevation && !env.is_elevated` | false | `needs_elevation` |
| 4 | `stats.total_bytes > max_default_item_bytes` | false | `too_large_default` |
| 5 | категория `Unknown` и (`value < unknown_select_threshold` или `bytes > unknown_max_default_bytes`) | false | `below_threshold` |
| 6 | категория `Unknown` | true | — |
| 7 | `value < select_threshold` | false | `below_threshold` |
| 8 | иначе | true | — |

После выбора: если `selected && (category == Credentials || sensitivity == High)`, выдаётся
issue `scoring.sensitive_selected` (одно на скан, с количеством в `message_args["count"]`).
`reason_tag` добавляется в `tags` находки.

Порог для `Unknown` ниже общего осознанно, по принципу «лучше лишнее, чем пропущенное».
При этом действует лимит размера, чтобы неизвестный многогигабайтный кэш не попадал в бэкап.

### 4.8 Сортировка результата

1. Порядок категорий: Credentials, GameSave, UserFiles, DevEnvironment, AppData, AppConfig, GameConfig, SystemSettings, Unknown, Reinstallable, Cache.
2. `score.value` по убыванию.
3. `title` (регистронезависимо), затем `id`.

Дочерние (`nested`) находки сортируются так же, UI группирует их под родителем через `children`.

### 4.9 Totals

Предлагаемая структура (уточняет `Totals` из SPEC-02 §6, см. §9):
```rust
pub struct Totals {
    pub by_category: BTreeMap<Category, CategoryTotals>,
    pub all: CategoryTotals,
    pub sensitive_selected: u32,
    pub unknown_count: u32,
    pub needs_elevation_count: u32,
    pub too_large_count: u32,
}
pub struct CategoryTotals { pub count: u32, pub bytes: u64, pub selected_count: u32, pub selected_bytes: u64 }
```
- Учитываются только находки без тега `nested`.
- `bytes` = `stats.total_bytes` (0, если stats нет, например у `SystemExport` до выполнения).
- Функция `totals()` принимает предикат выбора, поэтому UI использует её же для пересчёта при ручном выборе (через IPC, SPEC-11).

### 4.10 Конфигурация

Секция `scoring` (SPEC-01 §4.8.2) расширяется полями `ScoringConfig` (§4.1). Дефолты указаны
в §4.1 и §4.6. Невалидные значения (вне `[0, 1]` для весов, отрицательные байты) заменяются
дефолтами с issue `scoring.bad_config`.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| Находка без evidence (нарушение P3) | `debug_assert!`. В release добавляется синтетический `Evidence { Heuristic { "scoring-fallback" }, confidence: 0.3 }` + issue `scoring.no_evidence`. |
| `stats` = None у FileSet (Measure не отработал, отмена) | R = 0.5, S = 0. Правило 4 не срабатывает. Тег `unmeasured`. |
| `newest_mtime` в будущем (сбитые часы) | `d = 0` → R = 1.0. |
| NaN/∞ в confidence или importance | Трактуются как 0, issue `scoring.bad_number`. |
| Цикл вложенности (одинаковые пути у разных id — File и FileSet на один путь) | Более узкий тип (`File`) считается дочерним, остальное детерминированно по id. |
| Родитель выбран, дочерняя с `Sensitivity::High` | Sensitivity поднимается у родителя (§4.4), значит, срабатывает предупреждение. |
| Пути на разных дисках, регистр букв | Сравнение покомпонентно и регистронезависимо (SPEC-02 §5). |
| Пустой вход | Пустой выход, нулевые Totals. |

## 6. Тестирование

### 6.1 Таблица примеров (обязательный параметризованный тест)

`now = 2026-09-28`, `env.is_elevated = false`, дефолтный конфиг. Проверяется `value` ±0.02 и `selected`.

| # | Находка | Вход | Ожидаемый value | selected / tag |
|---|---|---|---|---|
| 1 | Elden Ring, сохранения | GameSave, Ludusavi C=1.0, 50 МБ, mtime −3 д, installed | ≈ 1.00 (0.9475 + 0.05 → clamp) | true |
| 2 | VS Code, настройки | AppConfig, Rule C=0.95, 2 МБ, −10 д, installed | ≈ 0.72 | true |
| 3 | Конфиг удалённой программы | AppConfig, Heuristic C=0.6, 1 МБ, −1095 д, installed=false | ≈ 0.34 | false / `below_threshold` |
| 4 | node_modules | Cache, Heuristic C=0.95, 800 МБ, −1 д | ≈ 0.05 | false / `restorable` |
| 5 | Кластер фото RAW | UserFiles, Heuristic C=0.7, 40 ГБ, −60 д | ≈ 0.55 | false / `too_large_default` |
| 6 | Репозиторий с незапушенными коммитами | DevEnvironment, Heuristic C=0.95, tag `git-unsaved`, 300 МБ, −1 д | ≈ 0.71 | true |
| 7 | Неизвестная папка | Unknown, Llm C=0.3, importance 0.3, 80 МБ, −20 д | ≈ 0.30 (0.304) | true (порог Unknown 0.30; граничный кейс — точное значение без допуска) |
| 8 | База KeePass | Credentials, Heuristic C=1.0, 2 МБ, −100 д | ≈ 0.98 | true + issue `sensitive_selected` |
| 9 | Профили Wi-Fi | SystemSettings, System C=1.0, stats=None, sensitivity High | ≈ 0.53 | true + issue |
| 10 | Documents в OneDrive | UserFiles, Rule C=1.0, 5 ГБ, −1 д, `cloud-synced` | ≈ 0.54 | false / `too_large_default` |
| 11 | Чистый git-репозиторий | Reinstallable, Heuristic C=0.85, tag `git-clean` | ≈ 0.12 | false / `restorable` |
| 12 | Экспорт драйверов | SystemSettings, System C=1.0, `requires_elevation` | ≈ 0.53 | false / `needs_elevation` |

Если реализация даёт расхождение больше допуска, сначала проверяется формула. Если ошибка
в таблице, исправляется спека, а не допуск.

### 6.2 Слияние
- Две находки с одним id (Rule AppConfig C=0.9 + Llm Cache C=0.8): победитель Rule, evidence = 2, нет `conflict` (Cache-проигравший не опасен).
- Rule `Reinstallable` + Heuristic `GameSave` C=0.8 с одним id → тег `conflict` + issue.
- Вложенность: `{APPDATA}\Foo` (AppData) ⊃ `{APPDATA}\Foo\settings.json` (AppConfig) → дочерняя, тег `nested`, Totals не удваиваются.
- Вложенность в мусор: `{LOCALAPPDATA}\Bar` (Reinstallable) ⊃ `{LOCALAPPDATA}\Bar\Saves` (GameSave) → поглощения нет, у родителя `exclude` содержит `Saves/**`.
- `include` родителя не покрывает ребёнка (`include: ["*.ini"]`, ребёнок `Saves\`) → не вложенность.
- `Unknown`-родитель с важным ребёнком → категория родителя не меняется, родитель получает `exclude` (FR-09-03), ребёнок остаётся самостоятельным. Тест фиксирует поведение (см. §9, вопрос 1).

### 6.3 Свойства (proptest)
- Перестановка входа не меняет выход (NFR-09-02).
- `0 ≤ value ≤ 1` для любых входов.
- Сумма `selected_bytes` по категориям = `all.selected_bytes`.
- Никакая находка `Cache`/`Reinstallable` не выбрана.

### 6.4 Производительность
- `criterion`: 10 000 синтетических находок с глубиной вложенности до 8 → ≤ 100 мс (NFR-09-01).

## 7. Задачи

- [ ] **T-09-01** — Каркас `sk-score`: реэкспорт `ScoringConfig` (тип и дефолты §4.1/§4.6 реализуются в SPEC-01 T-01-04), валидация значений из конфига. *Зависит:* T-01-04, T-02-01. *Готово, когда:* unit-тест дефолтов и частичного переопределения.
- [ ] **T-09-02** — `authority()` и приоритет источников §4.2. *Зависит:* T-09-01.
- [ ] **T-09-03** — `merge_by_id` §4.3 (включая конфликт категорий и issue). *Зависит:* T-09-02. *Готово, когда:* тесты §6.2 (id-часть).
- [ ] **T-09-04** — `merge_nested` §4.4 на `PathSet`/сортировке, с glob-проверкой include/exclude. *Зависит:* T-09-03, T-02-02. *Готово, когда:* тесты §6.2 (вложенность).
- [ ] **T-09-05** — `score()` §4.5 с компонентами и округлением. *Зависит:* T-09-01. *Готово, когда:* таблица §6.1 (value) зелёная.
- [ ] **T-09-06** — `default_selection()` §4.7 + теги причин + issue `sensitive_selected`. *Зависит:* T-09-05. *Готово, когда:* таблица §6.1 (selected) зелёная.
- [ ] **T-09-07** — `totals()` §4.9 и сортировка §4.8. *Зависит:* T-09-06.
- [ ] **T-09-08** — `run()` + встраивание в `sk-engine` (фаза `Score`), `now` берётся из engine. *Зависит:* T-09-07, T-01-06.
- [ ] **T-09-09** — proptest-свойства §6.3 и бенчмарк §6.4. *Зависит:* T-09-08.
- [ ] **T-09-10** — Ключи i18n для компонентов score и reason-тегов (UI «почему такая оценка»). *Зависит:* T-09-06.

## 8. Критерии приёмки

- [ ] Таблица §6.1 проходит полностью.
- [ ] Свойства §6.3 проходят на 10 000 случайных входов.
- [ ] На эталонной машине: ни одного выбранного по умолчанию `Cache`/`Reinstallable`. Все сохранения игр из ручного списка выбраны. Суммарный размер выбранного по умолчанию ≤ 20 ГБ (без пользовательских медиа, которые помечены `too_large_default`).
- [ ] В UI для любой находки можно показать все компоненты score (проверяется совместно со SPEC-11).

## 9. Открытые вопросы

1. **Unknown-родитель с важным ребёнком:** сейчас (FR-09-03) родитель получает `exclude`, и ребёнок живёт отдельно. Альтернатива — уточнять категорию родителя до категории ребёнка. Решение v1: exclude (консервативно). Уточнение категории — кандидат на пересмотр после eval на реальных машинах.
2. **Предложение к SPEC-02 §6:** зафиксировать структуру `Totals` из §4.9.
3. **Предложение к SPEC-02 §2.1:** хранить ли добавленные при слиянии `exclude` отдельно (`merged_excludes`), чтобы явно отличать их от исходного target? Сейчас они дописываются в `target.exclude`, а `id` не пересчитывается.
4. **Предложение к SPEC-08/SPEC-02:** `importance` от LLM передаётся через `Evidence.message_args["importance"]` (строка). Отдельное типизированное поле было бы чище.
5. **Бюджет выбора:** нужен ли общий лимит «выбрано по умолчанию ≤ N ГБ» или «≤ свободного места на цели»? Решается в SPEC-10/SPEC-11 (цель бэкапа известна только там). SPEC-09 даёт `totals()` для проверки.
6. **Обучение на выборе пользователя (future):** персональные поправки весов категорий и эмбеддинги похожих путей. Отдельное расширение после MVP.
