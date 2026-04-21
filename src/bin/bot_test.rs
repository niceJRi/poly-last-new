//! # bot_test — paper / simulation mode
//!
//! Usage:
//!   cargo run --bin bot_test
//!
//! No real orders are placed; all execution is simulated at the ask price.
//! Copy .env.example → .env and set MARKET, TRADE_AMOUNT, MAX_TRADES at minimum.

use std::sync::Arc;

use anyhow::Result;

use poly_last_new::{
    config::Config,
    csv_log,
    engine::{run, AppState},
    executor::TestExecutor,
};

fn install_crypto() {
    rustls::crypto::aws_lc_rs::default_provider().install_default().ok();
}

#[tokio::main]
async fn main() -> Result<()> {
    install_crypto();

    let cfg = Config::load_test()?;

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║      {} Last-Minute Winner Bot  —  TEST / PAPER MODE           ║", cfg.asset);
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!("  Market        : {}", cfg.market);
    println!("  Trade amount  : ${:.2} USDC", cfg.order_usdc);
    println!("  Max trades    : {}", if cfg.max_trades_per_market == 0 { "unlimited".to_string() } else { cfg.max_trades_per_market.to_string() });
    println!("  Slippage buf  : {:.4}", cfg.slippage_buffer);
    println!("  Post-mkt win  : {}s", cfg.post_market_secs);
    println!("  Price source  : Binance public API (250 ms polling)");
    println!("  No real orders will be placed.");
    println!();

    csv_log::print_session_pnl();

    let executor = Arc::new(TestExecutor);
    let state    = AppState::new(cfg, false);

    run(state, executor).await
}
