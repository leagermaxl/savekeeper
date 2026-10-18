//! LLM classification eval (SPEC-08 §6.3, NFR-08-02).
//!
//! ```text
//! cargo run -p sk-llm --example eval -- --provider ollama --model qwen2.5:7b-instruct
//! ```
//!
//! Sends the cases of `fixtures/llm-eval/*.json` straight to the classifier
//! (no cache, so every run measures the model) in batches like
//! `ClassifyRunner` (`max_batch`, `MAX_BATCH_INPUT_CHARS`), and prints strict
//! accuracy, accuracy by `acceptable_categories`, the confusion matrix, mean
//! latency and cost. Each request passes `final_check` against the current
//! machine's environment first; a violation stops the run before sending.
//!
//! Options: `--provider ollama|openai-compat|anthropic` and `--model <m>`
//! (required); `--endpoint <url>` (local providers); `--dir <folder>`
//! (default `fixtures/llm-eval`); `--batch <n>` (default the provider's
//! `max_batch`); `--timeout <s>` (default 300); `--limit <n>`;
//! `--api-key-env <VAR>` (Anthropic, default `ANTHROPIC_API_KEY`);
//! `--verbose` (print every answer, not only the mistakes).

mod dataset;
mod report;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sk_core::config::{CloudLlmConfig, LocalLlmConfig, LocalLlmKind};
use sk_core::env::Environment;
use sk_core::model::Category;
use sk_core::CancellationToken;
use sk_llm::{
    final_check, item_for_prompt, user_message, AnthropicClassifier, Classifier, ClassifyItem,
    LlmError, OllamaClassifier, OpenAiCompatClassifier, MAX_BATCH_INPUT_CHARS, PROMPT_VERSION,
};

use dataset::{category_name, LoadedCase};
use report::{confusion_markdown, Latency, Metrics, Outcome};

/// Pause before the single retry of a failed batch.
const RETRY_DELAY: Duration = Duration::from_secs(2);

struct Args {
    provider: String,
    model: String,
    endpoint: Option<String>,
    dir: PathBuf,
    batch: Option<usize>,
    timeout_s: u32,
    limit: Option<usize>,
    api_key_env: String,
    verbose: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        provider: String::new(),
        model: String::new(),
        endpoint: None,
        dir: dataset::default_dir(),
        batch: None,
        timeout_s: 300,
        limit: None,
        api_key_env: "ANTHROPIC_API_KEY".to_owned(),
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "--verbose" {
            args.verbose = true;
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = |v: &str| v.parse::<usize>().map_err(|e| format!("{flag}: {e}"));
        match flag.as_str() {
            "--provider" => args.provider = value,
            "--model" => args.model = value,
            "--endpoint" => args.endpoint = Some(value),
            "--dir" => args.dir = PathBuf::from(value),
            "--batch" => args.batch = Some(number(&value)?.max(1)),
            "--timeout" => {
                args.timeout_s = u32::try_from(number(&value)?).map_err(|e| e.to_string())?;
            }
            "--limit" => args.limit = Some(number(&value)?),
            "--api-key-env" => args.api_key_env = value,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if args.provider.is_empty() || args.model.is_empty() {
        return Err("--provider and --model are required".to_owned());
    }
    Ok(args)
}

fn classifier(args: &Args) -> Result<Arc<dyn Classifier>, String> {
    let local = |kind: LocalLlmKind, default_endpoint: &str| LocalLlmConfig {
        kind,
        endpoint: args
            .endpoint
            .clone()
            .unwrap_or_else(|| default_endpoint.to_owned()),
        model: args.model.clone(),
        timeout_s: args.timeout_s,
        ..LocalLlmConfig::default()
    };
    let err = |e: LlmError| e.to_string();
    Ok(match args.provider.as_str() {
        "ollama" => Arc::new(
            OllamaClassifier::new(&local(LocalLlmKind::Ollama, "http://127.0.0.1:11434"))
                .map_err(err)?,
        ),
        "openai-compat" => Arc::new(
            OpenAiCompatClassifier::new(&local(
                LocalLlmKind::OpenaiCompat,
                "http://127.0.0.1:1234",
            ))
            .map_err(err)?,
        ),
        "anthropic" => {
            let key = std::env::var(&args.api_key_env)
                .map_err(|_| format!("{} is not set", args.api_key_env))?;
            let cfg = CloudLlmConfig {
                model: args.model.clone(),
                timeout_s: args.timeout_s,
                ..CloudLlmConfig::default()
            };
            Arc::new(AnthropicClassifier::new(&cfg, key).map_err(err)?)
        }
        other => return Err(format!("unknown provider {other}")),
    })
}

/// Splits items like the runner: at most `max_batch`, at most
/// `MAX_BATCH_INPUT_CHARS` of input (a single larger item goes alone).
fn batches(items: Vec<ClassifyItem>, max_batch: usize) -> Vec<Vec<ClassifyItem>> {
    let mut out: Vec<Vec<ClassifyItem>> = Vec::new();
    let mut chars = 0;
    for item in items {
        let len = item_for_prompt(&item).to_string().chars().count();
        let full = out.last().is_none_or(|b| {
            b.len() >= max_batch || (!b.is_empty() && chars + len > MAX_BATCH_INPUT_CHARS)
        });
        if full {
            out.push(Vec::new());
            chars = 0;
        }
        chars += len;
        if let Some(batch) = out.last_mut() {
            batch.push(item);
        }
    }
    out
}

fn retryable(e: &LlmError) -> bool {
    !matches!(
        e,
        LlmError::Auth | LlmError::ModelNotFound(_) | LlmError::Cancelled | LlmError::Refusal
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let all = dataset::load_dir(&args.dir)?;
    let problems = dataset::problems(&all);
    if !problems.is_empty() {
        return Err(format!("invalid eval set:\n{}", problems.join("\n")));
    }
    if all.len() < dataset::MIN_CASES {
        eprintln!(
            "warning: {} cases, SPEC-08 §6.3 asks for at least {}",
            all.len(),
            dataset::MIN_CASES
        );
    }
    for (category, n) in dataset::per_category(&all) {
        if n < dataset::MIN_PER_CATEGORY && category != Category::Unknown {
            eprintln!(
                "warning: {} has {n} cases, SPEC-08 §6.3 asks for at least {}",
                category_name(category),
                dataset::MIN_PER_CATEGORY
            );
        }
    }
    let mut order = dataset::mixed_order(&all);
    if let Some(limit) = args.limit {
        order.truncate(limit);
    }
    let cases: Vec<LoadedCase> = order.iter().map(|&i| all[i].clone()).collect();
    let env = Environment::detect().map_err(|e| format!("environment: {e}"))?;

    let classifier = classifier(&args)?;
    let info = classifier.info();
    let health = classifier
        .health()
        .await
        .map_err(|e| format!("health: {e}"))?;
    if !health.model_available {
        return Err(format!("model {} is not available", info.model));
    }
    let max_batch = args.batch.unwrap_or(info.max_batch).max(1);
    println!(
        "eval: {} cases from {}, provider {}, model {}, server {}, prompt {PROMPT_VERSION}, batch {max_batch}",
        cases.len(),
        args.dir.display(),
        info.provider,
        info.model,
        health.server_version.as_deref().unwrap_or("-"),
    );

    let items: Vec<ClassifyItem> = cases
        .iter()
        .enumerate()
        .map(|(n, c)| dataset::to_item(&c.case, dataset::wire_id(n)))
        .collect();
    let batches = batches(items, max_batch);
    let cancel = CancellationToken::new();
    let mut answers: HashMap<String, Outcome> = HashMap::new();
    let mut latency = Latency::default();
    let started = Instant::now();
    for (n, batch) in batches.iter().enumerate() {
        final_check(&user_message(batch), &env)
            .map_err(|v| format!("privacy check failed for batch {n}: {v:?}"))?;
        let mut attempt = 0;
        let outcome = loop {
            attempt += 1;
            let t = Instant::now();
            match classifier.classify(batch, &cancel).await {
                Ok(results) => {
                    latency.batches.push((t.elapsed(), batch.len()));
                    break Ok(results);
                }
                Err(e) if attempt < 2 && retryable(&e) => {
                    eprintln!("batch {n}: {e}; retrying");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(e) => break Err(e),
            }
        };
        match outcome {
            Ok(results) => {
                for r in results {
                    let answer = Outcome::Predicted {
                        category: r.category,
                        confidence: r.confidence,
                        app_guess: r.app_guess,
                        reason: r.reason,
                    };
                    answers.insert(r.id, answer);
                }
            }
            Err(e) => {
                for item in batch {
                    answers.insert(item.id.clone(), Outcome::Failed(e.to_string()));
                }
            }
        }
        eprintln!(
            "batch {}/{} ({} items) done, {:.0?} elapsed",
            n + 1,
            batches.len(),
            batch.len(),
            started.elapsed()
        );
    }
    let outcomes: Vec<Outcome> = (0..cases.len())
        .map(|n| {
            answers
                .remove(&dataset::wire_id(n))
                .unwrap_or(Outcome::Missing)
        })
        .collect();
    print_report(&cases, &outcomes, &latency, &args, started.elapsed());
    // The classifier is fresh, so its counters cover this run only; local
    // providers report no tokens (cost 0).
    let usage = classifier.usage();
    println!(
        "usage reported by the provider: {} requests, {} input / {} output tokens, cost ${:.4}",
        usage.requests, usage.input_tokens, usage.output_tokens, usage.cost_usd
    );
    Ok(())
}

fn print_report(
    cases: &[LoadedCase],
    outcomes: &[Outcome],
    latency: &Latency,
    args: &Args,
    wall: Duration,
) {
    let m = Metrics::compute(cases, outcomes);
    println!();
    println!("date (UTC): {}", time::OffsetDateTime::now_utc().date());
    println!(
        "strict accuracy: {:.3} ({}/{})",
        m.strict_accuracy(),
        m.strict,
        m.total
    );
    println!(
        "accuracy by acceptable: {:.3} ({}/{})",
        m.acceptable_accuracy(),
        m.acceptable,
        m.total
    );
    println!(
        "traps answered acceptably: {}/{}; must_not hits: {}",
        m.traps_acceptable, m.traps, m.must_not_hits
    );
    println!(
        "latency ({} successful requests): mean {:.2?} per request, {:.2?} per item, max {:.2?}; wall {:.0?}",
        latency.batches.len(),
        latency.mean_per_batch(),
        latency.mean_per_item(),
        latency.max(),
        wall
    );
    println!();
    println!("{}", confusion_markdown(&m));
    println!("legend: gs game_save, gc game_config, ac app_config, ad app_data, uf user_files, de dev_environment,");
    println!("        cr credentials, ss system_settings, ri reinstallable, ca cache, un unknown, miss no result, err request failed");
    println!();
    for (loaded, outcome) in cases.iter().zip(outcomes) {
        let case = &loaded.case;
        let (ok, text) = match outcome {
            Outcome::Predicted {
                category,
                confidence,
                app_guess,
                reason,
            } => (
                case.accepts(*category),
                format!(
                    "{} [{confidence:.2}] app={} | {reason}",
                    category_name(*category),
                    app_guess.as_deref().unwrap_or("-")
                ),
            ),
            Outcome::Missing => (false, "<missing>".to_owned()),
            Outcome::Failed(e) => (false, format!("<error: {e}>")),
        };
        if args.verbose || !ok {
            println!(
                "{} {} (expected {}, app {}) -> {text}",
                if ok { "ok  " } else { "MISS" },
                case.id,
                category_name(case.expected_category),
                case.expected_app.as_deref().unwrap_or("-")
            );
        }
    }
}
