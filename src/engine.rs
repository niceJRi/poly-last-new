use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use chrono::Utc;
use reqwest::Client as HttpClient;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::api::{current_bucket_ts, fetch_market_resolution, fetch_orderbook, resolve_market};
use crate::chainlink::{fetch_price_fallback, PriceClient};
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
    pub beat_price: f64,   // Chainlink price at market start
    pub candles: CandleHistory,

    pub orderbook: Orderbook,

    pub phase: MarketPhase,
    pub bot_trades: Vec<BotTrade>,   // trades this market
    pub all_trades: Vec<BotTrade>,   // all-time trades this session

    pub poll_count: u64,
    pub last_tick_ms: u64,
    pub status_line: String,

    // After-market snapshot for display
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
            status_line: "Initializing...".to_string(),
            post_market_orderbook: None,
            post_market_winner: String::new(),
            post_market_end_price: 0.0,
            post_market_beat_price: 0.0,
            post_market_slug: String::new(),
            post_market_trades: Vec::new(),
        }
    }
}

// ── Shared render state (cloned for render task) ──────────────────────────────

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

    let oracle = PriceClient::new(
        http.clone(),
        state.config.ds_api_key.clone(),
        state.config.ds_api_secret.clone(),
        state.config.ds_feed_id.clone(),
        state.config.polygon_rpc_url.clone(),
        state.config.chainlink_feed.clone(),
    );

    // Spawn render task (redraws terminal every second)
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

        // Fetch price: Chainlink Data Streams → on-chain aggregator → Binance
        match oracle.latest_price().await {
            Ok(p) => {
                state.btc_price = p;
                state.candles.update(p);
            }
            Err(e) => {
                match fetch_price_fallback(&http, &state.config.asset).await {
                    Ok(p) => {
                        state.btc_price = p;
                        state.candles.update(p);
                    }
                    Err(_) => {
                        state.status_line = format!("Price fetch failed: {}", e);
                    }
                }
            }
        }

        if let Err(e) = tick(&mut state, &http, &oracle, &executor).await {
            state.status_line = format!("Error: {}", e);
        }

        state.last_tick_ms = t0.elapsed().as_millis() as u64;

        // Push snapshot to render task
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
    oracle: &PriceClient,
    executor: &Arc<dyn Executor>,
) -> Result<()> {
    let cfg = &state.config;
    let slug = format!("{}-{}", cfg.slug_prefix, current_bucket_ts(cfg.interval_secs));

    // ── Phase state machine runs FIRST ────────────────────────────────────────
    // This must come before the slug check so that when the market ends and the
    // slug changes simultaneously, tick_active sets JustEnded before we decide
    // whether to load the next market.
    match state.phase.clone() {
        MarketPhase::Active => {
            tick_active(state, http).await?;
        }

        MarketPhase::JustEnded { ended_at, winner, end_btc_price } => {
            let elapsed = Utc::now().timestamp() - ended_at;

            if elapsed >= state.config.post_market_secs as i64 {
                // Trading window expired → allow slug check below to load next market
                state.phase = MarketPhase::Transitioning;
                state.status_line = "Market window closed, loading next market...".to_string();
            } else {
                // Keep scanning for winner asks below $1.00 every tick during the window
                let winner_clone = winner.clone();
                let end_price = end_btc_price;
                tick_just_ended(state, http, executor, &winner_clone, end_price, ended_at).await?;
            }
        }

        MarketPhase::Transitioning => {
            state.status_line = "Waiting for next market...".to_string();
        }
    }

    // ── Market transition: skip while JustEnded trading window is active ──────
    // The new market's slug is already valid, but we hold off loading it until
    // the post-market buy window on the OLD market finishes.
    let in_trading_window = matches!(state.phase, MarketPhase::JustEnded { .. });
    if slug != state.cached_slug && !in_trading_window {
        handle_market_transition(state, slug.clone(), http, oracle).await?;
    }

    Ok(())
}

// ── Market transition handler ─────────────────────────────────────────────────

async fn handle_market_transition(
    state: &mut AppState,
    new_slug: String,
    http: &HttpClient,
    oracle: &PriceClient,
) -> Result<()> {
    // If we just transitioned out of a market, try to fetch PnL info
    if !state.cached_slug.is_empty() {
        try_finalize_pnl(state, http).await;
        state.phase = MarketPhase::Active;
        state.bot_trades.clear();
        state.post_market_orderbook = None;
    }

    // Resolve new market
    let meta = resolve_market(http, &state.config.slug_prefix, state.config.interval_secs).await?;

    // Fetch the exact beat price from Data Streams at event_start_time.
    // This matches the price Polymarket uses for resolution.
    // Falls back to: text-parsed price → current live price.
    let beat_price = if oracle.has_data_streams() {
        match oracle.price_at(meta.event_start_time).await {
            Ok(p) => {
                state.status_line = format!(
                    "Beat price from DS at {}: ${:.2}", meta.event_start_time, p
                );
                p
            }
            Err(e) => {
                eprintln!("[DS] historical beat price failed: {e}");
                meta.beat_price.unwrap_or(state.btc_price)
            }
        }
    } else {
        meta.beat_price.unwrap_or(state.btc_price)
    };

    state.beat_price     = if beat_price > 0.0 { beat_price } else { state.btc_price };
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
    // Check if market has ended
    if state.current_market.has_ended() {
        let end_price = state.btc_price;
        let winner = if end_price > state.beat_price { "up" } else { "down" };

        // Snapshot post-market state
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
            "Market ENDED! Winner: {}  Beat: ${:.2}  End: ${:.2}  Fetching orderbook...",
            winner.to_uppercase(),
            state.beat_price,
            end_price,
        );
        return Ok(());
    }

    // Fetch orderbook for display
    match fetch_orderbook(http, &state.current_market).await {
        Ok(ob) => state.orderbook = ob,
        Err(e) => eprintln!("[WARN] orderbook fetch failed: {}", e),
    }

    let secs_left = state.current_market.seconds_until_end();
    let direction = if state.btc_price > state.beat_price { "↑ UP" } else { "↓ DOWN" };
    let delta     = state.btc_price - state.beat_price;
    let pct       = if state.beat_price > 0.0 { delta / state.beat_price * 100.0 } else { 0.0 };

    state.status_line = format!(
        "Active  {:02}:{:02} left  BTC ${:.2}  Beat ${:.2}  Δ{:+.2} ({:+.3}%)  {}",
        secs_left / 60, secs_left % 60,
        state.btc_price, state.beat_price, delta, pct, direction,
    );

    Ok(())
}

// ── Just-ended tick: buy winner asks below $1.00, runs every tick during window ─

async fn tick_just_ended(
    state: &mut AppState,
    http: &HttpClient,
    executor: &Arc<dyn Executor>,
    winner: &str,
    end_price: f64,
    ended_at: i64,
) -> Result<()> {
    let elapsed = Utc::now().timestamp() - ended_at;
    let remaining = (state.config.post_market_secs as i64 - elapsed).max(0);

    // Stop trading if we've hit the per-market cap
    let max = state.config.max_trades_per_market;
    if max > 0 && state.bot_trades.len() >= max {
        state.status_line = format!(
            "Winner: {}  Max trades reached ({}/{})  Window: {}s",
            winner.to_uppercase(), state.bot_trades.len(), max, remaining,
        );
        return Ok(());
    }

    // Fetch current orderbook for winner side
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

    let winner_book = if winner == "up" { &ob.up } else { &ob.down };
    let winner_token = if winner == "up" {
        state.current_market.up_token_id.clone()
    } else {
        state.current_market.down_token_id.clone()
    };

    state.post_market_orderbook = Some(ob.clone());

    // Only buy asks strictly below $1.00 (profitable fills)
    let tradeable_asks: Vec<_> = winner_book.asks.iter()
        .filter(|ask| ask.price < 1.0)
        .cloned()
        .collect();

    if tradeable_asks.is_empty() {
        state.status_line = format!(
            "Winner: {}  No asks below $1.00  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), state.beat_price, end_price, remaining,
        );
        return Ok(());
    }

    // Walk asks cheapest first, spend up to ORDER_USDC budget this tick
    let budget = state.config.order_usdc;
    let mut remaining_budget = budget;
    let mut orders_placed = 0;
    let mut total_shares_bought = 0.0f64;
    let mut total_usdc_spent    = 0.0f64;

    for ask in &tradeable_asks {
        if remaining_budget < 1.0 { break; }

        let max_shares_at_price = remaining_budget / ask.price;
        let shares_to_buy = max_shares_at_price.min(ask.size);
        let cost = shares_to_buy * ask.price;

        if cost < 1.0 { break; }

        let params = BuyParams {
            token_id:        winner_token.clone(),
            outcome:         winner.to_string(),
            shares:          shares_to_buy,
            ask_price:       ask.price,
            slippage_buffer: state.config.slippage_buffer,
        };

        match executor.execute_buy(&params).await {
            Ok(result) => {
                let trade = BotTrade {
                    ts:          Utc::now(),
                    market_slug: state.current_market.slug.clone(),
                    outcome:     winner.to_string(),
                    shares:      result.shares,
                    usdc_spent:  result.usdc,
                    fill_price:  result.fill_price,
                    order_id:    result.order_id.clone(),
                    is_live:     executor.is_live(),
                };

                if let Err(e) = csv_log::append_trade(&trade) {
                    eprintln!("[CSV] write error: {}", e);
                }

                remaining_budget    -= result.usdc;
                total_shares_bought += result.shares;
                total_usdc_spent    += result.usdc;
                orders_placed       += 1;

                state.bot_trades.push(trade.clone());
                state.all_trades.push(trade);
                state.post_market_trades = state.bot_trades.clone();
            }
            Err(e) => {
                eprintln!("[ORDER] execute_buy failed: {}", e);
            }
        }
    }

    state.status_line = if orders_placed > 0 {
        format!(
            "Winner: {}  Bought {:.3} shares for ${:.2} in {} order(s)  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(),
            total_shares_bought, total_usdc_spent, orders_placed,
            state.beat_price, end_price, remaining,
        )
    } else {
        format!(
            "Winner: {}  Asks present but none filled (below $1 min)  Beat: ${:.2}  End: ${:.2}  Window: {}s",
            winner.to_uppercase(), state.beat_price, end_price, remaining,
        )
    };

    Ok(())
}

// ── PnL finalization ──────────────────────────────────────────────────────────

async fn try_finalize_pnl(state: &mut AppState, http: &HttpClient) {
    if state.bot_trades.is_empty() { return; }

    let slug = state.current_market.slug.clone();
    let winner = match fetch_market_resolution(http, &slug).await {
        Ok(Some(w)) => w,
        _ => state.post_market_winner.clone(),
    };

    for trade in &state.bot_trades {
        let pnl = if trade.outcome == winner {
            trade.shares * 1.0 - trade.usdc_spent
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
