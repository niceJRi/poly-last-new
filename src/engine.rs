use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use reqwest::Client as HttpClient;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::api::{current_bucket_ts, fetch_market_resolution, fetch_orderbook, resolve_market};
use crate::price::{start_price_stream, SharedPrice};
use crate::config::Config;
use crate::csv_log;
use crate::display::render;
use crate::executor::Executor;
use crate::types::{BotTrade, BuyParams, CandleHistory, MarketMeta, MarketPhase, Orderbook};

// ── Shared app state ──────────────────────────────────────────────────────────

pub struct AppState {
    pub config: Config,
    pub is_live: bool,

    pub current_market: MarketMeta,
    pub cached_slug: String,

    pub btc_price: f64,
    pub beat_price: f64,  // RTDS price captured at the moment the market bucket started
    pub candles: CandleHistory,

    pub orderbook: Orderbook,

    pub phase: MarketPhase,
    pub bot_trades: Vec<BotTrade>,
    pub all_trades: Vec<BotTrade>,

    pub poll_count: u64,
    pub last_tick_ms: u64,
    pub status_line: String,

    // Price captured at the exact bucket boundary second — becomes the next market's beat price.
    pub next_beat_price: Option<f64>,

    // Last bucket timestamp seen — used to detect boundary crossings (0 = first run / startup).
    pub last_seen_bucket_ts: i64,

    // After-market snapshot for display during the 25-sec window
    pub post_market_orderbook: Option<Orderbook>,
    pub post_market_winner: String,
    pub post_market_end_price: f64,
    pub post_market_beat_price: f64,
    pub post_market_slug: String,
    pub post_market_trades: Vec<BotTrade>,
}

impl AppState {
    pub fn new(config: Config, is_live: bool) -> Self {
        AppState {
            config,
            is_live,
            current_market: MarketMeta::default(),
            cached_slug: String::new(),
            btc_price: 0.0,
            beat_price: 0.0,
            candles: CandleHistory::new(3),
            orderbook: Orderbook::default(),
            phase: MarketPhase::Active,
            bot_trades: Vec::new(),
            all_trades: Vec::new(),
            poll_count: 0,
            last_tick_ms: 0,
            status_line: "Initializing… connecting to Polymarket RTDS".to_string(),
            next_beat_price: None,
            last_seen_bucket_ts: 0,
            post_market_orderbook: None,
            post_market_winner: String::new(),
            post_market_end_price: 0.0,
            post_market_beat_price: 0.0,
            post_market_slug: String::new(),
            post_market_trades: Vec::new(),
        }
    }
}

// ── Snapshot for render task ──────────────────────────────────────────────────

pub struct RenderState {
    pub config: Config,
    pub is_live: bool,
    pub current_market: MarketMeta,
    pub btc_price: f64,
    pub beat_price: f64,
    pub candles: CandleHistory,
    pub orderbook: Orderbook,
    pub phase: MarketPhase,
    pub bot_trades: Vec<BotTrade>,
    pub all_trades: Vec<BotTrade>,
    pub poll_count: u64,
    pub last_tick_ms: u64,
    pub status_line: String,
    pub post_market_orderbook: Option<Orderbook>,
    pub post_market_winner: String,
    pub post_market_end_price: f64,
    pub post_market_beat_price: f64,
    pub post_market_slug: String,
    pub post_market_trades: Vec<BotTrade>,
}

impl RenderState {
    fn from_app(s: &AppState) -> Self {
        RenderState {
            config: s.config.clone(),
            is_live: s.is_live,
            current_market: s.current_market.clone(),
            btc_price: s.btc_price,
            beat_price: s.beat_price,
            candles: CandleHistory {
                candles: s.candles.candles.clone(),
                max_candles: s.candles.max_candles,
                current_candle: s.candles.current_candle.clone(),
            },
            orderbook: s.orderbook.clone(),
            phase: s.phase.clone(),
            bot_trades: s.bot_trades.clone(),
            all_trades: s.all_trades.clone(),
            poll_count: s.poll_count,
            last_tick_ms: s.last_tick_ms,
            status_line: s.status_line.clone(),
            post_market_orderbook: s.post_market_orderbook.clone(),
            post_market_winner: s.post_market_winner.clone(),
            post_market_end_price: s.post_market_end_price,
            post_market_beat_price: s.post_market_beat_price,
            post_market_slug: s.post_market_slug.clone(),
            post_market_trades: s.post_market_trades.clone(),
        }
    }
}

// ── Main engine ───────────────────────────────────────────────────────────────

pub async fn run(mut state: AppState, executor: Arc<dyn Executor>) -> Result<()> {
    let http = HttpClient::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // Start background Polymarket RTDS WebSocket (updates SharedPrice in real-time)
    let price_stream: SharedPrice = start_price_stream(http.clone(), &state.config.asset);

    // Render task — redraws terminal every 500 ms
    let render_arc: Arc<Mutex<Option<RenderState>>> = Arc::new(Mutex::new(None));
    let render_clone = Arc::clone(&render_arc);
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_millis(500)).await;
            let guard = render_clone.lock().await;
            if let Some(rs) = guard.as_ref() {
                render(rs);
            }
        }
    });

    loop {
        state.poll_count += 1;
        let t0 = Instant::now();

        let current_bucket = current_bucket_ts(state.config.interval_secs);
        if current_bucket > state.last_seen_bucket_ts {
            state.last_seen_bucket_ts = current_bucket;
            state.next_beat_price = None; // discard stale boundary value; look fresh for this second
        }

        {
            let history = price_stream.lock().await;
            let p = history.current_price();
            if p > 0.0 {
                state.btc_price = p;
                state.candles.update(p);
            }
            // Match on Chainlink oracle timestamp exactly — same logic as the JS RTDS recorder.
            // Polled every 200ms until the oracle delivers the round whose chainlink_ts ==
            // the boundary second (e.g. 01:30:00).  No wall-clock fudge needed.
            if state.next_beat_price.is_none() && state.last_seen_bucket_ts > 0 {
                if let Some(bp) = history.price_exact(state.last_seen_bucket_ts) {
                    state.next_beat_price = Some(bp);
                    // If the market just ended before the oracle delivered this round,
                    // patch post_market_end_price and recalculate the winner now.
                    if matches!(state.phase, MarketPhase::JustEnded { .. }) {
                        state.post_market_end_price = bp;
                        if state.post_market_beat_price > 0.0 {
                            state.post_market_winner =
                                if bp > state.post_market_beat_price { "up" } else { "down" }
                                .to_string();
                        }
                    }
                }
            }
        }

        if let Err(e) = tick(&mut state, &http, &executor).await {
            state.status_line = format!("Error: {}", e);
        }

        // Startup catch: bot started mid-market so no boundary crossing was seen.
        // Once RTDS delivers a valid price, use it as the beat price immediately.
        // Fires exactly once (beat_price stays > 0 afterwards).
        if state.beat_price == 0.0
            && state.btc_price > 0.0
            && !state.current_market.is_empty()
        {
            state.beat_price = state.btc_price;
        }

        state.last_tick_ms = t0.elapsed().as_millis() as u64;

        {
            let mut guard = render_arc.lock().await;
            *guard = Some(RenderState::from_app(&state));
        }

        sleep(Duration::from_millis(state.config.poll_ms)).await;
    }
}

// ── Per-tick logic ────────────────────────────────────────────────────────────

async fn tick(
    state: &mut AppState,
    http: &HttpClient,
    executor: &Arc<dyn Executor>,
) -> Result<()> {
    let cfg = &state.config;
    let new_slug = format!("{}-{}", cfg.slug_prefix, current_bucket_ts(cfg.interval_secs));

    // ── Phase state machine ───────────────────────────────────────────────────
    match state.phase.clone() {
        MarketPhase::Active => {
            tick_active(state, http).await?;
        }

        MarketPhase::JustEnded { ended_at, winner, .. } => {
            let elapsed = Utc::now().timestamp() - ended_at;
            if elapsed >= state.config.post_market_secs as i64 {
                state.phase = MarketPhase::Transitioning;
                state.status_line = "Post-market window closed, loading next market…".to_string();
            } else {
                let w = winner.clone();
                // Use post_market_end_price which may have been patched once the
                // oracle delivered the boundary-second Chainlink round.
                let end_p = state.post_market_end_price;
                tick_just_ended(state, http, executor, &w, end_p, ended_at).await?;
            }
        }

        MarketPhase::Transitioning => {
            state.status_line = "Waiting for next market…".to_string();
        }
    }

    // ── Market transition (held until the 25-sec window closes) ──────────────
    let in_window = matches!(state.phase, MarketPhase::JustEnded { .. });
    if new_slug != state.cached_slug && !in_window {
        handle_market_transition(state, new_slug, http).await?;
    }

    Ok(())
}

// ── Market transition ─────────────────────────────────────────────────────────

async fn handle_market_transition(
    state: &mut AppState,
    new_slug: String,
    http: &HttpClient,
) -> Result<()> {
    if !state.cached_slug.is_empty() {
        try_finalize_pnl(state, http).await;
        state.phase = MarketPhase::Active;
        state.bot_trades.clear();
        state.post_market_orderbook = None;
    }

    let meta = resolve_market(http, &state.config.slug_prefix, state.config.interval_secs).await?;

    // Beat price comes from the RTDS snapshot taken at the prior market's end.
    // 0.0 means unknown (bot started mid-market; display shows "-", no trading).
    let beat_price = state.next_beat_price
        .take()
        .filter(|&p| p > 0.0)
        .unwrap_or(0.0);

    state.beat_price     = beat_price;
    state.current_market = meta;
    state.cached_slug    = new_slug;
    state.candles        = CandleHistory::new(3);

    state.status_line = format!(
        "New market: {}  Beat: ${:.2}", state.cached_slug, state.beat_price
    );
    Ok(())
}

// ── Active market tick ────────────────────────────────────────────────────────

async fn tick_active(state: &mut AppState, http: &HttpClient) -> Result<()> {
    // Detect market end using the bucket clock — accurate to the second.
    // `end_time` from the Gamma API is sometimes 1s past the true boundary,
    // so `has_ended()` would fire one second late and read the wrong RTDS price.
    let cur_bucket = current_bucket_ts(state.config.interval_secs);
    let market_ended = state.current_market.event_start_time > 0
        && cur_bucket > state.current_market.event_start_time;

    if market_ended {
        let end_price = state.next_beat_price.unwrap_or(state.btc_price);
        eprintln!("[WINNER] beat={:.2} end={:.2}", state.beat_price, end_price);
        let winner = if state.beat_price > 0.0 && end_price > state.beat_price { "up" } else { "down" };

        state.post_market_winner     = winner.to_string();
        state.post_market_end_price  = end_price;
        state.post_market_beat_price = state.beat_price;
        state.post_market_slug       = state.current_market.slug.clone();
        state.post_market_trades     = state.bot_trades.clone();

        state.phase = MarketPhase::JustEnded {
            ended_at:      Utc::now().timestamp(),
            winner:        winner.to_string(),
            end_btc_price: end_price,
        };

        state.status_line = format!(
            "Market ENDED!  Winner: {}  Beat: ${:.2}  End: ${:.2}  Scanning orderbook…",
            winner.to_uppercase(), state.beat_price, end_price,
        );
        return Ok(());
    }

    // Fetch orderbook for live display
    match fetch_orderbook(http, &state.current_market).await {
        Ok(ob) => state.orderbook = ob,
        Err(e) => eprintln!("[WARN] orderbook fetch: {}", e),
    }

    let secs_left = state.current_market.seconds_until_end();

    state.status_line = if state.beat_price > 0.0 {
        let delta = state.btc_price - state.beat_price;
        let pct   = delta / state.beat_price * 100.0;
        let dir   = if state.btc_price > state.beat_price { "↑ UP" } else { "↓ DOWN" };
        format!(
            "Active  {:02}:{:02} left  BTC ${:.2}  Beat ${:.2}  Δ{:+.2} ({:+.3}%)  {}",
            secs_left / 60, secs_left % 60,
            state.btc_price, state.beat_price, delta, pct, dir,
        )
    } else {
        format!(
            "Active  {:02}:{:02} left  BTC ${:.2}  Beat: N/A (started mid-market)",
            secs_left / 60, secs_left % 60, state.btc_price,
        )
    };
    Ok(())
}

// ── Just-ended tick ───────────────────────────────────────────────────────────

async fn tick_just_ended(
    state: &mut AppState,
    http: &HttpClient,
    executor: &Arc<dyn Executor>,
    winner: &str,
    end_price: f64,
    ended_at: i64,
) -> Result<()> {
    let elapsed   = Utc::now().timestamp() - ended_at;
    let remaining = (state.config.post_market_secs as i64 - elapsed).max(0);

    // If the bot started mid-market we never had a valid beat price for this
    // market, so skip trading and just wait for the next market.
    if state.beat_price == 0.0 {
        state.status_line = format!(
            "Skipping — beat price unknown (started mid-market)  Window: {}s", remaining,
        );
        return Ok(());
    }

    let max = state.config.max_trades_per_market;
    if max > 0 && state.bot_trades.len() >= max {
        state.status_line = format!(
            "Winner: {}  Max trades reached ({}/{})  Window: {}s",
            winner.to_uppercase(), state.bot_trades.len(), max, remaining,
        );
        return Ok(());
    }

    // Refresh orderbook every tick during the window
    let ob = match fetch_orderbook(http, &state.current_market).await {
        Ok(o) => o,
        Err(e) => {
            state.status_line = format!(
                "Winner: {}  Orderbook fetch failed: {}  Window: {}s",
                winner.to_uppercase(), e, remaining,
            );
            return Ok(());
        }
    };

    let winner_token = if winner == "up" {
        state.current_market.up_token_id.clone()
    } else {
        state.current_market.down_token_id.clone()
    };

    let winner_book = if winner == "up" { &ob.up } else { &ob.down };

    // Update display orderbook
    state.post_market_orderbook = Some(ob.clone());
    state.orderbook = ob.clone();

    let tradeable: Vec<_> = winner_book.asks.iter()
        .filter(|a| a.price < 1.0)
        .cloned()
        .collect();

    if tradeable.is_empty() {
        state.status_line = format!(
            "Winner: {}  No asks below $1.00  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), state.beat_price, end_price, remaining,
        );
        return Ok(());
    }

    // FOK order logic: try one ask level at a time.
    // On failure, immediately retry at index + ORDER_LEVEL_SKIP (next price level up).
    // FOK matches the whole book at or below the limit price, so don't cap by single-level size.
    let budget = state.config.order_usdc;
    let skip   = state.config.order_level_skip.max(1);
    let mut idx = 0;
    let mut trade_placed: Option<BotTrade> = None;

    while idx < tradeable.len() {
        let ask = &tradeable[idx];

        let shares_to_buy = budget / ask.price;
        if shares_to_buy * ask.price < 1.0 {
            idx += skip;
            continue;
        }

        let params = BuyParams {
            token_id:        winner_token.clone(),
            outcome:         winner.to_string(),
            shares:          shares_to_buy,
            ask_price:       ask.price,
            slippage_buffer: state.config.slippage_buffer,
        };

        match executor.execute_buy(&params).await {
            Ok(res) => {
                let trade = BotTrade {
                    ts:          Utc::now(),
                    market_slug: state.current_market.slug.clone(),
                    outcome:     winner.to_string(),
                    shares:      res.shares,
                    usdc_spent:  res.usdc,
                    fill_price:  res.fill_price,
                    order_id:    res.order_id.clone(),
                    is_live:     executor.is_live(),
                };
                if let Err(e) = csv_log::append_trade(&trade) {
                    eprintln!("[CSV] write error: {}", e);
                }
                state.bot_trades.push(trade.clone());
                state.all_trades.push(trade.clone());
                state.post_market_trades = state.bot_trades.clone();
                trade_placed = Some(trade);
                break; // one fill per tick
            }
            Err(e) => {
                let err_str = e.to_string();
                if executor.is_live() {
                    let ctx = format!(
                        "outcome={} shares={:.2} ask={:.4}",
                        winner, params.shares, params.ask_price,
                    );
                    if let Err(le) = csv_log::append_order_error(&state.config.market, &ctx, &err_str) {
                        eprintln!("[LOG] error log write failed: {}", le);
                    }
                }
                eprintln!("[ORDER] failed ask[{}]={:.4}: {}", idx, ask.price, err_str);
                idx += skip; // move to next price level immediately
            }
        }
    }

    state.status_line = if let Some(ref t) = trade_placed {
        format!(
            "Winner: {}  Bought {:.3} shares for ${:.2}  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), t.shares, t.usdc_spent,
            state.beat_price, end_price, remaining,
        )
    } else {
        format!(
            "Winner: {}  No fill (tried {} levels)  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), tradeable.len(),
            state.beat_price, end_price, remaining,
        )
    };

    Ok(())
}

// ── PnL finalization ──────────────────────────────────────────────────────────

async fn try_finalize_pnl(state: &mut AppState, http: &HttpClient) {
    if state.bot_trades.is_empty() { return; }

    let slug   = state.current_market.slug.clone();
    let winner = match fetch_market_resolution(http, &slug).await {
        Ok(Some(w)) => w,
        _            => state.post_market_winner.clone(),
    };

    for trade in &state.bot_trades {
        let pnl = if trade.outcome == winner {
            trade.shares - trade.usdc_spent
        } else {
            -trade.usdc_spent
        };
        if let Err(e) = csv_log::append_pnl_row(
            &slug,
            state.post_market_beat_price,
            state.post_market_end_price,
            &winner,
            trade,
            pnl,
            true,
        ) {
            eprintln!("[CSV] pnl write error: {}", e);
        }
    }
}
