/// All CSV files are written to `data/` folder.
/// Per-market trades:  data/<slug>_trades.csv
/// PnL summary:        data/pnl_summary.csv
use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::types::BotTrade;

const DATA_DIR: &str = "data";

fn ensure_data_dir() -> Result<()> {
    if !Path::new(DATA_DIR).exists() {
        fs::create_dir_all(DATA_DIR)?;
    }
    Ok(())
}

fn trade_csv_path(slug: &str) -> String {
    let safe: String = slug.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{}/{}_trades.csv", DATA_DIR, safe)
}

// ── Append a single bot trade ─────────────────────────────────────────────────

pub fn append_trade(trade: &BotTrade) -> Result<()> {
    ensure_data_dir()?;

    let path = trade_csv_path(&trade.market_slug);
    let is_new = !Path::new(&path).exists();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    if is_new {
        writeln!(file,
            "executed_at,market_slug,outcome,shares,usdc_spent,fill_price,order_id,is_live"
        )?;
    }

    writeln!(file,
        "{},{},{},{:.4},{:.4},{:.6},{},{}",
        trade.ts.format("%Y-%m-%dT%H:%M:%S"),
        trade.market_slug,
        trade.outcome.to_uppercase(),
        trade.shares,
        trade.usdc_spent,
        trade.fill_price,
        trade.order_id,
        trade.is_live,
    )?;

    Ok(())
}

// ── Append PnL row when market is resolved ────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn append_pnl_row(
    slug: &str,
    beat_price: f64,
    end_price: f64,
    winner: &str,
    trade: &BotTrade,
    pnl: f64,
    resolved: bool,
) -> Result<()> {
    ensure_data_dir()?;

    let path = format!("{}/pnl_summary.csv", DATA_DIR);
    let is_new = !Path::new(&path).exists();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;

    if is_new {
        writeln!(file,
            "slug,beat_price,end_price,winner,our_outcome,shares,usdc_spent,fill_price,pnl,resolved,executed_at,order_id"
        )?;
    }

    writeln!(file,
        "{},{:.2},{:.2},{},{},{:.4},{:.4},{:.6},{:.4},{},{},{}",
        slug,
        beat_price,
        end_price,
        winner,
        trade.outcome,
        trade.shares,
        trade.usdc_spent,
        trade.fill_price,
        pnl,
        resolved,
        trade.ts.format("%Y-%m-%dT%H:%M:%S"),
        trade.order_id,
    )?;

    Ok(())
}

// ── Print PnL summary from file ───────────────────────────────────────────────

pub fn print_session_pnl() {
    let path = format!("{}/pnl_summary.csv", DATA_DIR);
    let Ok(content) = std::fs::read_to_string(&path) else { return; };

    let mut total_pnl = 0.0f64;
    let mut count = 0usize;
    let mut wins  = 0usize;

    for (i, line) in content.lines().enumerate() {
        if i == 0 { continue; } // header
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 9 { continue; }
        if let Ok(pnl) = cols[8].parse::<f64>() {
            total_pnl += pnl;
            count += 1;
            if pnl > 0.0 { wins += 1; }
        }
    }

    if count > 0 {
        println!("Session PnL: {}{:.4} USDC  ({}/{} markets profitable)",
            if total_pnl >= 0.0 { "+" } else { "" },
            total_pnl, wins, count
        );
    }
}
