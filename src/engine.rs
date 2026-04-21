use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use reqwest::Client as HttpClient;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::api::{current_bucket_ts, fetch_market_resolution, fetch_orderbook, resolve_market};
use crate::binance::{fetch_price_rest, start_price_stream, SharedPrice};
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
    pub beat_price: f64,  // Binance price captured at the moment the market bucket started
    pub candles: CandleHistory,

    pub orderbook: Orderbook,

    pub phase: MarketPhase,
    pub bot_trades: Vec<BotTrade>,
    pub all_trades: Vec<BotTrade>,

    pub poll_count: u64,
    pub last_tick_ms: u64,
    pub status_line: String,

    // Beat price pre-captured when the next market bucket is first detected.
    // This is set even while the 25-sec post-market window is still running,
    // so the new market starts with the price from exactly its bucket boundary.
    pub next_beat_price: Option<f64>,

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
            status_line: "Initializing… connecting to Binance".to_string(),
            next_beat_price: None,
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

    // Grab initial price via REST while the poll loop warms up
    match fetch_price_rest(&http, &state.config.asset).await {
        Ok(p) => {
            state.btc_price = p;
            state.candles.update(p);
            state.status_line = format!(
                "Binance initial price: ${:.2} — starting stream…", p
            );
        }
        Err(e) => eprintln!("[Binance] initial REST fetch failed: {e}"),
    }

    // Start background 250 ms poll loop (updates SharedPrice in real-time)
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

        // Pull latest Binance price from shared stream
        {
            let p = *price_stream.lock().await;
            if p > 0.0 {
                state.btc_price = p;
                state.candles.update(p);
            }
        }

        if let Err(e) = tick(&mut state, &http, &executor).await {
            state.status_line = format!("Error: {}", e);
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

    // ── Beat-price capture: runs even during the 25-sec window ────────────────
    // The slug for the NEXT market becomes valid at the exact bucket boundary.
    // We snapshot the Binance price at that moment so the new market's
    // "price to beat" is the true starting price, not the post-window price.
    if !new_slug.is_empty()
        && new_slug != state.cached_slug
        && !state.cached_slug.is_empty()
        && state.next_beat_price.is_none()
    {
        state.next_beat_price = Some(state.btc_price);
        eprintln!(
            "[Engine] Saved next beat price ${:.2} for {}",
            state.btc_price, new_slug
        );
    }

    // ── Phase state machine ───────────────────────────────────────────────────
    match state.phase.clone() {
        MarketPhase::Active => {
            tick_active(state, http).await?;
        }

        MarketPhase::JustEnded { ended_at, winner, end_btc_price } => {
            let elapsed = Utc::now().timestamp() - ended_at;
            if elapsed >= state.config.post_market_secs as i64 {
                state.phase = MarketPhase::Transitioning;
                state.status_line = "Post-market window closed, loading next market…".to_string();
            } else {
                let w = winner.clone();
                tick_just_ended(state, http, executor, &w, end_btc_price, ended_at).await?;
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

    // Use the beat price captured at the exact bucket boundary (even if captured
    // during the 25-sec window).  Fall back to current price only if not set.
    let beat_price = state.next_beat_price
        .take()
        .filter(|&p| p > 0.0)
        .unwrap_or(state.btc_price);

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
    if state.current_market.has_ended() {
        let end_price = state.btc_price;
        let winner    = if end_price > state.beat_price { "up" } else { "down" };

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
    let delta     = state.btc_price - state.beat_price;
    let pct       = if state.beat_price > 0.0 { delta / state.beat_price * 100.0 } else { 0.0 };
    let dir       = if state.btc_price > state.beat_price { "↑ UP" } else { "↓ DOWN" };

    state.status_line = format!(
        "Active  {:02}:{:02} left  BTC ${:.2}  Beat ${:.2}  Δ{:+.2} ({:+.3}%)  {}",
        secs_left / 60, secs_left % 60,
        state.btc_price, state.beat_price, delta, pct, dir,
    );
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

    // Walk asks cheapest-first, spend up to TRADE_AMOUNT budget per tick
    let budget = state.config.order_usdc;
    let mut rem_budget          = budget;
    let mut orders_placed       = 0usize;
    let mut total_shares_bought = 0.0f64;
    let mut total_usdc_spent    = 0.0f64;

    for ask in &tradeable {
        if rem_budget < 1.0 { break; }

        let shares_to_buy = (rem_budget / ask.price).min(ask.size);
        let cost          = shares_to_buy * ask.price;
        if cost < 1.0 { break; }

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
                rem_budget          -= res.usdc;
                total_shares_bought += res.shares;
                total_usdc_spent    += res.usdc;
                orders_placed       += 1;
                state.bot_trades.push(trade.clone());
                state.all_trades.push(trade);
                state.post_market_trades = state.bot_trades.clone();
            }
            Err(e) => eprintln!("[ORDER] execute_buy failed: {}", e),
        }
    }

    state.status_line = if orders_placed > 0 {
        format!(
            "Winner: {}  Bought {:.3} shares for ${:.2} ({} order(s))  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), total_shares_bought, total_usdc_spent, orders_placed,
            state.beat_price, end_price, remaining,
        )
    } else {
        format!(
            "Winner: {}  Asks present but below $1 min order  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), state.beat_price, end_price, remaining,
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
