use std::io::Write as IoWrite;

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
    let mut buf = String::with_capacity(4096);

    // Move cursor to top-left without clearing — prevents any scroll
    buf.push_str("\x1B[H");

    let mode    = if s.is_live { "REAL (live orders)" } else { "TEST (paper)" };
    let now_str = Utc::now().format("%Y-%m-%dT%H:%M:%S UTC").to_string();

    // ── Header ────────────────────────────────────────────────────────────────
    wl(&mut buf, &format!("{}{}╔══════════════════════════════════════════════════════════════════╗{}", BOLD, CYAN, RESET));
    wl(&mut buf, &format!("{}{}║  {} Polymarket Last-Minute Bot  ─  {:32}{}║{}",
        BOLD, CYAN, s.config.asset, mode, CYAN, RESET));
    wl(&mut buf, &format!("{}{}╚══════════════════════════════════════════════════════════════════╝{}", BOLD, CYAN, RESET));

    wl(&mut buf, &format!("  {}Time   :{} {}   Poll #{}", DIM, RESET, now_str, s.poll_count));
    let tl = if s.current_market.slug.is_empty() {
        String::new()
    } else {
        time_left(s.current_market.end_time)
    };
    wl(&mut buf, &format!("  {}Market :{} {}  ({}{}{})",
        DIM, RESET,
        if s.current_market.slug.is_empty() { "resolving…" } else { &s.current_market.slug },
        CYAN, &tl, RESET,
    ));

    // ── Wallet / balance (real mode only) ─────────────────────────────────────
    if s.is_live {
        let bal_color = if s.usdc_balance >= 1.0 { GREEN } else { RED };
        let wallet = if s.wallet_address.is_empty() {
            "deriving…".to_string()
        } else {
            format!("{}…{}", &s.wallet_address[..6], &s.wallet_address[s.wallet_address.len()-4..])
        };
        wl(&mut buf, &format!("  {}Wallet :{} {}   {}{}Balance: ${:.2}{}",
            DIM, RESET, wallet,
            bal_color, BOLD, s.usdc_balance, RESET,
        ));
    }

    // ── Price vs Beat Price ───────────────────────────────────────────────────
    wl(&mut buf, "");
    wl(&mut buf, &format!("  {}──── {} Price vs Beat Price ──────────────────────────────{}", DIM, s.config.asset, RESET));

    let (beat, cur) = (s.beat_price, s.btc_price);
    let delta       = cur - beat;
    let pct         = if beat > 0.0 { delta / beat * 100.0 } else { 0.0 };
    let (dir_label, dir_color) = if cur > beat { ("UP  ↑", GREEN) } else { ("DOWN↓", RED) };

    wl(&mut buf, &format!("  {}Beat price   :{} {:>14}",
        YELLOW, RESET, if beat > 0.0 { format!("${:.2}", beat) } else { "-".to_string() }));
    wl(&mut buf, &format!("  {}Current price:{} {}{:>14}{}",
        WHITE, RESET, BOLD, if cur > 0.0 { format!("${:.2}", cur) } else { "-".to_string() }, RESET));
    if beat > 0.0 && cur > 0.0 {
        let sign = if delta >= 0.0 { "+" } else { "" };
        wl(&mut buf, &format!("  {}Difference   :{} {}{}{}{:.2}  ({}{:.3}%){}  →  {}{}{}{}",
            DIM, RESET,
            if delta >= 0.0 { GREEN } else { RED },
            sign, "$", delta, sign, pct, RESET,
            BOLD, dir_color, dir_label, RESET,
        ));
    } else {
        wl(&mut buf, "");
    }

    // ── Candle history ────────────────────────────────────────────────────────
    wl(&mut buf, "");
    wl(&mut buf, &format!("  {}──── 1-Minute Candles ──────────────────────────────────{}", DIM, RESET));
    let completed = s.candles.last_completed();
    if completed.is_empty() && s.candles.current_candle.is_none() {
        wl(&mut buf, &format!("  {}(no candle data yet){}", DIM, RESET));
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
            wl(&mut buf, &format!("  {}[{}]{}  {}{}{}",
                DIM, ts, RESET, col,
                candle_bar(c.open, c.high, c.low, c.close),
                RESET,
            ));
        }
    }

    // ── Orderbook ─────────────────────────────────────────────────────────────
    let ob_to_show = match &s.phase {
        MarketPhase::JustEnded { .. } => {
            s.post_market_orderbook.as_ref().unwrap_or(&s.orderbook)
        }
        _ => &s.orderbook,
    };

    wl(&mut buf, "");
    wl(&mut buf, &format!("  {}──── Orderbook — 5 Ask Levels ─────────────────────────{}", DIM, RESET));
    render_ask_table(&mut buf, "UP   asks", &ob_to_show.up,   GREEN);
    wl(&mut buf, "");
    render_ask_table(&mut buf, "DOWN asks", &ob_to_show.down, RED);

    // ── Post-market window ────────────────────────────────────────────────────
    if let MarketPhase::JustEnded { ended_at, .. } = &s.phase {
        let elapsed   = Utc::now().timestamp() - ended_at;
        let remaining = (s.config.post_market_secs as i64 - elapsed).max(0);
        // Use post_market_winner / post_market_end_price from state — these are
        // patched once the oracle delivers the exact boundary-second Chainlink round,
        // unlike the stale values stored inside the JustEnded enum at capture time.
        let winner    = &s.post_market_winner;
        let end_price = s.post_market_end_price;
        let win_color = if winner == "up" { GREEN } else { RED };

        wl(&mut buf, "");
        if remaining > 0 {
            wl(&mut buf, &format!("  {}{}▶ POST-MARKET WINDOW — {}s remaining{}",
                BOLD, YELLOW, remaining, RESET));
        } else {
            wl(&mut buf, &format!("  {}Window closed{}", DIM, RESET));
        }

        wl(&mut buf, &format!("  {}{}══ MARKET ENDED  Winner: {}{}{}  Beat: ${:.2}  End: ${:.2} ══{}{}",
            BOLD, win_color, winner.to_uppercase(), RESET, BOLD,
            s.post_market_beat_price, end_price,
            win_color, RESET,
        ));

        if let Some(ob) = &s.post_market_orderbook {
            let winner_book = if winner == "up" { &ob.up } else { &ob.down };
            let below: Vec<&OrderLevel> = winner_book.asks.iter()
                .filter(|a| a.price < 1.0)
                .collect();

            wl(&mut buf, "");
            if below.is_empty() {
                wl(&mut buf, &format!("  {}Winner ({}) asks below $1.00: none{}",
                    DIM, winner.to_uppercase(), RESET));
            } else {
                wl(&mut buf, &format!("  {}{}Winner ({}) asks below $1.00 — {} level(s){}",
                    BOLD, YELLOW, winner.to_uppercase(), below.len(), RESET));
                render_ask_rows_filtered(&mut buf, below);
            }
        }

        wl(&mut buf, "");
        if s.post_market_trades.is_empty() {
            wl(&mut buf, &format!("  {}No trades placed this market.{}", DIM, RESET));
        } else {
            wl(&mut buf, &format!("  {}── Trades this market ────────────────────────────────{}", DIM, RESET));
            render_trades_table(&mut buf, &s.post_market_trades);
        }

    } else if !s.bot_trades.is_empty() {
        wl(&mut buf, "");
        wl(&mut buf, &format!("  {}── Trades this market ────────────────────────────────{}", DIM, RESET));
        render_trades_table(&mut buf, &s.bot_trades);
    }

    // ── Status / footer ───────────────────────────────────────────────────────
    wl(&mut buf, "");
    wl(&mut buf, &format!("  {}Status: {}{}", DIM, RESET, s.status_line));
    wl(&mut buf, &format!("  {}tick {}ms  │  Ctrl+C to stop{}", DIM, s.last_tick_ms, RESET));

    // Clear from here to end of screen (erases leftover lines from previous render)
    buf.push_str("\x1B[0J");

    // Write the entire frame atomically — no interleaving with eprintln! from other tasks
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(buf.as_bytes()).ok();
    lock.flush().ok();
}

// Write a line with \x1B[K (erase to end of line) to prevent leftover characters
fn wl(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push_str("\x1B[K\n");  // clear rest of line before newline
}

// ── Orderbook helpers ─────────────────────────────────────────────────────────

fn render_ask_table(buf: &mut String, label: &str, book: &TokenBook, color: &str) {
    let asks: Vec<&OrderLevel> = book.asks.iter().take(5).collect();

    wl(buf, &format!("  {}{}{} — {} level(s){}",
        BOLD, color, label, asks.len(), RESET));

    if asks.is_empty() {
        wl(buf, &format!("    {}(no asks){}", DIM, RESET));
        return;
    }

    wl(buf, &format!("    {}{}  {}  {}  {}{}",
        DIM,
        pad_r("Price",   8),
        pad_l("Size",   10),
        pad_l("Value$", 10),
        pad_l("Cum$",   10),
        RESET,
    ));

    let mut cum = 0.0f64;
    for level in &asks {
        let val = level.price * level.size;
        cum += val;
        wl(buf, &format!("    {}  {}{}{}  {}  {}",
            pad_r(&format!("{:.4}", level.price), 8),
            color,
            pad_l(&format!("{:.3}", level.size), 10),
            RESET,
            pad_l(&format!("${:.2}", val),  10),
            pad_l(&format!("${:.2}", cum),  10),
        ));
    }
    if book.asks.len() > 5 {
        wl(buf, &format!("    {}… {} more levels{}", DIM, book.asks.len() - 5, RESET));
    }
}

fn render_ask_rows_filtered(buf: &mut String, levels: Vec<&OrderLevel>) {
    wl(buf, &format!("    {}{}  {}  {}{}",
        DIM,
        pad_r("Price", 8),
        pad_l("Size",  10),
        pad_l("Value$", 10),
        RESET,
    ));
    for lvl in levels.iter().take(5) {
        let val = lvl.price * lvl.size;
        wl(buf, &format!("    {}{}  {}  {}{}",
            YELLOW,
            pad_r(&format!("{:.4}", lvl.price), 8),
            pad_l(&format!("{:.3}", lvl.size),  10),
            pad_l(&format!("${:.2}", val),       10),
            RESET,
        ));
    }
    if levels.len() > 5 {
        wl(buf, &format!("    {}… {} more{}", DIM, levels.len() - 5, RESET));
    }
}

fn render_trades_table(buf: &mut String, trades: &[crate::types::BotTrade]) {
    wl(buf, &format!("    {}{}  {}  {}  {}  {}{}",
        DIM,
        pad_r("Time",   8),
        pad_r("Side",   5),
        pad_r("Shares", 8),
        pad_r("USDC",   8),
        "OrderID",
        RESET,
    ));
    for t in trades.iter().rev().take(10) {
        let ts    = t.ts.format("%H:%M:%S").to_string();
        let color = if t.outcome == "up" { GREEN } else { RED };
        let short = if t.order_id.len() > 14 { &t.order_id[..14] } else { &t.order_id };
        wl(buf, &format!("    {}  {}{}{}  {}  {}  {}",
            pad_r(&ts, 8),
            color, pad_r(&t.outcome.to_uppercase(), 5), RESET,
            pad_r(&format!("{:.3}", t.shares),     8),
            pad_r(&format!("${:.2}", t.usdc_spent), 8),
            short,
        ));
    }
}
