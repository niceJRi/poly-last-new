//! # bot_real — live trading mode
//!
//! Usage:
//!   cargo run --bin bot_real
//!
//! Reads all credentials and settings from .env.
//! REAL ORDERS ARE PLACED — ensure your wallet has USDC before running.

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

    println!("╔═══════════════════════════════════════════════════════════════╗");
    println!("║        {} Last-Minute Winner Bot  —  REAL / LIVE MODE        ║", cfg.asset);
    println!("╚═══════════════════════════════════════════════════════════════╝");
    println!("  Market       : {}", cfg.market);
    println!("  Order budget : ${:.2} USDC per market", cfg.order_usdc);
    println!("  Slippage buf : {:.4}", cfg.slippage_buffer);
    println!("  Polygon RPC  : {}", cfg.polygon_rpc_url);
    println!();
    println!("  ⚠  REAL ORDERS WILL BE PLACED — press Ctrl+C within 3s to abort.");
    println!();

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Show previous session PnL if available
    csv_log::print_session_pnl();

    let executor = Arc::new(RealExecutor::new(&cfg).await?);
    let state    = AppState::new(cfg, true);

    run(state, executor).await
}
