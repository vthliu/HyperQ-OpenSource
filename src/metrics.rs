use lazy_static::lazy_static;
use prometheus::{IntCounterVec, IntGauge, register_int_counter_vec, register_int_gauge};
use warp::Filter;
use tracing::info;

lazy_static! {
    pub static ref SENTINEL_TRIGGER_COUNT: IntCounterVec = 
        register_int_counter_vec!("sentinel_triggers_total", "Trigger count by type", &["type"]).unwrap();

    pub static ref POSITION_COUNT: IntGauge = 
        register_int_gauge!("positions_total", "Active positions count").unwrap();

    pub static ref API_RATE_LIMIT_REMAINING: IntGauge = 
        register_int_gauge!("api_rate_limit_remaining", "Remaining API tokens").unwrap();
}

pub async fn start_metrics_server(port: u16) {
    info!("Starting Prometheus metrics server on 127.0.0.1:{}", port);
    
    let metrics_route = warp::path!("metrics").map(|| {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let mut buffer = vec![];
        let metric_families = prometheus::gather();
        encoder.encode(&metric_families, &mut buffer).unwrap_or_default();
        String::from_utf8(buffer).unwrap_or_default()
    });

    warp::serve(metrics_route).run(([127, 0, 0, 1], port)).await;
}
