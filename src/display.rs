use chrono::Utc;

use crate::engine::RenderState;
use crate::types::{MarketPhase, TokenBook};

// ── Formatting helpers ────────────────────────────────────────────────────────

fn pad_r(s: &str, n: usize) -> String { format!("{:<width$}", s, width = n) }
fn pad_l(s: &str, n: usize) -> String { format!("{:>width$}", s, width = n) }

fn fmt_price(v: f64) -> String {
    if v <= 0.0 { return "-".to_string(); }
    format!("${:>12.2}", v)
}

fn fmt_delta(v: f64) -> String {
    if v > 0.0 { format!(" +{:.2}", v) }
    else       { format!(" {:.2}",  v) }
}

fn fmt_pct(v: f64) -> String {
    if v > 0.0 { format!("(+{:.3}%)", v) }
    else       { format!("({:.3}%)",  v) }
}

fn time_left(end_time: i64) -> String {
    let diff = end_time - Utc::now().timestamp();
    if diff <= 0 { return "ENDED".to_string(); }
    format!("{:02}:{:02}", diff / 60, diff % 60)
}

fn candle_bar(o: f64, h: f64, l: f64, c: f64) -> String {
    let _ = (h, l); // used for display only
    let dir = if c >= o { "▲" } else { "▼" };
    format!("{} O:{:.1} H:{:.1} L:{:.1} C:{:.1}", dir, o, h, l, c)
}

fn direction_label(current: f64, beat: f64) -> (&'static str, &'static str) {
    if current > beat { ("UP  ↑", "\x1b[32m") }   // green
    else              { ("DOWN↓", "\x1b[31m") }    // red
}

const RESET: &str = "\x1b[0m";
const BOLD:  &str = "\x1b[1m";
const CYAN:  &str = "\x1b[36m";
const YELLOW:&str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const RED:   &str = "\x1b[31m";
const DIM:   &str = "\x1b[2m";

// ── Main render ───────────────────────────────────────────────────────────────

pub fn render(s: &RenderState) {
    print!("\x1B[2J\x1B[H"); // clear screen + move to top

    let mode = if s.is_live { "REAL (live orders)" } else { "TEST (paper)" };
    let now_str = Utc::now().format("%Y-%m-%dT%H:%M:%S UTC").to_string();

    println!("{}{}╔══════════════════════════════════════════════════════════════════╗{}", BOLD, CYAN, RESET);
    println!("{}{}║  {} Last-Minute Winner Bot  ─  {}{}{}║{}",
        BOLD, CYAN, s.config.asset,
        pad_r(mode, 38),
        CYAN, "", RESET);
    println!("{}{}╚══════════════════════════════════════════════════════════════════╝{}", BOLD, CYAN, RESET);

    println!("  {}Time   :{} {}  Poll #{}", DIM, RESET, now_str, s.poll_count);
    let tl = if s.current_market.slug.is_empty() { String::new() } else { time_left(s.current_market.end_time) };
    println!("  {}Market :{} {}  ({}{}{})",
        DIM, RESET,
        if s.current_market.slug.is_empty() { "resolving..." } else { &s.current_market.slug },
        CYAN, &tl, RESET,
    );

    // ── BTC Price + Beat Price ────────────────────────────────────────────────
    println!();
    println!("  {}────── {} Price vs Beat Price ──────────────────────────{}", DIM, s.config.asset, RESET);

    let (dir_label, dir_color) = direction_label(s.btc_price, s.beat_price);
    let delta = s.btc_price - s.beat_price;
    let pct   = if s.beat_price > 0.0 { delta / s.beat_price * 100.0 } else { 0.0 };

    println!("  Beat price  : {}{}{}", YELLOW, fmt_price(s.beat_price), RESET);
    println!("  Current     : {}{}{}", BOLD,   fmt_price(s.btc_price),  RESET);
    println!("  Delta       : {}{}{}  {}{}{}  →  {}{}{}{}",
        if delta >= 0.0 { GREEN } else { RED },
        fmt_delta(delta), RESET,
        DIM, fmt_pct(pct), RESET,
        BOLD, dir_color, dir_label, RESET,
    );

    // ── Candle history ────────────────────────────────────────────────────────
    println!();
    println!("  {}────── 1-Minute Candles ({}) ───────────────────────────{}", DIM, s.config.asset, RESET);

    let completed = s.candles.last_completed();
    if completed.is_empty() && s.candles.current_candle.is_none() {
        println!("  {}No candle data yet...{}", DIM, RESET);
    } else {
        let all: Vec<_> = completed.iter()
            .copied()
            .chain(s.candles.current_candle.as_ref())
            .collect();
        for candle in all.iter().rev().take(3).rev() {
            let ts = chrono::DateTime::from_timestamp(candle.start_ts, 0)
                .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%H:%M").to_string())
                .unwrap_or_else(|| "??:??".to_string());
            let color = if candle.close >= candle.open { GREEN } else { RED };
            println!("  {}[{}]{}  {}{}{}",
                DIM, ts, RESET,
                color,
                candle_bar(candle.open, candle.high, candle.low, candle.close),
                RESET,
            );
        }
    }

    // ── Orderbook ────────────────────────────────────────────────────────────
    println!();
    println!("  {}────── Orderbook ──────────────────────────────────────{}", DIM, RESET);

    render_token_book("UP   Asks", &s.orderbook.up,   GREEN);
    println!();
    render_token_book("DOWN Asks", &s.orderbook.down, RED);

    // ── Post-market section ───────────────────────────────────────────────────
    if let MarketPhase::JustEnded { ended_at, winner, end_btc_price } = &s.phase {
        let elapsed = Utc::now().timestamp() - ended_at;
        let remaining = (s.config.post_market_secs as i64 - elapsed).max(0);

        println!();
        let win_color = if *winner == "up" { GREEN } else { RED };

        // Show active trading indicator with countdown
        let window_label = if remaining > 0 {
            format!("  {}{}TRADING WINDOW ACTIVE — {}s remaining{}", BOLD, YELLOW, remaining, RESET)
        } else {
            format!("  {}Window closed{}", DIM, RESET)
        };
        println!("{}", window_label);

        println!("  {}{}════ MARKET ENDED — Winner: {}{}{}  Beat: ${:.2}  End: ${:.2} ════{}{}",
            BOLD, win_color, winner.to_uppercase(), RESET, BOLD,
            s.post_market_beat_price, end_btc_price,
            win_color, RESET);

        println!("  Delta: {}{}{}",
            if end_btc_price >= &s.post_market_beat_price { GREEN } else { RED },
            fmt_delta(end_btc_price - s.post_market_beat_price),
            RESET,
        );

        if let Some(ob) = &s.post_market_orderbook {
            let winner_book = if *winner == "up" { &ob.up } else { &ob.down };
            let below_1: Vec<_> = winner_book.asks.iter().filter(|a| a.price < 1.0).collect();
            println!();
            if below_1.is_empty() {
                println!("  {}Winner ({}) asks below $1.00: none{}",
                    DIM, winner.to_uppercase(), RESET);
            } else {
                println!("  {}{}Winner ({}) asks below $1.00 — {} level(s):{}",
                    BOLD, YELLOW, winner.to_uppercase(), below_1.len(), RESET);
                render_token_book_compact_filtered(winner_book);
            }
        }

        if !s.post_market_trades.is_empty() {
            println!();
            println!("  {}── My trades this market ─────────────────────────────────{}",
                DIM, RESET);
            render_trades_table(&s.post_market_trades);
        } else {
            println!();
            println!("  {}No trades placed yet this market.{}", DIM, RESET);
        }
    } else if !s.bot_trades.is_empty() {
        println!();
        println!("  {}── My trades this market ─────────────────────────────────{}", DIM, RESET);
        render_trades_table(&s.bot_trades);
    }

    // ── Status line ───────────────────────────────────────────────────────────
    println!();
    println!("  {}Status: {}{}", DIM, RESET, s.status_line);
    println!("  {}tick {}ms  |  Ctrl+C to stop{}", DIM, s.last_tick_ms, RESET);
}

// ── Orderbook table helpers ───────────────────────────────────────────────────

fn render_token_book(label: &str, book: &TokenBook, color: &str) {
    let asks = book.asks.iter().take(5).collect::<Vec<_>>();
    let total_usdc = book.total_ask_usdc();
    let total_shares = book.total_ask_shares();

    println!("  {}{} — {} levels  ${:.2} total USDC  {:.2} shares{}",
        color, label,
        asks.len(), total_usdc, total_shares, RESET);

    if asks.is_empty() {
        println!("    {}(no asks){}", DIM, RESET);
        return;
    }

    println!("    {}{}  {}  {}  {}{}",
        DIM,
        pad_r("Price", 8),
        pad_l("Size", 10),
        pad_l("Value($)", 10),
        pad_l("Cum$", 10),
        RESET,
    );

    let mut cum = 0.0f64;
    for level in &asks {
        let val = level.price * level.size;
        cum += val;
        println!("    {}  {}{}{}  {}  {}",
            pad_r(&format!("{:.4}", level.price), 8),
            color,
            pad_l(&format!("{:.3}", level.size), 10),
            RESET,
            pad_l(&format!("${:.2}", val), 10),
            pad_l(&format!("${:.2}", cum), 10),
        );
    }
    if book.asks.len() > 5 {
        println!("    {}... {} more levels{}", DIM, book.asks.len() - 5, RESET);
    }
}

fn render_token_book_compact_filtered(book: &TokenBook) {
    let below: Vec<_> = book.asks.iter().filter(|a| a.price < 1.0).collect();
    if below.is_empty() {
        println!("    {}(no asks below $1.00){}", DIM, RESET);
        return;
    }
    for level in below.iter().take(8) {
        let val = level.price * level.size;
        println!("    {}price: {:.4}  size: {:.3}  value: ${:.2}{}",
            YELLOW, level.price, level.size, val, RESET);
    }
    if below.len() > 8 {
        println!("    {}... {} more{}", DIM, below.len() - 8, RESET);
    }
}

fn render_trades_table(trades: &[crate::types::BotTrade]) {
    println!("  {}{}  {}  {}  {}  {}  {}{}",
        DIM,
        pad_r("Time",    8),
        pad_r("Side",    5),
        pad_r("Shares",  8),
        pad_r("USDC",    8),
        pad_r("Price",   7),
        "OrderID",
        RESET,
    );
    for t in trades.iter().rev().take(10) {
        let ts  = t.ts.format("%H:%M:%S").to_string();
        let color = if t.outcome == "up" { GREEN } else { RED };
        let short_id = if t.order_id.len() > 12 { &t.order_id[..12] } else { &t.order_id };
        println!("  {}  {}{}{}  {}  {}  {}",
            pad_r(&ts, 8),
            color, pad_r(&t.outcome.to_uppercase(), 5), RESET,
            pad_r(&format!("{:.3}", t.shares), 8),
            pad_r(&format!("${:.2}", t.usdc_spent), 8),
            format!("{:<7}  {}", format!("{:.4}", t.fill_price), short_id),
        );
    }
}
