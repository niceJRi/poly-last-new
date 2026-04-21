/// Chainlink Data Streams REST client for BTC/USD and ETH/USD prices.
/// Falls back to on-chain aggregator (latestRoundData) if no DS credentials.
///
/// Authentication: HMAC-SHA256 per https://docs.chain.link/data-streams/reference/data-streams-api/authentication
/// Price encoding: int192 scaled by 10^18 (18 decimal places)
use anyhow::{anyhow, Result};
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};

const DS_BASE: &str = "https://api.dataengine.chain.link";
const LATEST_ROUND_DATA_SELECTOR: &str = "0xfeaf968c";

const FALLBACK_RPCS: &[&str] = &[
    "https://polygon-rpc.com",
    "https://rpc.ankr.com/polygon",
    "https://rpc-mainnet.maticvigil.com",
];

// ── Public price client ───────────────────────────────────────────────────────

pub struct PriceClient {
    http:          Client,
    ds_api_key:    Option<String>,
    ds_api_secret: Option<String>,
    ds_feed_id:    String,
    rpc_url:       String,
    feed_address:  String,
}

impl PriceClient {
    pub fn new(
        http: Client,
        ds_api_key: Option<String>,
        ds_api_secret: Option<String>,
        ds_feed_id: String,
        rpc_url: String,
        feed_address: String,
    ) -> Self {
        Self { http, ds_api_key, ds_api_secret, ds_feed_id, rpc_url, feed_address }
    }

    /// Latest real-time price. Tries Data Streams first, then on-chain aggregator.
    pub async fn latest_price(&self) -> Result<f64> {
        if let (Some(key), Some(secret)) = (&self.ds_api_key, &self.ds_api_secret) {
            match ds_latest(&self.http, key, secret, &self.ds_feed_id).await {
                Ok(p) => return Ok(p),
                Err(_) => {} // silently fall through to aggregator
            }
        }
        self.aggregator_price().await
    }

    /// Price at a specific unix timestamp (seconds). Data Streams only.
    /// Used to fetch the exact beat price at market startTime.
    pub async fn price_at(&self, unix_ts: i64) -> Result<f64> {
        let (key, secret) = match (&self.ds_api_key, &self.ds_api_secret) {
            (Some(k), Some(s)) => (k.as_str(), s.as_str()),
            _ => return Err(anyhow!("DS credentials required for historical price")),
        };
        ds_at_timestamp(&self.http, key, secret, &self.ds_feed_id, unix_ts).await
    }

    pub fn has_data_streams(&self) -> bool {
        self.ds_api_key.is_some() && self.ds_api_secret.is_some()
    }

    // ── On-chain aggregator fallback ──────────────────────────────────────────

    async fn aggregator_price(&self) -> Result<f64> {
        if let Ok(p) = self.call_rpc(&self.rpc_url).await {
            return Ok(p);
        }
        for rpc in FALLBACK_RPCS {
            if *rpc == self.rpc_url { continue; }
            if let Ok(p) = self.call_rpc(rpc).await {
                return Ok(p);
            }
        }
        Err(anyhow!("All Polygon RPCs failed for Chainlink aggregator"))
    }

    async fn call_rpc(&self, rpc_url: &str) -> Result<f64> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method":  "eth_call",
            "params":  [{ "to": self.feed_address, "data": LATEST_ROUND_DATA_SELECTOR }, "latest"],
            "id": 1
        });
        let resp: Value = self.http
            .post(rpc_url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        if let Some(e) = resp.get("error") {
            return Err(anyhow!("RPC error: {e}"));
        }

        let hex = resp["result"]
            .as_str()
            .ok_or_else(|| anyhow!("missing result"))?
            .trim_start_matches("0x");

        if hex.len() < 128 {
            return Err(anyhow!("RPC response too short"));
        }
        // Word 1 (bytes 32-63) = answer (int256), BTC/USD has 8 decimals
        let answer = u128::from_str_radix(&hex[64..128], 16)
            .map_err(|e| anyhow!("hex parse: {e}"))?;
        if answer == 0 {
            return Err(anyhow!("zero price from aggregator"));
        }
        Ok(answer as f64 / 1e8)
    }
}

// ── Data Streams helpers ──────────────────────────────────────────────────────

async fn ds_latest(http: &Client, key: &str, secret: &str, feed_id: &str) -> Result<f64> {
    let path = format!("/api/v1/reports/latest?feedID={}", feed_id);
    let resp = ds_get(http, key, secret, &path).await?;
    parse_ds_report(&resp)
}

async fn ds_at_timestamp(
    http: &Client,
    key: &str,
    secret: &str,
    feed_id: &str,
    unix_ts: i64,
) -> Result<f64> {
    let path = format!("/api/v1/reports?feedID={}&timestamp={}", feed_id, unix_ts);
    let resp = ds_get(http, key, secret, &path).await?;
    parse_ds_report(&resp)
}

async fn ds_get(http: &Client, key: &str, secret: &str, path: &str) -> Result<Value> {
    let ts_ms = Utc::now().timestamp_millis().to_string();
    let body_hash = sha256_hex(b"");
    let string_to_sign = format!("GET {} {} {} {}", path, body_hash, key, ts_ms);
    let sig = hmac_sha256_hex(secret.as_bytes(), string_to_sign.as_bytes());

    let url = format!("{}{}", DS_BASE, path);
    let resp: Value = http
        .get(&url)
        .header("Authorization", key)
        .header("X-Authorization-Timestamp", &ts_ms)
        .header("X-Authorization-Signature-SHA256", &sig)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(resp)
}

fn parse_ds_report(resp: &Value) -> Result<f64> {
    // Report price is hex-encoded int192 scaled by 10^18
    let price_hex = resp["report"]["price"]
        .as_str()
        .ok_or_else(|| anyhow!("missing report.price in DS response: {}", resp))?
        .trim_start_matches("0x");

    parse_ds_price(price_hex)
}

fn parse_ds_price(hex: &str) -> Result<f64> {
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        return Err(anyhow!("zero DS price"));
    }
    // int192 = 24 bytes = 48 hex chars; take lower 32 chars (u128)
    let relevant = if trimmed.len() > 32 { &trimmed[trimmed.len() - 32..] } else { trimmed };
    let raw = u128::from_str_radix(relevant, 16)
        .map_err(|e| anyhow!("DS price hex parse: {e}"))?;
    Ok(raw as f64 / 1e18)
}

// ── Crypto helpers ────────────────────────────────────────────────────────────

fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    hash.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC any key length");
    mac.update(data);
    mac.finalize().into_bytes().iter().map(|b| format!("{:02x}", b)).collect()
}

// ── Binance price stream fallback ─────────────────────────────────────────────

pub async fn fetch_price_fallback(client: &Client, asset: &str) -> Result<f64> {
    let symbol = match asset {
        "BTC" => "BTCUSDT",
        "ETH" => "ETHUSDT",
        _ => return Err(anyhow!("unknown asset {}", asset)),
    };
    let url = format!("https://api.binance.com/api/v3/ticker/price?symbol={}", symbol);
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
        .ok_or_else(|| anyhow!("no price in Binance response"))?
        .parse::<f64>()
        .map_err(|e| anyhow!("Binance parse: {e}"))
}

// Keep old name as alias for compatibility
pub type ChainlinkOracle = PriceClient;
