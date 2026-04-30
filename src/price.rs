/// Polymarket RTDS WebSocket price stream.
/// Subscribes to crypto_prices_chainlink and stores each Chainlink round with
/// its on-chain timestamp so callers can look up "price in effect at time T".
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_URL: &str = "wss://ws-live-data.polymarket.com";

// ── Price history ─────────────────────────────────────────────────────────────

/// One Chainlink round answer received from RTDS.
#[derive(Clone, Copy, Debug)]
pub struct PricePoint {
    pub price: f64,
    pub chainlink_ts: i64,  // Chainlink round timestamp, normalised to seconds
}

/// Sliding window of recent Chainlink price points in chronological order.
/// `price_at(ts)` gives the price that was *in effect* at second `ts`:
/// the most recent round where chainlink_ts ≤ ts.
pub struct PriceHistory {
    points: Vec<PricePoint>,
}

impl PriceHistory {
    fn new() -> Self { Self { points: Vec::new() } }

    fn push(&mut self, p: PricePoint) {
        // Skip duplicate timestamps
        if let Some(last) = self.points.last() {
            if last.chainlink_ts == p.chainlink_ts { return; }
        }
        self.points.push(p);
        if self.points.len() > 60 { self.points.remove(0); }
    }

    pub fn current_price(&self) -> f64 {
        self.points.last().map(|p| p.price).unwrap_or(0.0)
    }

    /// Exact Chainlink round at unix second `ts`.
    /// Returns Some only when a round with chainlink_ts == ts exists.
    pub fn price_exact(&self, ts: i64) -> Option<f64> {
        self.points.iter()
            .find(|p| p.chainlink_ts == ts)
            .map(|p| p.price)
    }

    /// Price in effect at unix second `ts` (fallback: last round ≤ ts).
    pub fn price_at(&self, ts: i64) -> Option<f64> {
        self.points.iter()
            .filter(|p| p.chainlink_ts <= ts)
            .last()
            .map(|p| p.price)
    }
}

pub type SharedPrice = Arc<Mutex<PriceHistory>>;

// ── Public API ────────────────────────────────────────────────────────────────

pub fn start_price_stream(_client: Client, asset: &str) -> SharedPrice {
    let symbol = rtds_symbol(asset).to_string();
    let shared = Arc::new(Mutex::new(PriceHistory::new()));
    tokio::spawn(ws_loop(symbol, Arc::clone(&shared)));
    shared
}

/// No REST fallback — engine handles the Err case (starts with price 0.0).
pub async fn fetch_price_rest(_client: &Client, _asset: &str) -> Result<f64> {
    Err(anyhow::anyhow!("price comes from Polymarket RTDS WebSocket"))
}

// ── WebSocket loop ────────────────────────────────────────────────────────────

async fn ws_loop(symbol: String, price: SharedPrice) {
    loop {
        if let Err(e) = ws_connect(&symbol, &price).await {
            eprintln!("[RTDS] disconnected: {e} — reconnecting in 2s");
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn ws_connect(symbol: &str, shared: &SharedPrice) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(WS_URL).await?;
    let (mut write, mut read) = ws_stream.split();

    // filters must be a JSON-encoded string (mirrors the JS reference impl)
    let filters_str = serde_json::to_string(&json!({ "symbol": symbol }))?;
    let sub = json!({
        "action": "subscribe",
        "subscriptions": [{
            "topic": "crypto_prices_chainlink",
            "type": "*",
            "filters": filters_str
        }]
    });
    write.send(Message::Text(sub.to_string())).await?;

    while let Some(msg) = read.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(d) => { write.send(Message::Pong(d)).await?; continue; }
            Message::Close(_) => break,
            _ => continue,
        };

        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        if v["type"] != "update" { continue; }

        let price = v["payload"]["value"].as_f64()
            .or_else(|| v["payload"]["value"].as_str()
                .and_then(|s| s.parse::<f64>().ok()));

        // Chainlink timestamps may be in seconds or milliseconds — normalise to seconds
        let ts_raw = v["payload"]["timestamp"].as_i64().unwrap_or(0);
        let chainlink_ts = if ts_raw > 1_000_000_000_000 { ts_raw / 1000 } else { ts_raw };
        // Fall back to system clock if RTDS provides no timestamp
        let chainlink_ts = if chainlink_ts > 0 { chainlink_ts } else { now_secs() };

        if let Some(p) = price {
            if p > 0.0 {
                shared.lock().await.push(PricePoint { price: p, chainlink_ts });
            }
        }
    }
    Ok(())
}

fn rtds_symbol(asset: &str) -> &'static str {
    match asset {
        "ETH"  => "eth/usd",
        "SOL"  => "sol/usd",
        "BNB"  => "bnb/usd",
        "XRP"  => "xrp/usd",
        "DOGE" => "doge/usd",
        "HYPE" => "hype/usd",
        _      => "btc/usd",   // BTC + fallback
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
