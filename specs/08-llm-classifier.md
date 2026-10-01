# SPEC-08: LLM-классификатор

| Поле | Значение |
|---|---|
| ID | SPEC-08 |
| Статус | approved |
| Фаза | P2 |
| Крейт(ы) | `sk-llm` |
| Зависит от | SPEC-01, SPEC-02, SPEC-07 |
| Используется в | SPEC-09, SPEC-11 |
| Последнее изменение | 2026-10-01 (`LlmConfig` — в `sk-core::config`; `MockClassifier` в `sk-llm` под feature `mock`) |

## 1. Цель

Классифицировать папки, которые не распознали правила и эвристики (`Category::Unknown`
+ `FolderSummary` из SPEC-07). Модель определяет, что это за приложение, какая это категория
данных, насколько данные важны и почему, и предлагает `include`/`exclude`. LLM является
надстройкой (принцип P4): без неё программа полностью работает. Приватность обязательна (P2):
наружу уходят только обезличенные метаданные, облако выключено по умолчанию.

## 2. Область

### 2.1 Входит
- Трейт `Classifier` и три провайдера: Ollama, OpenAI-совместимый (llama.cpp server, LM Studio, vLLM), Anthropic.
- Системный промпт, JSON-схема ответа, версионирование промпта.
- Батчинг, лимиты, таймауты, ретраи, параллелизм, отмена.
- Кэш результатов (JSONL).
- Приватность: redact, превью отправляемого, `allow_content_peek`.
- Хранение API-ключа.
- Health-check и авто-детект локальных провайдеров, рекомендуемые модели.
- Валидация и нормализация ответа.
- Eval-набор и метрика качества.

### 2.2 Не входит
- Вычисление `FolderSummary` (SPEC-03) и отбор кандидатов (SPEC-07).
- Итоговый score (SPEC-09). LLM даёт только `importance` и `confidence` как вход.
- Агентный режим и эмбеддинги (см. §9, backlog SPEC-00 §9).
- Загрузка и запуск локальных моделей самой программой (пользователь ставит Ollama/LM Studio сам, UI даёт инструкцию).

## 3. Требования

### 3.1 Функциональные
- **FR-08-01** — `llm.mode = off | local | cloud` (SPEC-01 §4.8.2), по умолчанию `off`. В режиме `off` крейт не делает ни одного сетевого запроса.
- **FR-08-02** — На вход подаются только `FolderSummary` после `sk-core::privacy::redact` (SPEC-02 §4.2). Абсолютные пути, имя пользователя и имя машины не отправляются никогда.
- **FR-08-03** — Ответ — строго JSON по схеме §4.5. Невалидный ответ → ретрай (1 раз) → иначе элемент остаётся `Unknown` с issue.
- **FR-08-04** — Кэш: повторная классификация неизменившейся сводки той же моделью и версией промпта не делает запроса.
- **FR-08-05** — Перед первым облачным запросом за скан UI показывает превью отправляемых данных (§4.8) и требует подтверждения. Галочка «больше не спрашивать» хранится в конфиге.
- **FR-08-06** — `max_items_per_scan` (по умолчанию 300) ограничивает число элементов на скан. Приоритет отбора: больший `total_bytes × recency`, но не более 50 ГБ на элемент (огромное почти всегда кэш или игры).
- **FR-08-07** — Результат превращается в `Evidence { source: Llm { provider, model }, message_key: "evidence.llm", message_args: { reason, app_guess } , confidence }` и обновляет `category`, `app`, `include`/`exclude` находки.
- **FR-08-08** — Health-check провайдера доступен из UI («Проверить подключение»): доступность, наличие модели, время ответа.
- **FR-08-09** — Авто-детект: при `mode = local` без настроек проверяются `http://127.0.0.1:11434` (Ollama) и `http://127.0.0.1:1234` (LM Studio), `http://127.0.0.1:8080` (llama.cpp server).
- **FR-08-10** — Отмена прерывает текущие HTTP-запросы (drop future) за ≤ 1 с. Уже полученные результаты сохраняются в кэш.

### 3.2 Нефункциональные
- **NFR-08-01** — Local (7–8B, GPU): ≤ 3 с на батч из 8 элементов. Cloud (Haiku 4.5): ≤ 5 с на батч из 20.
- **NFR-08-02** — Accuracy по категории на eval-наборе (§6.3): ≥ 0.80 для `claude-haiku-4-5`, ≥ 0.70 для `qwen2.5:7b-instruct`.
- **NFR-08-03** — Стоимость облачного скана из 300 элементов на Haiku 4.5 ≤ $0.05 (≈ 300 × 600 входных токенов + 300 × 120 выходных; оценка и фактическое значение показываются в UI).
- **NFR-08-04** — API-ключ не попадает в логи, отчёты, `scans/*.json` и сообщения об ошибках.

## 4. Дизайн

### 4.1 Публичный API

```rust
// sk-llm
pub struct ClassifyItem {
    pub id: String,                     // FindingId (SPEC-02 §2.7), связывает ответ с находкой
    pub summary: FolderSummary,         // уже после redact
    pub peek: Vec<ContentPeek>,         // пусто, если allow_content_peek = false (§4.9)
    pub hint: Option<String>,           // подсказка эвристики: "unity-like", "electron" ...
}

pub struct Classification {
    pub id: String,
    pub category: Category,             // SPEC-02 §2.3; после валидации
    pub app_guess: Option<String>,      // "OBS Studio"
    pub importance: f32,                // 0..=1, clamp
    pub confidence: f32,                // 0..=1, clamp
    pub reason: String,                 // ≤ 200 символов, обрезается
    pub suggested_include: Vec<String>, // глобы относительно корня, валидированные
    pub suggested_exclude: Vec<String>,
    pub from_cache: bool,
}

pub struct ClassifierInfo {
    pub provider: &'static str,         // "ollama" | "openai_compat" | "anthropic" | "mock"
    pub model: String,
    pub is_local: bool,
    pub max_batch: usize,
    pub max_concurrency: usize,
}

#[async_trait]
pub trait Classifier: Send + Sync {
    fn info(&self) -> ClassifierInfo;
    async fn health(&self) -> Result<HealthReport, LlmError>;
    /// Классифицирует ОДИН батч (≤ info().max_batch). Порядок ответа не гарантирован — сопоставление по id.
    async fn classify(&self, items: &[ClassifyItem], cancel: &CancellationToken)
        -> Result<Vec<Classification>, LlmError>;
}

/// Оркестратор фазы Classify: кэш, отбор, батчинг, параллелизм, ретраи, прогресс.
pub struct ClassifyRunner { classifier: Arc<dyn Classifier>, cache: LlmCache, cfg: LlmConfig }

impl ClassifyRunner {
    pub fn from_config(cfg: &LlmConfig, data_dir: &Path, secrets: &dyn SecretStore) -> Result<Option<Self>, LlmError>; // None при mode=off
    pub fn preview(&self, items: &[ClassifyItem]) -> RequestPreview;             // §4.8
    pub async fn run(&self, items: Vec<ClassifyItem>, events: &EventSink, cancel: &CancellationToken)
        -> RunOutput;                                                            // never Err: ошибки → issues
    pub fn estimate_cost(&self, n_items: usize) -> Option<CostEstimate>;         // только cloud
}

pub struct RunOutput { pub results: Vec<Classification>, pub issues: Vec<ScanIssue>, pub usage: UsageStats }

#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    #[error("provider unreachable: {0}")] Unreachable(String),
    #[error("model not found: {0}")] ModelNotFound(String),
    #[error("auth failed")] Auth,
    #[error("rate limited, retry after {0:?}")] RateLimited(Option<Duration>),
    #[error("overloaded")] Overloaded,
    #[error("bad response: {0}")] BadResponse(String),
    #[error("timeout")] Timeout,
    #[error("cancelled")] Cancelled,
}
```

Интеграция в `sk-engine` (SPEC-01 §4.4, фаза `Classify`): engine собирает `ClassifyItem` из
находок `Unknown` + `ScanReport::unknown_summaries`, вызывает `ClassifyRunner::run` и применяет
результаты через `apply_classification(&mut Finding, &Classification, &ClassifierInfo)`.

### 4.2 Провайдеры

Rust не имеет официального Anthropic SDK, поэтому все провайдеры реализованы на `reqwest`
(rustls, без native-tls) поверх raw HTTP.

#### 4.2.1 Ollama
- `POST {endpoint}/api/chat`:
  ```json
  { "model": "qwen2.5:7b-instruct", "stream": false,
    "messages": [{"role":"system","content":"<SYSTEM_PROMPT>"},{"role":"user","content":"<ITEMS_JSON>"}],
    "format": { /* JSON Schema §4.5 */ },
    "options": { "temperature": 0, "num_ctx": 8192, "seed": 42 },
    "keep_alive": "10m" }
  ```
- Ответ: `message.content` — строка JSON.
- Health: `GET /api/version` (доступность) + `GET /api/tags` (список моделей → есть ли настроенная).
- `max_batch = 8`, `max_concurrency = 1` (локальная GPU, параллельность только мешает).

#### 4.2.2 OpenAI-совместимый (llama.cpp `llama-server`, LM Studio, vLLM)
- `POST {endpoint}/v1/chat/completions`:
  ```json
  { "model": "<model>", "temperature": 0,
    "messages": [...],
    "response_format": { "type": "json_schema",
                         "json_schema": { "name": "classifications", "strict": true, "schema": { /* §4.5 */ } } } }
  ```
- Ответ: `choices[0].message.content`.
- Health: `GET /v1/models`.
- Если сервер не поддерживает `json_schema` (HTTP 400 с упоминанием `response_format`), происходит fallback на `{"type":"json_object"}` + схема в промпте. Флаг кэшируется на сессию.
- `max_batch = 8`, `max_concurrency = 1` (настраивается).
- API-ключ опционален (`Authorization: Bearer`).

#### 4.2.3 Anthropic (Messages API)
- `POST https://api.anthropic.com/v1/messages`, заголовки: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`.
- Тело:
  ```json
  { "model": "claude-haiku-4-5",
    "max_tokens": 4096,
    "system": [{ "type": "text", "text": "<SYSTEM_PROMPT>", "cache_control": { "type": "ephemeral" } }],
    "messages": [{ "role": "user", "content": "<ITEMS_JSON>" }],
    "output_config": { "format": { "type": "json_schema", "schema": { /* §4.5 */ } } } }
  ```
- Модели: по умолчанию `claude-haiku-4-5` (дёшево, поддерживает structured outputs). Опция качества — `claude-sonnet-5`. Для Sonnet 5 дополнительно передаётся `"output_config": { "effort": "low", "format": … }`: классификация не требует глубокого рассуждения. Для Haiku 4.5 параметр `effort` **не передаётся** (ошибка API). Model id — только точные строки без суффиксов дат.
- Structured outputs (`output_config.format`) гарантируют валидный JSON по схеме. Числовые и строковые ограничения (`minimum`, `maximum`, `maxLength`) схемой API **не поддерживаются**, поэтому в отправляемой схеме их нет, а проверка идёт на клиенте (§4.6). Каждый объект схемы должен иметь `additionalProperties: false`.
- Ответ: `content[]` → первый блок `type = "text"` → JSON. Обязательно проверяется `stop_reason`: `end_turn` — ок; `max_tokens` — батч делится пополам и повторяется; `refusal` — элементы батча остаются `Unknown` с issue `llm.refusal`.
- Prompt caching: `cache_control` на системном промпте. Минимальный кэшируемый префикс зависит от модели: для Haiku 4.5 — 4096 токенов, для Sonnet 5 — 1024. Системный промпт (~1500 токенов) на Haiku кэшироваться не будет, и это допустимо. Проверяется по `usage.cache_read_input_tokens`, значение пишется в `UsageStats`.
- Ошибки: `429 rate_limit_error` → ждать `retry-after` (секунды), иначе backoff. `529 overloaded_error` и `5xx` → backoff. `401/403` → `LlmError::Auth` без ретраев. `400` → `BadResponse` без ретраев.
- Health: минимальный запрос `max_tokens: 1` с текстом `"ping"` (стоит доли цента) **или** `GET /v1/models/{model}` (бесплатно, предпочтительно).
- `max_batch = 20`, `max_concurrency = 4`.

#### 4.2.4 Mock
`MockClassifier` для тестов: возвращает ответы из таблицы (`HashMap<PathTemplate, Classification>`)
и умеет симулировать задержку, ошибки 429/529, невалидный JSON и отмену.

Живёт в `sk-llm` (модуль `mock`): публичен под feature `mock`, в тестах самого крейта доступен через `cfg(test)`.
Другие крейты (`sk-engine`) подключают его как `sk-llm = { features = ["mock"] }` в `[dev-dependencies]`.
В `sk-testkit` его нет: зависимость `sk-testkit → sk-llm` вместе с dev-зависимостью `sk-llm → sk-testkit`
дала бы в unit-тестах `sk-llm` две копии крейта, и мок реализовывал бы «чужой» трейт `Classifier`.

### 4.3 Системный промпт (prompt_version = "v1")

Хранится в `crates/sk-llm/prompts/classify_v1.txt` (include_str!). Изменение текста требует
увеличить `PROMPT_VERSION`, что инвалидирует кэш.

```text
You are a file-system analyst helping a Windows user decide what to back up before reinstalling Windows.
Everything that can be re-downloaded or reinstalled should NOT be backed up; everything the user created
or configured and cannot easily recreate SHOULD be backed up.

You receive a JSON array of folder summaries. Each item has an "id" and a "summary" with:
- path: an anonymized path template. Tokens: {HOME} user profile, {APPDATA} Roaming, {LOCALAPPDATA} Local,
  {LOCALLOW} LocalLow, {DOCUMENTS}, {SAVED_GAMES}, {PROGRAMDATA}, {DRIVE:X} other drives.
- total_bytes, file_count, dir_count, max_depth
- newest_mtime / oldest_mtime (RFC 3339)
- ext_histogram: most common extensions with counts and bytes
- sample_names: up to 20 relative file paths (some parts may be "<redacted>")
- top_children: largest subfolders
- markers: precomputed hints (HasExecutables, CacheLike, ConfigLike, SqliteFiles, ElectronApp, UnityGame, ...)
- optional "peek": the first bytes of a few small text config files (secrets removed)
- optional "hint": a guess from local heuristics; it may be wrong

For EACH item return one object with:
- id: copy the input id exactly
- category: one of
  game_save      - saved games / progress
  game_config    - game settings, keybindings, mods configuration
  app_config     - small application settings/preferences
  app_data       - application data the user would miss (databases, profiles, libraries, local projects)
  user_files     - documents, media or projects created by the user
  dev_environment- developer tools configuration, SDK settings, keys used for development
  credentials    - password databases, private keys, certificates, tokens
  system_settings- operating system level settings
  reinstallable  - program binaries, downloaded content, anything restored by reinstalling or re-downloading
  cache          - caches, temporary files, logs, crash dumps, update leftovers
  unknown        - you cannot tell with reasonable confidence
- app_guess: the most likely application or game name, or null
- importance: 0.0-1.0, how painful it would be to lose this folder (1.0 = irreplaceable user work)
- confidence: 0.0-1.0, how sure you are about the category
- reason: one short sentence (max 200 characters) explaining the decision in plain English
- suggested_include: glob patterns relative to the folder to back up (empty = everything)
- suggested_exclude: glob patterns relative to the folder to skip (caches, logs, binaries inside it)

Rules:
- Judge by structure and names only. Never invent facts; if unsure use "unknown" with low confidence.
- Large folders dominated by .exe/.dll/.pak/.bundle files are usually "reinstallable".
- Folders named like caches or full of .tmp/.log/.dmp are "cache" even if large.
- Small JSON/INI/XML/CFG-dominated folders with recent changes are usually "app_config".
- SQLite databases or Electron "IndexedDB"/"Local Storage" with recent changes are usually "app_data".
- Prefer suggesting excludes for cache-like subfolders instead of rejecting the whole folder.
- Return exactly one result per input id, in any order. Output JSON only.
```

Сообщение `user` — компактный JSON `{"items":[{id, summary, peek?, hint?}, …]}` (serde, без
форматирования, поля с пустыми массивами опускаются). Метки времени усекаются до дня, чтобы
кэш-ключ не менялся из-за секунд (§4.7).

### 4.4 Конфигурация

Секция `llm` (SPEC-01 §4.8.2), расширенная. Тип `LlmConfig` (и `LlmMode`, SPEC-02 §6) определён в `sk-core::config` и реэкспортируется из `sk-llm`; поля и дефолты ниже нормативны:
```jsonc
"llm": {
  "mode": "off",
  "local":  { "kind": "ollama", "endpoint": "http://127.0.0.1:11434", "model": "qwen2.5:7b-instruct",
              "timeout_s": 120, "max_batch": 8, "max_concurrency": 1 },
  "cloud":  { "kind": "anthropic", "model": "claude-haiku-4-5", "api_key_source": "credential_manager", // | "env"
              "api_key_env": "ANTHROPIC_API_KEY", "timeout_s": 60, "max_batch": 20, "max_concurrency": 4 },
  "max_items_per_scan": 300,
  "allow_content_peek": false,          // для local; для cloud требуется отдельно:
  "allow_content_peek_cloud": false,
  "confirm_cloud_each_scan": true,
  "min_confidence_to_apply": 0.5
}
```
`kind` для local: `ollama | openai_compat`. Для cloud в v1 поддерживается только `anthropic`
(а также `openai_compat` с внешним URL — только с явным предупреждением о приватности).

### 4.5 JSON-схема ответа

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["results"],
  "properties": {
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "category", "app_guess", "importance", "confidence", "reason",
                     "suggested_include", "suggested_exclude"],
        "properties": {
          "id":         { "type": "string" },
          "category":   { "type": "string", "enum": ["game_save","game_config","app_config","app_data","user_files",
                                                     "dev_environment","credentials","system_settings",
                                                     "reinstallable","cache","unknown"] },
          "app_guess":  { "anyOf": [{ "type": "string" }, { "type": "null" }] },
          "importance": { "type": "number" },
          "confidence": { "type": "number" },
          "reason":     { "type": "string" },
          "suggested_include": { "type": "array", "items": { "type": "string" } },
          "suggested_exclude": { "type": "array", "items": { "type": "string" } }
        }
      }
    }
  }
}
```
Значения `enum` совпадают с `serde(rename_all = "snake_case")` для `Category` (SPEC-02 §2.3).
Тест проверяет это автоматически.

### 4.6 Валидация и нормализация ответа

1. Парсинг JSON. При ошибке — один ретрай с тем же батчем, затем issue `llm.bad_json`.
2. Сопоставление по `id`. Лишние id игнорируются, отсутствующие → повтор для них одним мини-батчем, затем `Unknown`.
3. `category`: неизвестное значение → `Unknown`.
4. `importance`, `confidence`: NaN → 0, clamp к `[0, 1]`.
5. `reason`: trim, удаление управляющих символов, обрезка до 200 символов по границе char, не пустой (иначе `"—"`).
6. `app_guess`: trim, ≤ 80 символов, пустая строка → None.
7. Глобы: парсятся `globset`. Невалидные, абсолютные, содержащие `..` или начинающиеся с диска отбрасываются. Не более 20 шт. каждого вида.
8. Применение: если `confidence < min_confidence_to_apply`, категория остаётся `Unknown`, но `Evidence` добавляется (reason видна пользователю). LLM **не может** переопределить находки из правил и Ludusavi. Engine передаёт в LLM только `Unknown`, а SPEC-09 разрешает конфликты по приоритету источников.

### 4.7 Кэш

- Файл: `savekeeper-data/cache/llm-cache.jsonl`. Решение по вопросу SPEC-01 §9: **JSONL** (без C-зависимостей, достаточно для ≤ 10⁴ записей).
- Ключ: `blake3( canonical_json(summary_for_prompt) + "\n" + peek_digest + "\n" + provider + ":" + model + "\n" + PROMPT_VERSION )`, hex.
  `summary_for_prompt` — ровно то, что отправляется (после redact, с датами, усечёнными до дня).
- Строка: `{"key": "...", "created_at": "...", "provider": "...", "model": "...", "prompt_version": "v1", "result": { /* Classification без id */ }}`.
- Загрузка при старте в `HashMap` (последняя запись по ключу побеждает). Дозапись append-only с `flush` после каждого батча. Компакция при > 20 000 строк или при > 30% устаревших (дубли, другая версия промпта): переписывание во временный файл и атомарный `rename`.
- TTL: 180 дней.
- Битые строки пропускаются с warn в лог.
- Кнопка «Очистить кэш LLM» в UI удаляет файл.

### 4.8 Приватность и превью

- `RequestPreview { provider, model, is_local, item_count, sample: Vec<serde_json::Value> /* первые 3 элемента, ровно как уйдут */, total_bytes_to_send, est_cost_usd }`.
- Для `cloud` UI показывает превью в модальном окне (SPEC-11) до первого запроса скана, если `confirm_cloud_each_scan = true`. Отказ → фаза `Classify` пропускается с issue `llm.cloud_declined`.
- Перед отправкой каждого элемента выполняется **повторная** проверка (defense in depth): если в сериализованном JSON найдено `env.user_name`, `env.machine_name`, абсолютный путь (`[A-Za-z]:\\` вне `{DRIVE:X}`), email или строка, похожая на токен, элемент исключается и выдаётся issue `llm.privacy_blocked`. Функция `sk_llm::privacy::final_check(&str, &Environment) -> Result<(), PrivacyViolation>`.
- Логи содержат только количество элементов, модель, длительность и usage, без содержимого запросов.

### 4.9 Content peek (опционально)

- Включается `allow_content_peek` (local) и отдельно `allow_content_peek_cloud` (cloud). Оба по умолчанию `false`.
- Берутся до 3 файлов на папку: расширения `json ini cfg xml toml yaml yml conf txt`, размер ≤ 64 КБ, валидный UTF-8 (или UTF-16 LE с BOM → конвертация).
- Читаются первые 2 КБ, затем выполняется скраб секретов:
  - строки, где ключ совпадает с `(?i)(pass(word)?|pwd|secret|token|api[_-]?key|auth|cookie|session|private|bearer|credential)` → значение заменяется на `<redacted>`;
  - строки длиной ≥ 24 символов `[A-Za-z0-9+/=_-]` → `<redacted>`;
  - email, IPv4/IPv6, URL с user:pass → `<redacted>`;
  - затем `redact()` из SPEC-02 §4.2.
- `ContentPeek { file: String /* относительное имя */, head: String }`.
- Файлы из папок категории-кандидата `Credentials` и с расширениями из группы `credentials` (SPEC-07 §4.3.2) никогда не читаются.

### 4.10 Хранение API-ключа

- Трейт `SecretStore { get(name) -> Option<SecretString>; set(name, value); delete(name) }`.
- Реализации: `CredentialManagerStore` (крейт `keyring`, Windows Credential Manager, target `SaveKeeper/anthropic_api_key`) и `EnvStore` (`api_key_env`). Порядок чтения: сначала `api_key_source`, затем другой источник как fallback.
- В портативном режиме ключ лежит в Credential Manager **текущего пользователя** и не переезжает с флешкой. UI это объясняет.
- Тип `secrecy::SecretString`: `Debug` не раскрывает значение, в заголовок подставляется только в момент запроса.

### 4.11 Рекомендуемые модели (UI-подсказки)

| Провайдер | Модель | Когда |
|---|---|---|
| Ollama | `qwen2.5:7b-instruct` (по умолчанию) | GPU ≥ 6 ГБ VRAM |
| Ollama | `qwen2.5:3b-instruct`, `llama3.2:3b` | слабое железо / только CPU |
| Ollama | `qwen2.5:14b-instruct` | GPU ≥ 12 ГБ, лучшее качество |
| LM Studio / llama.cpp | любая instruct-модель с поддержкой JSON schema (GGUF Q4_K_M) | — |
| Anthropic | `claude-haiku-4-5` (по умолчанию) | дёшево и быстро |
| Anthropic | `claude-sonnet-5` | выше качество, ~2× дороже Haiku на вход |

Список хранится в `crates/sk-llm/data/recommended.yaml` и показывается в UI. Если модели нет
в Ollama, UI показывает команду `ollama pull <model>`. Программа сама модель не скачивает.

### 4.12 Батчинг, параллелизм, ретраи

- Порядок: dedupe по кэш-ключу → попадания в кэш → отбор (FR-08-06) → сортировка по `path` (детерминизм) → чанки по `max_batch`, дополнительно ограниченные ~12 000 символами входа на батч.
- Параллелизм: `futures::stream::iter(batches).buffer_unordered(max_concurrency)`.
- Таймаут на запрос: `timeout_s`. Общий бюджет фазы: `max(60 с, 2 × n_batches × timeout_s / concurrency)`.
- Ретраи: до 3 попыток, экспоненциальный backoff `1 с, 2 с, 4 с` + jitter ±20%. При 429 используется `retry-after`, если он есть (но не более 60 с). Если за фазу 3 батча подряд завершились `Unreachable`/`Auth`, фаза прерывается, остальные элементы остаются `Unknown`, выдаётся issue `llm.provider_failed`.
- Прогресс: `Event::Progress { phase: Classify, done: items_done, total: Some(items_total), current: Some(app_guess or path) }`.

## 5. Ошибки и граничные случаи

| Ситуация | Поведение |
|---|---|
| `mode = local`, провайдер недоступен | Health-check в начале фазы → issue `llm.unreachable` (с подсказкой «запустите Ollama»), фаза пропускается. Скан успешен. |
| Модель не установлена в Ollama | issue `llm.model_missing` с командой `ollama pull`. |
| Нет API-ключа при `mode = cloud` | Фаза пропускается, issue `llm.no_api_key`. UI ведёт в настройки. |
| 401/403 | `Auth`, без ретраев, issue `llm.auth_failed`. |
| `stop_reason = max_tokens` | Батч делится пополам (рекурсивно до 1 элемента). |
| `stop_reason = refusal` | Элементы остаются `Unknown`, issue `llm.refusal`. |
| Модель вернула результат для чужого id | Игнорируется. |
| Два элемента с одинаковой сводкой (одна и та же структура у разных папок) | Один запрос, результат дублируется по id (кэш-ключ одинаковый). |
| Отмена посреди батча | Drop future, уже полученные результаты сохранены в кэш, `RunOutput` частичный. |
| Корпоративный прокси | `reqwest` уважает `HTTPS_PROXY`. Локальные endpoint'ы идут в обход прокси (`no_proxy` для 127.0.0.1/localhost). |
| Сводка слишком большая (например, длинные имена) | `sample_names` обрезаются до 120 символов каждое, общий лимит 4 000 символов на элемент. |

## 6. Тестирование

### 6.1 Unit
- Сериализация запроса для каждого провайдера — snapshot-тест тела (insta), без ключа.
- Enum схемы §4.5 совпадает с `Category` (итерация по всем вариантам).
- Отправляемая Anthropic-схема не содержит `minimum`/`maximum`/`maxLength`, и все объекты имеют `additionalProperties: false`.
- Валидация §4.6: clamp, NaN, неизвестная категория, плохие глобы, `..`, абсолютные пути, отсутствующие id.
- `final_check`: ловит имя пользователя, машину, `C:\Users\…`, email, токен.
- Скраб peek §4.9: пароли в INI/JSON/XML, длинные токены, UTF-16.
- Кэш: запись/чтение, последняя побеждает, битая строка, компакция атомарна, смена `PROMPT_VERSION` → промах.

### 6.2 Интеграционные (без сети)
- `wiremock`-сервер, эмулирующий Ollama, OpenAI-compat и Anthropic: успех, 429 с `retry-after`, 529, 500, невалидный JSON, `stop_reason: max_tokens`, `refusal`, таймаут. Проверяются ретраи, деление батча, issues и отмена за ≤ 1 с.
- `ClassifyRunner` + `MockClassifier`: лимит `max_items_per_scan`, приоритет отбора, детерминизм порядка, повторный прогон на 100% из кэша (0 запросов).

### 6.3 Eval (ручной / nightly, с сетью)
- Набор: `fixtures/llm-eval/*.json` — ≥ 150 `FolderSummary` с реальных машин (обезличенных) + `expected_category` + `acceptable_categories` (например, `app_config|app_data`).
- Покрытие: ≥ 10 примеров на каждую категорию, кроме `unknown`, включая «ловушки» (большой кэш с `.json`, игра в `Documents`, Electron-приложение).
- Бинарник `cargo run -p sk-llm --example eval -- --provider ollama --model qwen2.5:7b-instruct`. Выводит accuracy (строгую и по `acceptable`), confusion matrix, среднюю латентность и стоимость.
- Порог — NFR-08-02. Результаты фиксируются в `fixtures/llm-eval/RESULTS.md` при каждом изменении промпта.

## 7. Задачи

- [ ] **T-08-01** — Каркас `sk-llm`: типы §4.1, `LlmError`, реэкспорт `LlmConfig` (тип и дефолты §4.4 реализуются в SPEC-01 T-01-04), `MockClassifier` под feature `mock` (§4.2.4). *Зависит:* T-02-01, T-01-04. *Готово, когда:* компилируется, unit-тесты mock.
- [ ] **T-08-02** — Промпт `classify_v1.txt`, `PROMPT_VERSION`, JSON-схема §4.5 как константа + тест соответствия `Category`. *Зависит:* T-08-01.
- [ ] **T-08-03** — Валидация и нормализация §4.6 + `apply_classification`. *Зависит:* T-08-01.
- [ ] **T-08-04** — Приватность: `final_check`, построение `summary_for_prompt` (усечение дат, лимиты длины), `RequestPreview`. *Зависит:* T-02-04. *Готово, когда:* тесты §6.1.
- [ ] **T-08-05** — Кэш JSONL §4.7. *Зависит:* T-08-04.
- [ ] **T-08-06** — Провайдер Ollama + health + авто-детект. *Зависит:* T-08-02, T-08-03. *Готово, когда:* wiremock-тесты.
- [ ] **T-08-07** — Провайдер OpenAI-compat + fallback на `json_object`. *Зависит:* T-08-06.
- [ ] **T-08-08** — Провайдер Anthropic: тело §4.2.3, structured outputs, prompt caching, обработка `stop_reason`, 429/529/5xx, health через `/v1/models/{model}`, `UsageStats`, оценка стоимости. *Зависит:* T-08-02, T-08-03. *Готово, когда:* wiremock-тесты для всех кодов.
- [ ] **T-08-09** — `SecretStore`: Credential Manager (`keyring`) + env, `secrecy`. *Готово, когда:* Windows-тест set/get/delete. Ключ не виден в `Debug` и логах.
- [ ] **T-08-10** — `ClassifyRunner`: отбор, батчинг, `buffer_unordered`, ретраи с backoff, деление батча, прогресс, отмена, частичные результаты. *Зависит:* T-08-05, T-08-06. *Готово, когда:* интеграционные тесты §6.2.
- [ ] **T-08-11** — Content peek + скраб секретов §4.9. *Зависит:* T-08-04.
- [ ] **T-08-12** — Встраивание в `sk-engine` (фаза `Classify`), issues, события. *Зависит:* T-08-10, T-01-06.
- [ ] **T-08-13** — `recommended.yaml` + API для UI: список рекомендаций, health, `ollama pull`-подсказка. *Зависит:* T-08-06.
- [ ] **T-08-14** — Eval-набор (≥ 150 элементов) и `examples/eval`. Первые результаты в `RESULTS.md`. *Зависит:* T-08-06, T-08-08.

## 8. Критерии приёмки

- [ ] При `mode = off` сетевой трафик от программы отсутствует (проверка: тест с `reqwest`-клиентом, который паникует при создании, и ручная проверка в Resource Monitor).
- [ ] Ни один отправленный запрос в eval-прогоне и в интеграционных тестах не содержит имени пользователя или машины (автоматическая проверка на wiremock).
- [ ] Повторный скан без изменений ФС делает 0 запросов к LLM.
- [ ] NFR-08-02 (accuracy) достигнут для обеих моделей по умолчанию.
- [ ] Отмена фазы `Classify` завершается ≤ 1 с.
- [ ] UI показывает превью и оценку стоимости перед облачной классификацией.

## 9. Открытые вопросы

- **Агентный режим (future, расширение SPEC-08):** модель с инструментами `list_dir(path_template)`, `read_head(file, 2KB)` исследует сложные папки сама. Требует tool use, лимитов на число шагов и более строгой приватности. В v1 не делаем.
- **Message Batches API** (50% скидка, асинхронно) подходит для фоновых сканов, но не для интерактивных. Рассмотреть для режима «глубокий анализ ночью».
- **Эмбеддинги** путей и сводок для кластеризации похожих папок и обучения на решениях пользователя: см. расширение SPEC-09.
- **Предложение к SPEC-01 §4.8.2:** заменить `"api_key_env"` на пару `api_key_source` + `api_key_env`, добавить `allow_content_peek_cloud`, `confirm_cloud_each_scan`, `min_confidence_to_apply`, `timeout_s`, `max_batch`, `max_concurrency` (§4.4).
- **Предложение к SPEC-01 §4.2:** добавить в зависимости `sk-llm` крейты `keyring`, `secrecy`, `wiremock` (dev).
- **Предложение к SPEC-02:** нужен ли `Evidence.message_args["importance"]`, чтобы SPEC-09 мог использовать `importance` от LLM? Альтернатива — отдельное поле `Evidence.importance: Option<f32>`. Предварительно: `message_args["importance"]` как строка с числом. SPEC-09 парсит её.
- Нужно ли давать LLM `hint` от эвристики, или это смещает ответ? Решить по результатам eval (A/B с hint и без).
