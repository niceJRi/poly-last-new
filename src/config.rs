use anyhow::{bail, Context, Result};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub market: String,           // "btc-5m", "btc-15m", "eth-5m", "eth-15m"
    pub interval_secs: u64,
    pub slug_prefix: String,      // "btc-updown-5m" etc.
    pub asset: String,            // "BTC" or "ETH"
    pub order_usdc: f64,          // USDC amount to buy after winner confirmed
    pub slippage_buffer: f64,     // extra price buffer when placing orders
    pub poll_ms: u64,             // polling interval in ms
    pub polygon_rpc_url: String,
    pub chainlink_feed: String,   // On-chain aggregator address on Polygon (fallback)
    pub ds_api_key: Option<String>,    // Chainlink Data Streams API key
    pub ds_api_secret: Option<String>, // Chainlink Data Streams API secret
    pub ds_feed_id: String,            // Data Streams feed ID for this asset
    // real-mode credentials
    pub private_key: Option<String>,
    pub builder_api_key: Option<String>,
    pub builder_secret: Option<String>,
    pub builder_passphrase: Option<String>,
    pub post_market_secs: u64,    // seconds to show post-market info
    pub max_trades_per_market: usize, // max number of trades to place per market (0 = unlimited)
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

        // ds_feed_id: Chainlink Data Streams feed ID (from data.chain.link/streams/...)
        // btc-usd-cexprice-streams → 0x00036fe43f87884450b4c7e093cd5ed99cac6640d8c2000e6afc02c8838d0265
        // eth-usd-cexprice-streams → 0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782
        let (secs, slug_prefix, asset, chainlink_feed, ds_feed_id) = match market.as_str() {
            "btc-5m"  => (300u64, "btc-updown-5m",  "BTC",
                "0xc907E116054Ad103354f2D350FD2514433D57F6f",
                "0x00036fe43f87884450b4c7e093cd5ed99cac6640d8c2000e6afc02c8838d0265"),
            "btc-15m" => (900u64, "btc-updown-15m", "BTC",
                "0xc907E116054Ad103354f2D350FD2514433D57F6f",
                "0x00036fe43f87884450b4c7e093cd5ed99cac6640d8c2000e6afc02c8838d0265"),
            "eth-5m"  => (300u64, "eth-updown-5m",  "ETH",
                "0xF9680D99D6C9589e2a93a78A04A279e509205945",
                "0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782"),
            "eth-15m" => (900u64, "eth-updown-15m", "ETH",
                "0xF9680D99D6C9589e2a93a78A04A279e509205945",
                "0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782"),
            other => bail!("MARKET must be btc-5m | btc-15m | eth-5m | eth-15m, got '{}'", other),
        };

        let ds_api_key    = env::var("CHAINLINK_DS_API_KEY").ok();
        let ds_api_secret = env::var("CHAINLINK_DS_API_SECRET").ok();

        let order_usdc: f64 = env::var("ORDER_USDC")
            .unwrap_or_else(|_| "10.0".to_string())
            .parse()
            .context("ORDER_USDC must be a number")?;

        if order_usdc <= 0.0 {
            bail!("ORDER_USDC must be > 0");
        }

        let slippage_buffer: f64 = env::var("SLIPPAGE_BUFFER")
            .unwrap_or_else(|_| "0.02".to_string())
            .parse()
            .context("SLIPPAGE_BUFFER must be a number")?;

        let polygon_rpc_url = env::var("POLYGON_RPC_URL")
            .unwrap_or_else(|_| "https://polygon-rpc.com".to_string());

        let (pk, bkey, bsec, bpass) = if require_credentials {
            let pk   = env::var("POLYMARKET_PRIVATE_KEY").context("POLYMARKET_PRIVATE_KEY not set")?;
            let bkey = env::var("POLYMARKET_BUILDER_KEY").context("POLYMARKET_BUILDER_KEY not set")?;
            let bsec = env::var("POLYMARKET_BUILDER_SECRET").context("POLYMARKET_BUILDER_SECRET not set")?;
            let bpass= env::var("POLYMARKET_BUILDER_PASSPHRASE").context("POLYMARKET_BUILDER_PASSPHRASE not set")?;
            (Some(pk), Some(bkey), Some(bsec), Some(bpass))
        } else {
            (None, None, None, None)
        };

        Ok(Config {
            market,
            interval_secs: secs,
            slug_prefix: slug_prefix.to_string(),
            asset: asset.to_string(),
            chainlink_feed: chainlink_feed.to_string(),
            ds_api_key,
            ds_api_secret,
            ds_feed_id: ds_feed_id.to_string(),
            order_usdc,
            slippage_buffer,
            poll_ms: 500,
            polygon_rpc_url,
            private_key: pk,
            builder_api_key: bkey,
            builder_secret: bsec,
            builder_passphrase: bpass,
            post_market_secs: 30,
            max_trades_per_market: env::var("MAX_TRADES_PER_MARKET")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .context("MAX_TRADES_PER_MARKET must be a whole number")?,
        })
    }
}
