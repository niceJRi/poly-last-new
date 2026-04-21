/// CSV persistence layer.
///
/// Per-market trades : data/<slug>_trades.csv
/// PnL summary       : data/pnl_summary.csv
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

// ── Append a single executed trade ───────────────────────────────────────────

pub fn append_trade(trade: &BotTrade) -> Result<()> {
    ensure_data_dir()?;

    let path   = trade_csv_path(&trade.market_slug);
    let is_new = !Path::new(&path).exists();

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

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

// ── Append PnL row when a market resolves ────────────────────────────────────

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

    let path   = format!("{}/pnl_summary.csv", DATA_DIR);
    let is_new = !Path::new(&path).exists();

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

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

// ── Print cumulative PnL from pnl_summary.csv ────────────────────────────────

pub fn print_session_pnl() {
    let path = format!("{}/pnl_summary.csv", DATA_DIR);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut total_pnl   = 0.0f64;
    let mut total_spent = 0.0f64;
    let mut wins        = 0usize;
    let mut losses      = 0usize;

    for (i, line) in content.lines().enumerate() {
        if i == 0 { continue; }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 9 { continue; }
        let usdc: f64 = cols[6].parse().unwrap_or(0.0);
        let pnl:  f64 = cols[8].parse().unwrap_or(0.0);
        total_pnl   += pnl;
        total_spent += usdc;
        if pnl > 0.0 { wins += 1; } else { losses += 1; }
    }

    let count = wins + losses;
    if count == 0 { return; }

    let sign = if total_pnl >= 0.0 { "+" } else { "" };
    println!(
        "  Historical PnL  ({} markets / {} wins / {} losses)   {}{}  USDC spent: ${:.2}",
        count, wins, losses,
        sign, format!("{:.4}", total_pnl), total_spent
    );
}

// ── Grand-total PnL across all CSV files (for `cargo run --bin pnl`) ─────────

pub struct PnlSummary {
    pub total_pnl:   f64,
    pub total_spent: f64,
    pub wins:        usize,
    pub losses:      usize,
    pub rows: Vec<PnlRow>,
}

pub struct PnlRow {
    pub slug:        String,
    pub beat_price:  f64,
    pub end_price:   f64,
    pub winner:      String,
    pub our_outcome: String,
    pub usdc_spent:  f64,
    pub pnl:         f64,
}

pub fn load_pnl_summary() -> PnlSummary {
    let path = format!("{}/pnl_summary.csv", DATA_DIR);
    let content = std::fs::read_to_string(&path).unwrap_or_default();

    let mut summary = PnlSummary {
        total_pnl:   0.0,
        total_spent: 0.0,
        wins:        0,
        losses:      0,
        rows:        Vec::new(),
    };

    for (i, line) in content.lines().enumerate() {
        if i == 0 { continue; }
        let c: Vec<&str> = line.split(',').collect();
        if c.len() < 9 { continue; }

        let pnl:   f64 = c[8].parse().unwrap_or(0.0);
        let spent: f64 = c[6].parse().unwrap_or(0.0);

        summary.total_pnl   += pnl;
        summary.total_spent += spent;
        if pnl > 0.0 { summary.wins += 1; } else { summary.losses += 1; }

        summary.rows.push(PnlRow {
            slug:        c[0].to_string(),
            beat_price:  c[1].parse().unwrap_or(0.0),
            end_price:   c[2].parse().unwrap_or(0.0),
            winner:      c[3].to_string(),
            our_outcome: c[4].to_string(),
            usdc_spent:  spent,
            pnl,
        });
    }

    summary
}
