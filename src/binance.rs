/// Binance public API price stream for BTC/ETH.
/// Polls the REST ticker endpoint every 250 ms and exposes a shared f64 price.
/// No API key required — uses the public endpoint.
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

pub type SharedPrice = Arc<Mutex<f64>>;

/// Spawn a background task that keeps `SharedPrice` updated via Binance REST.
/// Returns the shared price immediately (starts at 0.0 until the first poll).
pub fn start_price_stream(client: Client, asset: &str) -> SharedPrice {
    let symbol = binance_symbol(asset);
    let shared = Arc::new(Mutex::new(0.0f64));
    let shared_clone = Arc::clone(&shared);
    tokio::spawn(poll_loop(client, symbol.to_string(), shared_clone));
    shared
}

async fn poll_loop(client: Client, symbol: String, price: SharedPrice) {
    loop {
        match fetch_rest_inner(&client, &symbol).await {
            Ok(p) => *price.lock().await = p,
            Err(e) => eprintln!("[Binance] poll error: {e}"),
        }
        sleep(Duration::from_millis(250)).await;
    }
}

/// One-shot REST fetch — used for the initial price before the poll loop catches up.
pub async fn fetch_price_rest(client: &Client, asset: &str) -> Result<f64> {
    fetch_rest_inner(client, binance_symbol(asset)).await
}

async fn fetch_rest_inner(client: &Client, symbol: &str) -> Result<f64> {
    let url = format!(
        "https://api.binance.com/api/v3/ticker/price?symbol={}",
        symbol
    );
    let resp: Value = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    resp["price"]
        .as_str()
        .ok_or_else(|| anyhow!("no price field in Binance response"))?
        .parse::<f64>()
        .map_err(|e| anyhow!("Binance price parse: {e}"))
}

fn binance_symbol(asset: &str) -> &'static str {
    if asset == "ETH" { "ETHUSDT" } else { "BTCUSDT" }
}
