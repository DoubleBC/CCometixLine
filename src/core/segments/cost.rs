use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId, TranscriptEntry};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Default)]
pub struct CostSegment;

impl CostSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for CostSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let model_config = ModelConfig::load();

        // 尝试获取当前模型的自定义定价
        if let Some(pricing) = model_config.get_pricing(&input.model.id) {
            // 从 transcript 累计 token 用量，用自定义定价计算成本
            let (total_input, total_output) =
                cumulative_tokens_from_transcript(&input.transcript_path);

            if total_input > 0 || total_output > 0 {
                let cost = (total_input as f64 / 1_000_000.0) * pricing.price_per_million_input
                    + (total_output as f64 / 1_000_000.0) * pricing.price_per_million_output;

                let primary = if cost < 0.01 {
                    "$0".to_string()
                } else {
                    format!("${:.4}", cost)
                };

                let secondary = String::new();

                let mut metadata = HashMap::new();
                metadata.insert("cost".to_string(), cost.to_string());
                metadata.insert("source".to_string(), "custom_pricing".to_string());

                return Some(SegmentData {
                    primary,
                    secondary,
                    metadata,
                });
            }
        }

        // 回退到 Claude Code 内置 cost
        let cost_data = input.cost.as_ref()?;

        let primary = if let Some(cost) = cost_data.total_cost_usd {
            if cost == 0.0 || cost < 0.01 {
                "$0".to_string()
            } else {
                format!("${:.2}", cost)
            }
        } else {
            return None;
        };

        let secondary = String::new();

        let mut metadata = HashMap::new();
        if let Some(cost) = cost_data.total_cost_usd {
            metadata.insert("cost".to_string(), cost.to_string());
        }
        metadata.insert("source".to_string(), "claude_code".to_string());

        Some(SegmentData {
            primary,
            secondary,
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Cost
    }
}

/// 解析 transcript 文件，累计所有消息的 input/output token 用量
fn cumulative_tokens_from_transcript(transcript_path: &str) -> (u64, u64) {
    let path = Path::new(transcript_path);
    let mut total_input: u64 = 0;
    let mut total_output: u64 = 0;

    if let Ok(file) = fs::File::open(path) {
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<TranscriptEntry>(line) {
                if let Some(message) = &entry.message {
                    if let Some(raw_usage) = &message.usage {
                        let normalized = raw_usage.clone().normalize();
                        total_input += normalized.input_tokens as u64;
                        total_output += normalized.output_tokens as u64;
                    }
                }
            }
        }
    }

    (total_input, total_output)
}
