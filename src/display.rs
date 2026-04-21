use chrono::Utc;

use crate::engine::RenderState;
use crate::types::{MarketPhase, OrderLevel, TokenBook};

// ── ANSI helpers ──────────────────────────────────────────────────────────────

const RESET:  &str = "\x1b[0m";
const BOLD:   &str = "\x1b[1m";
const DIM:    &str = "\x1b[2m";
const CYAN:   &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN:  &str = "\x1b[32m";
const RED:    &str = "\x1b[31m";
const WHITE:  &str = "\x1b[37m";

fn pad_r(s: &str, n: usize) -> String { format!("{:<width$}", s, width = n) }
fn pad_l(s: &str, n: usize) -> String { format!("{:>width$}", s, width = n) }

fn time_left(end_time: i64) -> String {
    let diff = end_time - Utc::now().timestamp();
    if diff <= 0 { return "ENDED".to_string(); }
    format!("{:02}:{:02}", diff / 60, diff % 60)
}

fn candle_bar(o: f64, h: f64, l: f64, c: f64) -> String {
    let dir = if c >= o { "▲" } else { "▼" };
    format!("{} O:{:.1} H:{:.1} L:{:.1} C:{:.1}", dir, o, h, l, c)
}

// ── Main render ───────────────────────────────────────────────────────────────

pub fn render(s: &RenderState) {
    print!("\x1B[2J\x1B[H"); // clear screen + cursor home

    let mode    = if s.is_live { "REAL (live orders)" } else { "TEST (paper)" };
    let now_str = Utc::now().format("%Y-%m-%dT%H:%M:%S UTC").to_string();

    // ── Header ────────────────────────────────────────────────────────────────
    println!("{}{}╔══════════════════════════════════════════════════════════════════╗{}", BOLD, CYAN, RESET);
    println!("{}{}║  {} Polymarket Last-Minute Bot  ─  {:32}{}║{}",
        BOLD, CYAN, s.config.asset, mode, CYAN, RESET);
    println!("{}{}╚══════════════════════════════════════════════════════════════════╝{}", BOLD, CYAN, RESET);

    println!("  {}Time   :{} {}   Poll #{}", DIM, RESET, now_str, s.poll_count);
    let tl = if s.current_market.slug.is_empty() {
        String::new()
    } else {
        time_left(s.current_market.end_time)
    };
    println!("  {}Market :{} {}  ({}{}{})",
        DIM, RESET,
        if s.current_market.slug.is_empty() { "resolving…" } else { &s.current_market.slug },
        CYAN, &tl, RESET,
    );

    // ── Price vs Beat Price ───────────────────────────────────────────────────
    println!();
    println!("  {}──── {} Price vs Beat Price ──────────────────────────────{}", DIM, s.config.asset, RESET);

    let (beat, cur) = (s.beat_price, s.btc_price);
    let delta       = cur - beat;
    let pct         = if beat > 0.0 { delta / beat * 100.0 } else { 0.0 };
    let (dir_label, dir_color) = if cur > beat {
        ("UP  ↑", GREEN)
    } else {
        ("DOWN↓", RED)
    };

    println!("  {}Beat price   :{} {:>14}",
        YELLOW, RESET, if beat > 0.0 { format!("${:.2}", beat) } else { "-".to_string() });
    println!("  {}Current price:{} {}{:>14}{}",
        WHITE, RESET, BOLD, if cur > 0.0 { format!("${:.2}", cur) } else { "-".to_string() }, RESET);
    if beat > 0.0 && cur > 0.0 {
        let sign = if delta >= 0.0 { "+" } else { "" };
        println!("  {}Difference   :{} {}{}{}{:.2}  ({}{:.3}%){}  →  {}{}{}{}",
            DIM, RESET,
            if delta >= 0.0 { GREEN } else { RED },
            sign, "$", delta, sign, pct, RESET,
            BOLD, dir_color, dir_label, RESET,
        );
    }

    // ── Candle history ────────────────────────────────────────────────────────
    println!();
    println!("  {}──── 1-Minute Candles ──────────────────────────────────{}", DIM, RESET);
    let completed = s.candles.last_completed();
    if completed.is_empty() && s.candles.current_candle.is_none() {
        println!("  {}(no candle data yet){}", DIM, RESET);
    } else {
        let all: Vec<_> = completed.iter()
            .copied()
            .chain(s.candles.current_candle.as_ref())
            .collect();
        for c in all.iter().rev().take(3).rev() {
            let ts = chrono::DateTime::from_timestamp(c.start_ts, 0)
                .map(|dt: chrono::DateTime<chrono::Utc>| dt.format("%H:%M").to_string())
                .unwrap_or_else(|| "??:??".to_string());
            let col = if c.close >= c.open { GREEN } else { RED };
            println!("  {}[{}]{}  {}{}{}",
                DIM, ts, RESET, col,
                candle_bar(c.open, c.high, c.low, c.close),
                RESET,
            );
        }
    }

    // ── Orderbook: 5 levels each side ────────────────────────────────────────
    // During the 25-sec window, prefer the refreshed post-market orderbook.
    let ob_to_show = match &s.phase {
        MarketPhase::JustEnded { .. } => {
            s.post_market_orderbook.as_ref().unwrap_or(&s.orderbook)
        }
        _ => &s.orderbook,
    };

    println!();
    println!("  {}──── Orderbook — 5 Ask Levels ─────────────────────────{}", DIM, RESET);
    render_ask_table("UP   asks", &ob_to_show.up,   GREEN);
    println!();
    render_ask_table("DOWN asks", &ob_to_show.down, RED);

    // ── Post-market window ────────────────────────────────────────────────────
    if let MarketPhase::JustEnded { ended_at, winner, end_btc_price } = &s.phase {
        let elapsed   = Utc::now().timestamp() - ended_at;
        let remaining = (s.config.post_market_secs as i64 - elapsed).max(0);
        let win_color = if *winner == "up" { GREEN } else { RED };

        println!();

        if remaining > 0 {
            println!("  {}{}▶ POST-MARKET WINDOW — {}s remaining{}",
                BOLD, YELLOW, remaining, RESET);
        } else {
            println!("  {}Window closed{}", DIM, RESET);
        }

        println!("  {}{}══ MARKET ENDED  Winner: {}{}{}  Beat: ${:.2}  End: ${:.2} ══{}{}",
            BOLD, win_color, winner.to_uppercase(), RESET, BOLD,
            s.post_market_beat_price, end_btc_price,
            win_color, RESET,
        );

        // Show winner-side asks (price < $1.00)
        if let Some(ob) = &s.post_market_orderbook {
            let winner_book = if *winner == "up" { &ob.up } else { &ob.down };
            let below: Vec<&OrderLevel> = winner_book.asks.iter()
                .filter(|a| a.price < 1.0)
                .collect();

            println!();
            if below.is_empty() {
                println!("  {}Winner ({}) asks below $1.00: none{}",
                    DIM, winner.to_uppercase(), RESET);
            } else {
                println!("  {}{}Winner ({}) asks below $1.00 — {} level(s){}",
                    BOLD, YELLOW, winner.to_uppercase(), below.len(), RESET);
                render_ask_rows_filtered(below);
            }
        }

        // Trades placed this market
        println!();
        if s.post_market_trades.is_empty() {
            println!("  {}No trades placed this market.{}", DIM, RESET);
        } else {
            println!("  {}── Trades this market ────────────────────────────────{}", DIM, RESET);
            render_trades_table(&s.post_market_trades);
        }

    } else if !s.bot_trades.is_empty() {
        println!();
        println!("  {}── Trades this market ────────────────────────────────{}", DIM, RESET);
        render_trades_table(&s.bot_trades);
    }

    // ── Status / footer ───────────────────────────────────────────────────────
    println!();
    println!("  {}Status: {}{}", DIM, RESET, s.status_line);
    println!("  {}tick {}ms  │  Ctrl+C to stop{}", DIM, s.last_tick_ms, RESET);
}

// ── Orderbook helpers ─────────────────────────────────────────────────────────

fn render_ask_table(label: &str, book: &TokenBook, color: &str) {
    let asks: Vec<&OrderLevel> = book.asks.iter().take(5).collect();

    println!("  {}{}{} — {} level(s){}",
        BOLD, color, label, asks.len(), RESET);

    if asks.is_empty() {
        println!("    {}(no asks){}", DIM, RESET);
        return;
    }

    // Column header
    println!("    {}{}  {}  {}  {}{}",
        DIM,
        pad_r("Price",   8),
        pad_l("Size",   10),
        pad_l("Value$", 10),
        pad_l("Cum$",   10),
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
            pad_l(&format!("${:.2}", val),  10),
            pad_l(&format!("${:.2}", cum),  10),
        );
    }
    if book.asks.len() > 5 {
        println!("    {}… {} more levels{}", DIM, book.asks.len() - 5, RESET);
    }
}

fn render_ask_rows_filtered(levels: Vec<&OrderLevel>) {
    println!("    {}{}  {}  {}{}",
        DIM,
        pad_r("Price", 8),
        pad_l("Size",  10),
        pad_l("Value$", 10),
        RESET,
    );
    for lvl in levels.iter().take(5) {
        let val = lvl.price * lvl.size;
        println!("    {}{}  {}  {}",
            YELLOW,
            pad_r(&format!("{:.4}", lvl.price), 8),
            pad_l(&format!("{:.3}", lvl.size),  10),
            pad_l(&format!("${:.2}", val),       10),
        );
    }
    if levels.len() > 5 {
        println!("    {}{}… {} more{}", YELLOW, DIM, levels.len() - 5, RESET);
    }
    print!("{}", RESET);
}

fn render_trades_table(trades: &[crate::types::BotTrade]) {
    println!("    {}{}  {}  {}  {}  {}{}",
        DIM,
        pad_r("Time",   8),
        pad_r("Side",   5),
        pad_r("Shares", 8),
        pad_r("USDC",   8),
        "OrderID",
        RESET,
    );
    for t in trades.iter().rev().take(10) {
        let ts    = t.ts.format("%H:%M:%S").to_string();
        let color = if t.outcome == "up" { GREEN } else { RED };
        let short = if t.order_id.len() > 14 { &t.order_id[..14] } else { &t.order_id };
        println!("    {}  {}{}{}  {}  {}  {}",
            pad_r(&ts, 8),
            color, pad_r(&t.outcome.to_uppercase(), 5), RESET,
            pad_r(&format!("{:.3}", t.shares),     8),
            pad_r(&format!("${:.2}", t.usdc_spent), 8),
            short,
        );
    }
}
