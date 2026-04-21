use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::Value;

use crate::types::{MarketMeta, Orderbook, OrderLevel, TokenBook};

const GAMMA: &str = "https://gamma-api.polymarket.com";
const CLOB:  &str = "https://clob.polymarket.com";

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn current_bucket_ts(interval_secs: u64) -> i64 {
    let now = now_unix();
    (now / interval_secs as i64) * interval_secs as i64
}

fn parse_maybe_array(v: &Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        return arr.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect();
    }
    if let Some(s) = v.as_str() {
        if let Ok(parsed) = serde_json::from_str::<Vec<String>>(s) {
            return parsed;
        }
    }
    vec![]
}

/// Try to parse beat price from market question/description text.
/// Looks for patterns like "$83,450" or "$83450.50"
pub fn parse_beat_price_from_text(text: &str) -> Option<f64> {
    // Find dollar amounts with optional commas and decimals
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b',' || bytes[end] == b'.') {
                end += 1;
            }
            if end > start {
                let num_str: String = text[start..end].chars().filter(|c| *c != ',').collect();
                if let Ok(price) = num_str.parse::<f64>() {
                    if price > 1000.0 && price < 10_000_000.0 {
                        return Some(price);
                    }
                }
            }
        }
        i += 1;
    }
    None
}

// ── Market resolution ─────────────────────────────────────────────────────────

/// Resolve the current active market for a given slug prefix and interval.
/// Tries current bucket, then falls back to the previous bucket.
pub async fn resolve_market(
    client: &Client,
    slug_prefix: &str,
    interval_secs: u64,
) -> Result<MarketMeta> {
    for back in 0..=1i64 {
        let ts   = current_bucket_ts(interval_secs) - back * interval_secs as i64;
        let slug = format!("{}-{}", slug_prefix, ts);
        let url  = format!("{}/markets?slug={}", GAMMA, url_encode(&slug));

        let resp: Value = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let items = match resp.as_array() {
            Some(arr) if !arr.is_empty() => arr,
            _ => continue,
        };

        let raw      = &items[0];
        let outcomes = parse_maybe_array(&raw["outcomes"]);
        let ids      = parse_maybe_array(&raw["clobTokenIds"]);

        let up_idx = outcomes.iter().position(|o| o.to_lowercase().contains("up")).unwrap_or(0);
        let dn_idx = outcomes.iter().position(|o| o.to_lowercase().contains("down")).unwrap_or(1);

        // Parse end_time from endDate field
        let end_time = raw["endDate"].as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(ts + interval_secs as i64);

        // eventStartTime: the exact moment the price range opens (beat price moment)
        // Top-level eventStartTime is canonical; events[0].startTime is a fallback
        let event_start_time = raw["eventStartTime"].as_str()
            .or_else(|| {
                raw["events"].as_array()
                    .and_then(|evts| evts.first())
                    .and_then(|e| e["startTime"].as_str())
            })
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
            .unwrap_or(ts); // fall back to bucket timestamp

        // Try to extract beat price from question/description text
        let question = raw["question"]
            .as_str()
            .or_else(|| raw["description"].as_str())
            .unwrap_or("")
            .to_string();
        let beat_price = parse_beat_price_from_text(&question)
            .or_else(|| {
                let desc = raw["description"].as_str().unwrap_or("");
                parse_beat_price_from_text(desc)
            });

        return Ok(MarketMeta {
            slug,
            end_time,
            event_start_time,
            up_token_id:   ids.get(up_idx).cloned().unwrap_or_default(),
            down_token_id: ids.get(dn_idx).cloned().unwrap_or_default(),
            beat_price,
            question,
        });
    }
    Err(anyhow!("Cannot find current {} market", slug_prefix))
}

// ── Orderbook fetch ───────────────────────────────────────────────────────────

/// Fetch full orderbook for a single token.
async fn fetch_token_book(client: &Client, token_id: &str) -> Result<TokenBook> {
    if token_id.is_empty() {
        return Ok(TokenBook::default());
    }
    let url = format!("{}/book?token_id={}", CLOB, url_encode(token_id));
    let resp: Value = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let asks = parse_levels(&resp["asks"]);
    let bids = parse_levels(&resp["bids"]);

    let mut book = TokenBook { asks, bids };
    // Sort asks ascending (cheapest first), bids descending (highest first)
    book.asks.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap_or(std::cmp::Ordering::Equal));
    book.bids.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap_or(std::cmp::Ordering::Equal));
    Ok(book)
}

fn parse_levels(arr: &Value) -> Vec<OrderLevel> {
    let Some(items) = arr.as_array() else { return vec![]; };
    items.iter().filter_map(|item| {
        let price = item["price"].as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| item["price"].as_f64())?;
        let size = item["size"].as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| item["size"].as_f64())?;
        if price <= 0.0 || size <= 0.0 { return None; }
        Some(OrderLevel { price, size })
    }).collect()
}

/// Fetch orderbooks for both UP and DOWN tokens in parallel.
pub async fn fetch_orderbook(client: &Client, market: &MarketMeta) -> Result<Orderbook> {
    let (up, down) = tokio::try_join!(
        fetch_token_book(client, &market.up_token_id),
        fetch_token_book(client, &market.down_token_id),
    )?;
    Ok(Orderbook { up, down, fetched_at: now_unix() })
}

// ── Winner confirmation from Polymarket ──────────────────────────────────────

/// Fetch actual on-chain resolution from Polymarket Gamma API.
/// Returns "up", "down", or None if not yet resolved.
pub async fn fetch_market_resolution(client: &Client, slug: &str) -> Result<Option<String>> {
    let url = format!("{}/markets?slug={}", GAMMA, url_encode(slug));
    let resp: Value = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let items = match resp.as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    let raw = &items[0];

    // Check if market is resolved
    let resolved = raw["resolved"].as_bool().unwrap_or(false);
    if !resolved {
        return Ok(None);
    }

    // Find winning outcome
    let outcomes = parse_maybe_array(&raw["outcomes"]);
    let resolutions = raw["outcomePrices"]
        .as_str()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .or_else(|| raw["outcomePrices"].as_array().map(|arr| {
            arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()
        }));

    if let Some(prices) = resolutions {
        for (i, price_str) in prices.iter().enumerate() {
            let p: f64 = price_str.parse().unwrap_or(0.0);
            if p >= 0.99 {
                if let Some(outcome) = outcomes.get(i) {
                    let lower = outcome.to_lowercase();
                    if lower.contains("up") { return Ok(Some("up".to_string())); }
                    if lower.contains("down") { return Ok(Some("down".to_string())); }
                }
            }
        }
    }

    Ok(None)
}

// ── URL encoding ─────────────────────────────────────────────────────────────

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => { out.push('%'); out.push_str(&format!("{:02X}", b)); }
        }
    }
    out
}
