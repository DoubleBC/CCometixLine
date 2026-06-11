use super::{Segment, SegmentData};
use crate::config::{SegmentId, InputData};
use crate::utils::credentials;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 每日余额快照，用于计算今日花费
#[derive(Debug, Serialize, Deserialize)]
struct DailySnapshot {
    date: String,           // YYYY-MM-DD，本地日期
    start_balance: f64,     // 当日首次记录的余额
}

impl DailySnapshot {
    fn path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".claude").join("ccline").join(".daily_balance.json")
    }

    fn load() -> Option<Self> {
        let content = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save(&self) {
        if let Some(parent) = Self::path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::write(Self::path(), json);
        }
    }
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

/// 查询 DeepSeek 余额 API，返回 (余额数值, 货币单位)
fn fetch_balance(api_url: &str, token: &str, timeout_secs: u64) -> Option<(f64, String)> {
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
        .and_then(|info| {
            info.total_balance.parse::<f64>().ok().map(|v| (v, info.currency))
        })
}

#[derive(Default)]
pub struct CostSegment;

impl CostSegment {
    pub fn new() -> Self { Self }
}

impl Segment for CostSegment {
    fn collect(&self, _input: &InputData) -> Option<SegmentData> {
        // 读取配置中的 balance_api_url
        let config = crate::config::Config::load().ok()?;
        let segment_config = config.segments.iter().find(|s| s.id == SegmentId::Cost)?;
        let api_url = segment_config.options.get("balance_api_url")?.as_str()?;

        // 获取 API Key 并查询余额
        let token = credentials::get_api_key()?;
        let (balance, currency) = fetch_balance(api_url, &token, 3)?;

        // 今日日期
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        // 加载快照，计算今日花费
        let today_cost = match DailySnapshot::load() {
            Some(snap) if snap.date == today => {
                if balance > snap.start_balance {
                    // 充值了，重置基准
                    DailySnapshot { date: today, start_balance: balance }.save();
                    0.0
                } else {
                    (snap.start_balance - balance).max(0.0)
                }
            }
            _ => {
                // 新的一天或无快照
                DailySnapshot { date: today, start_balance: balance }.save();
                0.0
            }
        };

        let primary = format!(
            "¥{:.2}|¥{:.2}",
            today_cost,
            balance
        );

        let mut metadata = HashMap::new();
        metadata.insert("currency".to_string(), currency);

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId { SegmentId::Cost }
}
