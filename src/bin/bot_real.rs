//! # bot_real — live trading mode
//!
//! Usage:
//!   cargo run --bin bot_real
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

    let cfg = Config::load_real()?;

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
