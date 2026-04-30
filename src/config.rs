use anyhow::{bail, Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub market: String,
    pub interval_secs: u64,
    pub slug_prefix: String,
    pub asset: String,
    pub order_usdc: f64,         // USDC to spend per trade (TRADE_AMOUNT in .env)
    pub slippage_buffer: f64,
    pub poll_ms: u64,
    pub post_market_secs: u64,   // seconds to keep post-market window open
    pub max_trades_per_market: usize, // 0 = unlimited (MAX_TRADES in .env)
    // real-mode credentials
    pub private_key: Option<String>,
    pub builder_code: Option<String>,  // 32-byte hex builder attribution code (CLOB V2)
}

impl Config {
    pub fn load_test(market: &str) -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::build(market, false)
    }

    pub fn load_real(market: &str) -> Result<Self> {
        dotenvy::dotenv().ok();
        Self::build(market, true)
    }

    fn build(market: &str, require_credentials: bool) -> Result<Self> {
        let market = market.to_string();

        let (secs, slug_prefix, asset) = match market.as_str() {
            "btc-5m"   => (300u64, "btc-updown-5m",   "BTC"),
            "btc-15m"  => (900u64, "btc-updown-15m",  "BTC"),
            "eth-5m"   => (300u64, "eth-updown-5m",   "ETH"),
            "eth-15m"  => (900u64, "eth-updown-15m",  "ETH"),
            "sol-5m"   => (300u64, "sol-updown-5m",   "SOL"),
            "sol-15m"  => (900u64, "sol-updown-15m",  "SOL"),
            "bnb-5m"   => (300u64, "bnb-updown-5m",   "BNB"),
            "bnb-15m"  => (900u64, "bnb-updown-15m",  "BNB"),
            "xrp-5m"   => (300u64, "xrp-updown-5m",   "XRP"),
            "xrp-15m"  => (900u64, "xrp-updown-15m",  "XRP"),
            "doge-5m"  => (300u64, "doge-updown-5m",  "DOGE"),
            "doge-15m" => (900u64, "doge-updown-15m", "DOGE"),
            "hype-5m"  => (300u64, "hype-updown-5m",  "HYPE"),
            "hype-15m" => (900u64, "hype-updown-15m", "HYPE"),
            other => bail!(
                "MARKET must be one of: btc-5m btc-15m eth-5m eth-15m sol-5m sol-15m \
                 bnb-5m bnb-15m xrp-5m xrp-15m doge-5m doge-15m hype-5m hype-15m  \
                 got '{}'", other
            ),
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

        let (pk, builder_code) = if require_credentials {
            let pk   = env::var("POLYMARKET_PRIVATE_KEY").context("POLYMARKET_PRIVATE_KEY not set")?;
            let code = env::var("POLYMARKET_BUILDER_CODE").ok(); // optional
            (Some(pk), code)
        } else {
            (None, None)
        };

        Ok(Config {
            market,
            interval_secs: secs,
            slug_prefix: slug_prefix.to_string(),
            asset: asset.to_string(),
            order_usdc,
            slippage_buffer,
            poll_ms: 200,
            post_market_secs: 25,
            max_trades_per_market: max_trades,
            private_key: pk,
            builder_code,
        })
    }
}
