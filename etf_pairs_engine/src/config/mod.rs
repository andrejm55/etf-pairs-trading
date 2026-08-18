use anyhow::{anyhow, Result};
use serde::de::{Error as DeError, Unexpected};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt, fs, path::PathBuf, str::FromStr};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub engine: EngineConfig,
    pub alpaca: AlpacaConfig,
    pub storage: StorageConfig,
    pub strategy: StrategyConfig,
    pub execution: ExecutionConfig,
    pub risk: RiskConfig,
    pub pairs: Vec<PairConfig>,
}

impl AppConfig {
    pub fn from_file(path: &str) -> Result<Self> {
        let raw = fs::read_to_string(path).or_else(|_| {
            let mut fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            fallback.push(path);
            fs::read_to_string(fallback)
        })?;
        Ok(toml::from_str(&raw)?)
    }
    pub fn symbol_universe(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        for p in self.pairs.iter().filter(|p| p.enabled) {
            set.insert(p.a.clone());
            set.insert(p.b.clone());
        }
        set.into_iter().collect()
    }

    pub fn signal_timeframes(&self) -> Vec<SignalTimeframe> {
        let mut set = BTreeSet::new();
        for timeframe in self
            .pairs
            .iter()
            .filter(|p| p.enabled)
            .filter_map(|p| p.signal_timeframe)
            .filter(|timeframe| *timeframe != SignalTimeframe::Tick)
        {
            set.insert(timeframe);
        }
        set.into_iter().collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SignalTimeframe {
    Tick,
    OneHour,
    FourHour,
    OneDay,
}

impl SignalTimeframe {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::OneHour => "1h",
            Self::FourHour => "4h",
            Self::OneDay => "1d",
        }
    }
}

impl FromStr for SignalTimeframe {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tick" | "poll" | "quote" => Ok(Self::Tick),
            "1h" | "1hour" | "1-hour" | "hour" | "hourly" => Ok(Self::OneHour),
            "4h" | "4hour" | "4-hour" | "four_hour" | "4hourly" => Ok(Self::FourHour),
            "1d" | "1day" | "1-day" | "day" | "daily" => Ok(Self::OneDay),
            other => Err(anyhow!(
                "unsupported signal_timeframe '{other}', expected tick, 1h, 4h, or 1d"
            )),
        }
    }
}

impl<'de> Deserialize<'de> for SignalTimeframe {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::from_str(&raw).map_err(|_| {
            D::Error::invalid_value(
                Unexpected::Str(&raw),
                &"tick, 1h, 4h, or 1d signal timeframe",
            )
        })
    }
}

impl Serialize for SignalTimeframe {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Display for SignalTimeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EngineConfig {
    pub mode: String,
    pub loop_ms: u64,
    pub warn_loop_ms: u64,
    #[serde(default)]
    pub signal_bar_source: Option<String>,
    #[serde(default)]
    pub signal_bar_feed: Option<String>,
    #[serde(default)]
    pub signal_bar_poll_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlpacaConfig {
    pub paper: bool,
    pub trading_base_url: String,
    pub data_base_url: String,
    pub api_key_env: String,
    pub secret_key_env: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    pub enabled: bool,
    pub sqlite_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StrategyConfig {
    pub rolling_window_ticks: usize,
    pub entry_zscore: f64,
    pub exit_zscore: f64,
    pub min_samples: usize,
    pub max_spread_bps: f64,
    #[serde(default)]
    pub use_rolling_beta: Option<bool>,
    #[serde(default)]
    pub beta_min: Option<f64>,
    #[serde(default)]
    pub beta_max: Option<f64>,
    #[serde(default)]
    pub min_correlation: Option<f64>,
    #[serde(default)]
    pub max_spread_std_bps: Option<f64>,
    #[serde(default)]
    pub entry_confirmation_bars: Option<u64>,
    #[serde(default)]
    pub min_expected_spread_move_after_costs: Option<f64>,
    #[serde(default)]
    pub adverse_zscore: Option<f64>,
    #[serde(default)]
    pub stop_loss_dollars: Option<f64>,
    #[serde(default)]
    pub profit_protection_min_dollars: Option<f64>,
    #[serde(default)]
    pub profit_protection_retrace_fraction: Option<f64>,
    #[serde(default)]
    pub profit_protection_floor_dollars: Option<f64>,
    #[allow(dead_code)]
    pub min_half_life_ticks: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionConfig {
    pub passive_entry: bool,
    pub stale_order_seconds: u64,
    pub max_leg_delay_seconds: u64,
    pub use_aggressive_rescue: bool,
    pub client_order_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskConfig {
    pub initial_equity: f64,
    pub max_pair_notional: f64,
    pub max_total_notional: f64,
    pub max_daily_loss: f64,
    pub max_open_pairs: usize,
    pub max_holding_minutes: u64,
    pub require_easy_to_borrow_for_short: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PairConfig {
    pub id: String,
    pub a: String,
    pub b: String,
    pub enabled: bool,
    pub beta: Option<f64>,
    pub leg_notional: f64,
    #[serde(default)]
    pub rolling_window_ticks: Option<usize>,
    #[serde(default)]
    pub entry_zscore: Option<f64>,
    #[serde(default)]
    pub exit_zscore: Option<f64>,
    #[serde(default)]
    pub min_samples: Option<usize>,
    #[serde(default)]
    pub max_spread_bps: Option<f64>,
    #[serde(default)]
    pub use_rolling_beta: Option<bool>,
    #[serde(default)]
    pub beta_min: Option<f64>,
    #[serde(default)]
    pub beta_max: Option<f64>,
    #[serde(default)]
    pub min_correlation: Option<f64>,
    #[serde(default)]
    pub max_spread_std_bps: Option<f64>,
    #[serde(default)]
    pub entry_confirmation_bars: Option<u64>,
    #[serde(default)]
    pub min_expected_spread_move_after_costs: Option<f64>,
    #[serde(default)]
    pub adverse_zscore: Option<f64>,
    #[serde(default)]
    pub stop_loss_dollars: Option<f64>,
    #[serde(default)]
    pub profit_protection_min_dollars: Option<f64>,
    #[serde(default)]
    pub profit_protection_retrace_fraction: Option<f64>,
    #[serde(default)]
    pub profit_protection_floor_dollars: Option<f64>,
    #[allow(dead_code)]
    #[serde(default)]
    pub min_half_life_ticks: Option<usize>,
    #[serde(default)]
    pub max_holding_bars: Option<u64>,
    #[serde(default)]
    pub signal_timeframe: Option<SignalTimeframe>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optimized_mix_signal_timeframes() {
        let cfg = AppConfig::from_file("config/optimized_mix.toml").unwrap();
        let timeframes = cfg.signal_timeframes();
        assert!(timeframes.contains(&SignalTimeframe::OneHour));
        assert!(timeframes.contains(&SignalTimeframe::FourHour));
        assert!(timeframes.contains(&SignalTimeframe::OneDay));
        assert!(!timeframes.contains(&SignalTimeframe::Tick));
    }

    #[test]
    fn serializes_timeframe_as_config_string() {
        assert_eq!(
            serde_json::to_string(&SignalTimeframe::OneHour).unwrap(),
            "\"1h\""
        );
    }
}
