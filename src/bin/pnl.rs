//! Calculate and display total PnL from all trade data in the data/ folder.
//!
//! Usage:
//!   cargo run --bin pnl

use poly_last_new::csv_log::load_pnl_summary;

fn main() {
    let s = load_pnl_summary();

    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║              Polymarket Bot — Actual PnL Summary                 ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");
    println!();

    if s.rows.is_empty() {
        println!("  No trade data found in data/pnl_summary.csv.");
        println!("  Run the bot first to generate trade history.");
        return;
    }

    // Per-market rows
    println!("  {:<44}  {:>10}  {:>6}  {:>10}",
        "Market slug", "Beat price", "Result", "PnL (USDC)");
    println!("  {}", "─".repeat(76));

    for row in &s.rows {
        let result = if row.our_outcome == row.winner { "WIN" } else { "LOSS" };
        let pnl_str = sign_str(row.pnl, 4);
        println!("  {:<44}  {:>10.2}  {:>6}  {:>10}",
            truncate(&row.slug, 44), row.beat_price, result, pnl_str);
    }

    println!("  {}", "─".repeat(76));
    println!();

    let count = s.wins + s.losses;
    let win_rate = if count > 0 { s.wins as f64 / count as f64 * 100.0 } else { 0.0 };
    let roi      = if s.total_spent > 0.0 { s.total_pnl / s.total_spent * 100.0 } else { 0.0 };

    println!("  Markets traded : {}", count);
    println!("  Wins / Losses  : {} / {}  ({:.1}% win rate)", s.wins, s.losses, win_rate);
    println!("  USDC spent     : ${:.4}", s.total_spent);
    println!("  Total PnL      : {} USDC", sign_str(s.total_pnl, 4));
    println!("  ROI            : {}%", sign_str(roi, 2));
    println!();
}

fn sign_str(v: f64, decimals: usize) -> String {
    if v >= 0.0 {
        format!("+{:.prec$}", v, prec = decimals)
    } else {
        format!("{:.prec$}", v, prec = decimals)
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}
