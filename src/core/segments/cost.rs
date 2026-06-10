use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId, TranscriptEntry};
use crate::utils::credentials;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct CostSegment;

impl CostSegment {
    pub fn new() -> Self {
        Self
    }
}

/// 从单个 transcript 文件累计 token（区分缓存命中/未命中/输出）
fn tokens_from_file(path: &Path) -> (u64, u64, u64) {
    let mut input_uncached: u64 = 0;
    let mut input_cached: u64 = 0;
    let mut output: u64 = 0;

    if let Ok(file) = fs::File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line.trim()) {
                if let Some(msg) = &entry.message {
                    if let Some(u) = &msg.usage {
                        let n = u.clone().normalize();
                        let cached = n.cache_read_input_tokens.min(n.input_tokens);
                        input_uncached += (n.input_tokens - cached) as u64;
                        input_cached += cached as u64;
                        output += n.output_tokens as u64;
                    }
                }
            }
        }
    }
    (input_uncached, input_cached, output)
}

/// 计算单个 session 的花费
fn calc_cost(
    pricing: &crate::config::models::ModelPricing,
    input_uncached: u64,
    input_cached: u64,
    output: u64,
) -> f64 {
    (input_uncached as f64 / 1_000_000.0) * pricing.price_per_million_input
        + (input_cached as f64 / 1_000_000.0) * pricing.price_per_million_input_cached
        + (output as f64 / 1_000_000.0) * pricing.price_per_million_output
}

/// 获取当天所有 transcript 文件的路径列表
fn today_transcript_files(transcript_path: &str) -> Vec<PathBuf> {
    let current_path = Path::new(transcript_path);
    let project_dir = match current_path.parent() {
        Some(d) => d,
        None => return vec![current_path.to_path_buf()],
    };

    let today = Local::now().format("%Y-%m-%d").to_string();
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            // 检查文件修改日期是否为今天
            if let Ok(meta) = path.metadata() {
                if let Ok(modified) = meta.modified() {
                    let dt: DateTime<Local> = modified.into();
                    if dt.format("%Y-%m-%d").to_string() == today {
                        files.push(path);
                    }
                }
            }
        }
    }
    // 确保当前文件在列表中
    if !files.contains(&current_path.to_path_buf()) {
        files.push(current_path.to_path_buf());
    }
    files
}

#[derive(Debug, Serialize, Deserialize)]
struct BalanceCache {
    balance: String,
    cached_at: String,
}

#[derive(Debug, Deserialize)]
struct BalanceInfo {
    currency: String,
    total_balance: String,
}

#[derive(Debug, Deserialize)]
struct BalanceResponse {
    balance_infos: Option<Vec<BalanceInfo>>,
}

fn fetch_balance(api_url: &str, token: &str, timeout_secs: u64) -> Option<String> {
    let agent = ureq::Agent::new_with_defaults();
    let response = agent
        .get(api_url)
        .header("Authorization", &format!("Bearer {}", token))
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build()
        .call()
        .ok()?;
    let body: BalanceResponse = response.into_body().read_json().ok()?;
    body.balance_infos
        .and_then(|infos| infos.into_iter().next())
        .map(|info| format!("{} {}", info.total_balance, info.currency))
}

impl Segment for CostSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let model_config = ModelConfig::load();

        // ---- Part 1: 当前 session 消耗 from transcript ----
        let (session_uncached, session_cached, session_output) =
            tokens_from_file(Path::new(&input.transcript_path));

        let session_cost = model_config
            .get_pricing(&input.model.id)
            .map(|p| calc_cost(&p, session_uncached, session_cached, session_output));

        let session_display = session_cost.map_or("-".to_string(), |c| format!("{:.2}", c));

        // ---- Part 2: 本日消耗 from all today's transcripts ----
        let today_files = today_transcript_files(&input.transcript_path);
        let (day_uncached, day_cached, day_output) = today_files
            .iter()
            .map(|f| tokens_from_file(f))
            .fold((0u64, 0u64, 0u64), |(a1, a2, a3), (b1, b2, b3)| {
                (a1 + b1, a2 + b2, a3 + b3)
            });

        let day_cost = model_config
            .get_pricing(&input.model.id)
            .map(|p| calc_cost(&p, day_uncached, day_cached, day_output));

        let day_display = day_cost.map_or("-".to_string(), |c| format!("{:.2}", c));

        // ---- Part 3: 总余额 from DeepSeek API ----
        let balance_display = 'bal: {
            let config = crate::config::Config::load().ok()?;
            let segment_config = config.segments.iter().find(|s| s.id == SegmentId::Cost)?;
            let api_url = segment_config
                .options
                .get("balance_api_url")
                .and_then(|v| v.as_str())?;
            let token = credentials::get_api_key()?;
            fetch_balance(api_url, &token, 3)?
        };

        // ---- 组装显示 ----
        let primary = format!("¥{}|¥{}|¥{}", session_display, day_display, balance_display);

        let mut metadata = HashMap::new();
        if let Some(c) = session_cost {
            let _ = c;
        }
        metadata.insert("source".to_string(), "deepseek".to_string());

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Cost
    }
}
