//! # bot_test — paper / simulation mode
//!
//! Usage:
//!   cargo run --bin bot_test
//!
//! Reads MARKET and ORDER_USDC from .env (copy .env.example → .env and fill in).
//! No real orders are placed; all execution is simulated at the ask price.

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

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║        {} Last-Minute Winner Bot  —  TEST / PAPER MODE       ║", cfg.asset);
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!("  Market       : {}", cfg.market);
    println!("  Order budget : ${:.2} USDC per market", cfg.order_usdc);
    println!("  Slippage buf : {:.4}", cfg.slippage_buffer);
    println!("  Polygon RPC  : {}", cfg.polygon_rpc_url);
    println!("  No real orders will be placed.");
    println!();

    // Show previous session PnL if available
    csv_log::print_session_pnl();

    let executor = Arc::new(TestExecutor);
    let state    = AppState::new(cfg, false);

    run(state, executor).await
}
