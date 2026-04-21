use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;

// ── Market metadata ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MarketMeta {
    pub slug: String,
    pub end_time: i64,             // unix timestamp when market closes
    pub event_start_time: i64,     // unix timestamp when the price range starts (beat price moment)
    pub up_token_id: String,
    pub down_token_id: String,
    pub beat_price: Option<f64>,   // parsed from question text (rarely present)
    pub question: String,
}

impl Default for MarketMeta {
    fn default() -> Self {
        MarketMeta {
            slug: String::new(),
            end_time: 0,
            event_start_time: 0,
            up_token_id: String::new(),
            down_token_id: String::new(),
            beat_price: None,
            question: String::new(),
        }
    }
}

impl MarketMeta {
    pub fn is_empty(&self) -> bool {
        self.slug.is_empty()
    }

    pub fn seconds_until_end(&self) -> i64 {
        self.end_time - chrono::Utc::now().timestamp()
    }

    pub fn has_ended(&self) -> bool {
        self.end_time > 0 && chrono::Utc::now().timestamp() >= self.end_time
    }
}

// ── Price candles (1-minute OHLC based on Chainlink BTC price) ────────────────

#[derive(Debug, Clone)]
pub struct Candle {
    pub start_ts: i64, // unix ts of candle start (1 minute boundary)
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub ticks: u32,
}

impl Candle {
    pub fn new(ts: i64, price: f64) -> Self {
        Self { start_ts: ts, open: price, high: price, low: price, close: price, ticks: 1 }
    }

    pub fn update(&mut self, price: f64) {
        if price > self.high { self.high = price; }
        if price < self.low  { self.low  = price; }
        self.close = price;
        self.ticks += 1;
    }

    pub fn direction(&self) -> &'static str {
        if self.close >= self.open { "↑" } else { "↓" }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CandleHistory {
    pub candles: VecDeque<Candle>,
    pub max_candles: usize,
    pub current_candle: Option<Candle>,
}

impl CandleHistory {
    pub fn new(max_candles: usize) -> Self {
        Self { candles: VecDeque::new(), max_candles, current_candle: None }
    }

    pub fn update(&mut self, price: f64) {
        let now_ts = chrono::Utc::now().timestamp();
        let candle_ts = (now_ts / 60) * 60; // 1-minute boundary

        match &mut self.current_candle {
            Some(c) if c.start_ts == candle_ts => {
                c.update(price);
            }
            _ => {
                // Flush finished candle to history
                if let Some(old) = self.current_candle.take() {
                    self.candles.push_back(old);
                    while self.candles.len() > self.max_candles {
                        self.candles.pop_front();
                    }
                }
                self.current_candle = Some(Candle::new(candle_ts, price));
            }
        }
    }

    /// Returns up to `max_candles` completed candles (oldest first)
    pub fn last_completed(&self) -> Vec<&Candle> {
        self.candles.iter().collect()
    }
}

// ── Orderbook ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct OrderLevel {
    pub price: f64,
    pub size: f64,
}

impl OrderLevel {
    pub fn value(&self) -> f64 {
        self.price * self.size
    }
}

#[derive(Debug, Clone, Default)]
pub struct TokenBook {
    pub asks: Vec<OrderLevel>, // sorted lowest price first
    pub bids: Vec<OrderLevel>, // sorted highest price first
}

impl TokenBook {
    pub fn best_ask(&self) -> Option<f64> {
        self.asks.first().map(|l| l.price)
    }
    pub fn total_ask_usdc(&self) -> f64 {
        self.asks.iter().map(|l| l.value()).sum()
    }
    pub fn total_ask_shares(&self) -> f64 {
        self.asks.iter().map(|l| l.size).sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Orderbook {
    pub up: TokenBook,
    pub down: TokenBook,
    pub fetched_at: i64,
}

// ── Trade records ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct BotTrade {
    pub ts: DateTime<Utc>,
    pub market_slug: String,
    pub outcome: String,      // "up" or "down"
    pub shares: f64,
    pub usdc_spent: f64,
    pub fill_price: f64,
    pub order_id: String,
    pub is_live: bool,
}

// PnL for a completed market
#[derive(Debug, Clone, Serialize)]
pub struct MarketPnl {
    pub slug: String,
    pub beat_price: f64,
    pub end_price: f64,
    pub winner: String,   // "up" or "down"
    pub our_outcome: String,
    pub shares_bought: f64,
    pub usdc_spent: f64,
    pub pnl: f64,         // shares * 1.0 - usdc_spent  (winner) or -usdc_spent (loser)
    pub resolved: bool,   // whether Polymarket confirmed winner
}

// ── Execution types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BuyParams {
    pub token_id: String,
    pub outcome: String,
    pub shares: f64,
    pub ask_price: f64,
    pub slippage_buffer: f64,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub fill_price: f64,
    pub shares: f64,
    pub usdc: f64,
    pub order_id: String,
    pub notes: String,
}

// ── Market lifecycle phase ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum MarketPhase {
    /// Market is active; time remaining > 0
    Active,
    /// Market just ended; trade winner asks below $1.00 for up to post_market_secs
    JustEnded {
        ended_at: i64,
        winner: String,        // "up" or "down"
        end_btc_price: f64,
    },
    /// Transitioning to next market
    Transitioning,
}

impl Default for MarketPhase {
    fn default() -> Self { MarketPhase::Active }
}
