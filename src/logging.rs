use tracing_subscriber::{EnvFilter};

pub fn init_logging(level: &str, format: &str) {
    let filter = EnvFilter::new(level);
    
    match format {
        "json" => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
        _ => {
            // Use Beijing local time (requires system timezone to be set to Asia/Shanghai)
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_thread_ids(true)
                .without_time()
                .init();
        }
    }
    tracing::info!("Logging initialized (level: {}, format: {})", level, format);
}
