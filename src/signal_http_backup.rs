use std::time::Duration;
use reqwest::Client;
use tracing::{info, warn, error};
use tokio::sync::mpsc::Sender;

use crate::models_signal::Signal;

pub async fn start_http_backup(url: &str, health_url: &str, tx: Sender<Signal>) {
    let client = Client::new();
    let mut last_timestamp = 0;
    
    info!("Starting HTTP Backup polling to {}", url);
    
    loop {
        // Health check
        match client.get(health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                // Polling for missing signals if needed, or just normal polling
                let query_url = format!("{}?from={}", url, last_timestamp);
                match client.get(&query_url).send().await {
                    Ok(resp) => {
                        if let Ok(signals) = resp.json::<Vec<Signal>>().await {
                            for signal in signals {
                                if signal.timestamp > last_timestamp {
                                    last_timestamp = signal.timestamp;
                                    let _ = tx.send(signal).await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("HTTP Backup fetch failed: {}", e);
                    }
                }
            }
            _ => {
                error!("HTTP Health check failed! Consecutive failures should pause trading.");
            }
        }
        
        // Polling every 2 seconds
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
