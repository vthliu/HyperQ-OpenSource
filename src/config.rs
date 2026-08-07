#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub dry_run: bool,
    pub binance: BinanceConfig,
    pub signal: SignalConfig,
    pub risk_guard: RiskGuardConfig,
    pub sentinel: SentinelConfig,
    pub position: PositionConfig,
    pub executor: ExecutorConfig,
    pub asset_tiers: AssetTiersConfig,
    #[serde(default = "default_defense")]
    pub defense_mode: DefenseConfig,
    pub metrics: MetricsConfig,
    pub logging: LoggingConfig,
    pub time_stop: TimeStopConfig,
    pub prob_threshold: ProbThresholdConfig,
}

fn default_defense() -> DefenseConfig {
    DefenseConfig {
        enabled: true,
        leverage_multiplier: 0.5,
        hard_stop_multiplier: 0.8,
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DefenseConfig {
    pub enabled: bool,
    pub leverage_multiplier: f64,
    pub hard_stop_multiplier: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BinanceConfig {
    pub env: String,
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SignalConfig {
    pub zmq_address: String,
    pub http_backup_url: String,
    pub health_check_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskGuardConfig {
    pub zscore_multiplier: f64,
    pub vwap_recover_threshold: f64,
    pub alert_grace_cycles: u32,
    pub layer1: RiskLayerConfig,
    pub layer2: RiskLayerConfig,
    pub layer3: RiskLayerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskLayerConfig {
    pub accel_base: f64,
    pub vel_base: f64,
    #[serde(default)]
    pub zscore_multiplier: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SentinelConfig {
    pub check_interval_ms: u64,
    pub stale_data_timeout_ms: u64,
    pub closing_timeout_ms: u64,
    pub funding_rate_exit_threshold: f64,
    pub funding_rate_open_threshold: f64,
    pub min_signal_prob_for_high_funding: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TimeStopConfig {
    pub layer1_max_holding_hours: f64,
    pub layer2_max_holding_hours: f64,
    pub layer3_max_holding_hours: f64,
    pub profit_threshold: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PositionConfig {
    pub max_positions: usize,
    pub max_leverage: u8,
    pub rwa_risk_multiplier: f64,
    #[serde(default = "default_dynamic_sizing")]
    pub dynamic_sizing: DynamicSizingConfig,
}

fn default_dynamic_sizing() -> DynamicSizingConfig {
    DynamicSizingConfig {
        enabled: false,
        min_alloc_pct: 0.20,
        max_alloc_pct: 0.20,
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DynamicSizingConfig {
    pub enabled: bool,
    pub min_alloc_pct: f64,
    pub max_alloc_pct: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExecutorConfig {
    pub market_order_slippage_tolerance: f64,
    pub max_market_order_retries: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AssetTiersConfig {
    pub layer1: Vec<String>,
    pub layer2: Vec<String>,
    #[serde(default)]
    pub layer3: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MetricsConfig {
    pub prometheus_port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProbThresholdConfig {
    pub layer1: f64,
    pub layer2: f64,
    pub layer3: f64,
}

impl AppConfig {
    pub fn load() -> Result<Self, config::ConfigError> {
        let builder = config::Config::builder()
            .add_source(config::File::with_name("config.toml"));
            
        let mut config = builder.build()?.try_deserialize::<Self>()?;
        
        if let Ok(key) = std::env::var("BINANCE_API_KEY") {
            config.binance.api_key = key;
        }
        if let Ok(secret) = std::env::var("BINANCE_API_SECRET") {
            config.binance.api_secret = secret;
        }
        
        Ok(config)
    }
}
