use anyhow::{bail, Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub market: String,          // "btc-5m", "btc-15m", "eth-5m", "eth-15m"
    pub interval_secs: u64,
    pub slug_prefix: String,     // "btc-updown-5m" etc.
    pub asset: String,           // "BTC" or "ETH"
    pub order_usdc: f64,         // USDC to spend per trade (TRADE_AMOUNT in .env)
    pub slippage_buffer: f64,
    pub poll_ms: u64,
    pub post_market_secs: u64,   // seconds to keep post-market window open
    pub max_trades_per_market: usize, // 0 = unlimited (MAX_TRADES in .env)
    // real-mode credentials
    pub private_key: Option<String>,
    pub builder_api_key: Option<String>,
    pub builder_secret: Option<String>,
    pub builder_passphrase: Option<String>,
}

impl Config {
    pub fn load_test() -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::build(false)
    }

    pub fn load_real() -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::build(true)
    }

    fn build(require_credentials: bool) -> Result<Self> {
        let market = env::var("MARKET").unwrap_or_else(|_| "btc-5m".to_string());

        let (secs, slug_prefix, asset) = match market.as_str() {
            "btc-5m"  => (300u64, "btc-updown-5m",  "BTC"),
            "btc-15m" => (900u64, "btc-updown-15m", "BTC"),
            "eth-5m"  => (300u64, "eth-updown-5m",  "ETH"),
            "eth-15m" => (900u64, "eth-updown-15m", "ETH"),
            other => bail!("MARKET must be btc-5m | btc-15m | eth-5m | eth-15m, got '{}'", other),
        };

        // TRADE_AMOUNT is the primary env var; ORDER_USDC accepted for backward compat
        let order_usdc: f64 = env::var("TRADE_AMOUNT")
            .or_else(|_| env::var("ORDER_USDC"))
            .unwrap_or_else(|_| "10.0".to_string())
            .parse()
            .context("TRADE_AMOUNT must be a number")?;

        if order_usdc <= 0.0 {
            bail!("TRADE_AMOUNT must be > 0");
        }

        let slippage_buffer: f64 = env::var("SLIPPAGE_BUFFER")
            .unwrap_or_else(|_| "0.02".to_string())
            .parse()
            .context("SLIPPAGE_BUFFER must be a number")?;

        // MAX_TRADES is the primary env var; MAX_TRADES_PER_MARKET accepted for backward compat
        let max_trades: usize = env::var("MAX_TRADES")
            .or_else(|_| env::var("MAX_TRADES_PER_MARKET"))
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .context("MAX_TRADES must be a whole number")?;

        let (pk, bkey, bsec, bpass) = if require_credentials {
            let pk    = env::var("POLYMARKET_PRIVATE_KEY").context("POLYMARKET_PRIVATE_KEY not set")?;
            let bkey  = env::var("POLYMARKET_BUILDER_KEY").context("POLYMARKET_BUILDER_KEY not set")?;
            let bsec  = env::var("POLYMARKET_BUILDER_SECRET").context("POLYMARKET_BUILDER_SECRET not set")?;
            let bpass = env::var("POLYMARKET_BUILDER_PASSPHRASE").context("POLYMARKET_BUILDER_PASSPHRASE not set")?;
            (Some(pk), Some(bkey), Some(bsec), Some(bpass))
        } else {
            (None, None, None, None)
        };

        Ok(Config {
            market,
            interval_secs: secs,
            slug_prefix: slug_prefix.to_string(),
            asset: asset.to_string(),
            order_usdc,
            slippage_buffer,
            poll_ms: 200,           // poll Polymarket orderbook every 200 ms
            post_market_secs: 25,   // 25-second post-market trading window
            max_trades_per_market: max_trades,
            private_key: pk,
            builder_api_key: bkey,
            builder_secret: bsec,
            builder_passphrase: bpass,
        })
    }
}
