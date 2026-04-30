//! # bot_real — live trading mode
//!
//! Usage:
//!   cargo run --bin bot_real -- <market>
//!   cargo run --bin bot_real -- btc-5m
//!
//! REAL ORDERS ARE PLACED — ensure your wallet has USDC and all .env keys are set.

use std::sync::Arc;

use anyhow::Result;

use poly_last_new::{
    config::Config,
    csv_log,
    engine::{run, AppState},
    executor::RealExecutor,
};

fn install_crypto() {
    rustls::crypto::aws_lc_rs::default_provider().install_default().ok();
}

#[tokio::main]
async fn main() -> Result<()> {
    install_crypto();

    let market = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: bot_real <market>");
        eprintln!("  Markets: btc-5m | btc-15m | eth-5m | eth-15m | sol-5m | sol-15m");
        eprintln!("           bnb-5m | bnb-15m | xrp-5m | xrp-15m | doge-5m | doge-15m | hype-5m | hype-15m");
        std::process::exit(1);
    });
    let cfg = Config::load_real(&market)?;

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║      {} Last-Minute Winner Bot  —  REAL / LIVE MODE            ║", cfg.asset);
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!("  Market        : {}", cfg.market);
    println!("  Trade amount  : ${:.2} USDC", cfg.order_usdc);
    println!("  Max trades    : {}", if cfg.max_trades_per_market == 0 { "unlimited".to_string() } else { cfg.max_trades_per_market.to_string() });
    println!("  Slippage buf  : {:.4}", cfg.slippage_buffer);
    println!("  Post-mkt win  : {}s", cfg.post_market_secs);
    println!("  Price source  : Binance public API (250 ms polling)");
    println!();
    println!("  !! REAL ORDERS WILL BE PLACED — press Ctrl+C within 3s to abort !!");
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    csv_log::print_session_pnl();

    let executor = Arc::new(RealExecutor::new(&cfg).await?);
    let state    = AppState::new(cfg, true);

    run(state, executor).await
}
