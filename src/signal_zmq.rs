#![allow(dead_code)]

use std::time::Duration;
use zeromq::{Socket, SocketRecv, SubSocket};
use tracing::{info, error};
use tokio::sync::mpsc::Sender;

use crate::models_signal::Signal;

/// Try to connect ZMQ (quickly). Returns Ok(()) if connected successfully.
/// This is kept separate so main.rs can wrap it in a timeout.
pub async fn try_connect_zmq(address: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut socket = SubSocket::new();
    info!("Connecting ZMQ to {}...", address);
    socket.connect(address).await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    socket.subscribe("").await.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
    info!("ZMQ probe connect OK");
    Ok(())
}

pub async fn start_zmq_subscriber(address: &str, tx: Sender<Signal>) {
    let mut socket = SubSocket::new();
    
    info!("Connecting ZMQ to {}...", address);
    if let Err(e) = socket.connect(address).await {
        error!("Failed to connect ZMQ to {}: {}", address, e);
        return;
    }
    
    if let Err(e) = socket.subscribe("").await {
        error!("Failed to subscribe ZMQ: {}", e);
        return;
    }
    
    info!("ZMQ Subscriber connected to {}", address);


    loop {
        // Use timeout to prevent blocking forever, which makes shutdown cleaner
        // and doesn't spam logs on timeout.
        match tokio::time::timeout(Duration::from_millis(1000), socket.recv()).await {
            Ok(Ok(message)) => {
                if let Some(data) = message.get(0) {
                    let text = String::from_utf8_lossy(data);
                    if let Some(signal) = parse_signal(&text) {
                        info!("📥 Received Signal: {}, prob={}", signal.symbol, signal.prob);
                        let _ = tx.send(signal).await;
                    }
                }
            }
            Ok(Err(e)) => {
                error!("ZMQ receive error: {}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(_) => {
                // Timeout, just continue
                continue;
            }
        }
    }
}

fn parse_signal(text: &str) -> Option<Signal> {
    let parts: Vec<&str> = text.split('|').collect();
    if parts.len() < 8 {
        return None;
    }
    
    let msg_id = parts[0].to_string();
    let timestamp = parts[1].parse().ok()?;
    let symbol = parts[2].to_string();
    let price = parts[3].parse().ok()?;
    let atr_24h = parts[4].parse().ok()?;
    let is_long = parts[5] == "true" || parts[5] == "1";
    let raw_score = parts[6].parse().ok()?;
    let prob = parts[7].parse().ok()?;
    
    let mut tier = None;
    let mut is_new_symbol = None;
    let regime;
    
    // New payload: msg_id|timestamp|symbol|price|atr_24h|is_long|raw_score|prob|features|tier|is_new_symbol|regime
    if parts.len() >= 11 {
        tier = Some(parts[9].to_string());
        is_new_symbol = Some(parts[10] == "true" || parts[10] == "1");
    }
    
    if parts.len() >= 12 {
        regime = Some(parts[11].to_string());
    } else {
        regime = Some("CHOP_HIGH_VOL".to_string()); // Default to most defensive mode if missing
    }
    
    Some(Signal {
        msg_id,
        timestamp,
        symbol,
        price,
        atr_24h,
        is_long,
        raw_score,
        prob,
        tier,
        is_new_symbol,
        regime,
    })
}
