# poly-last-new

A high-performance Polymarket last-minute trading bot written in Rust. Monitors short-duration BTC/ETH price prediction markets (5-minute and 15-minute intervals), identifies the winner at market expiration using Chainlink oracle prices, and immediately buys the winning outcome shares before prices settle.

## Strategy

At market expiration, the bot compares the current BTC/ETH price to the "beat price" (the price when the market opened) via Chainlink oracles on Polygon. It then buys the winning outcome (UP or DOWN) at ask prices within milliseconds of the result being known.

## Features

- **Two execution modes**: paper trading (test) and live trading (real)
- **Chainlink oracle** price feeds with Binance REST API fallback
- **Polymarket CLOB** orderbook integration (builder API + standard API)
- **Real-time terminal UI** showing candles, orderbook, trades, and P&L
- **CSV trade logs** per market and aggregated P&L summary
- **State machine** managing full market lifecycle (active → ended → transitioning)

## Prerequisites

- [Rust toolchain](https://rustup.rs/) (edition 2021, stable)
- Network access to Polymarket APIs and a Polygon RPC endpoint

## Installation

```bash
git clone <repo>
cd poly-last-new
cargo build --release
```

## Configuration

Copy the example environment file and fill in your values:

```bash
cp .env.example .env
```

### Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `MARKET` | No | `btc-5m` | Market to trade: `btc-5m`, `btc-15m`, `eth-5m`, `eth-15m` |
| `ORDER_USDC` | No | `10.0` | USDC budget per trade after winner is confirmed |
| `SLIPPAGE_BUFFER` | No | `0.02` | Price slippage buffer added to ask (e.g. `0.02` = 2%) |
| `POLYGON_RPC_URL` | No | `https://polygon-rpc.com` | Polygon RPC endpoint for Chainlink oracle calls |
| `POLYMARKET_PRIVATE_KEY` | Live only | — | Wallet private key (`0x...`) for signing orders |
| `POLYMARKET_BUILDER_KEY` | Optional | — | Builder API key (UUID) — falls back to standard API if absent |
| `POLYMARKET_BUILDER_SECRET` | Optional | — | Builder API secret |
| `POLYMARKET_BUILDER_PASSPHRASE` | Optional | — | Builder API passphrase |

## Usage

### Paper Trading (no real orders)

```bash
cargo run --bin bot_test
```

Simulates fills at ask prices. Safe to run without any credentials. Ideal for testing strategy and validating the setup.

### Live Trading (real USDC orders)

```bash
cargo run --bin bot_real
```

Places real orders on the Polymarket CLOB. The bot waits 3 seconds on startup — press `Ctrl+C` to abort before trading begins.

**Requirements for live mode:**
- `POLYMARKET_PRIVATE_KEY` set in `.env`
- Sufficient USDC balance in the configured wallet on Polygon

### Production Build

```bash
cargo build --release
# Binaries at:
#   target/release/bot_test
#   target/release/bot_real
```

The release profile enables LTO, O3 optimization, and binary stripping for minimal latency.

## Project Structure

```
poly-last-new/
├── Cargo.toml
├── .env.example
├── data/                       # Auto-created; CSV logs written here
│   ├── <slug>_trades.csv
│   └── pnl_summary.csv
└── src/
    ├── bin/
    │   ├── bot_test.rs         # Paper trading entry point
    │   └── bot_real.rs         # Live trading entry point
    ├── api.rs                  # Polymarket Gamma + CLOB API calls
    ├── chainlink.rs            # Chainlink oracle price fetching
    ├── config.rs               # .env configuration loading
    ├── display.rs              # Terminal UI rendering
    ├── engine.rs               # Main trading loop & state machine
    ├── executor.rs             # Order execution (test vs. live)
    ├── csv_log.rs              # Trade and P&L CSV logging
    ├── types.rs                # Core data structures
    └── lib.rs                  # Module exports
```

## Data Logs

All logs are written to the `data/` directory (created automatically).

**Per-market trade log** (`data/<slug>_trades.csv`):

| Column | Description |
|---|---|
| `executed_at` | UTC timestamp of execution |
| `market_slug` | Market identifier |
| `outcome` | `UP` or `DOWN` |
| `shares` | Shares purchased |
| `usdc_spent` | USDC amount used |
| `fill_price` | Average fill price |
| `order_id` | Order/paper trade ID |
| `is_live` | `true` for live orders, `false` for paper |

**P&L summary** (`data/pnl_summary.csv`):

| Column | Description |
|---|---|
| `market_slug` | Market identifier |
| `beat_price` | Reference price at market open |
| `end_price` | Chainlink price at expiration |
| `winner` | Correct outcome |
| `our_outcome` | Outcome purchased |
| `shares_bought` | Total shares |
| `usdc_spent` | Total USDC spent |
| `pnl_usd` | Profit/loss in USD |
| `pnl_percentage` | Profit/loss as percentage |
| `resolved` | Whether market resolved |
| `executed_at` | Timestamp |
| `order_id` | Associated order ID |

## Supported Markets

| Slug | Asset | Duration | Chainlink Feed |
|---|---|---|---|
| `btc-5m` | BTC/USD | 5 minutes | `0xc907E116054Ad103354f2D350FD2514433D57F6f` |
| `btc-15m` | BTC/USD | 15 minutes | `0xc907E116054Ad103354f2D350FD2514433D57F6f` |
| `eth-5m` | ETH/USD | 5 minutes | `0xF9680D99D6C9589e2a93a78A04A279e509205945` |
| `eth-15m` | ETH/USD | 15 minutes | `0xF9680D99D6C9589e2a93a78A04A279e509205945` |

All feeds are on Polygon mainnet with 8 decimal precision.

## External APIs

| Service | Purpose |
|---|---|
| Polymarket Gamma API | Market metadata, token IDs, resolution |
| Polymarket CLOB API | Orderbook queries, order placement |
| Chainlink (Polygon) | Authoritative price at market end |
| Binance REST API | Price fallback if Chainlink RPC fails |

## Operational Notes

- **Latency**: The bot polls orderbooks every ~500ms. For production use, a private Polygon RPC (`POLYGON_RPC_URL`) is strongly recommended over the public default.
- **Wallet funding**: Live mode requires USDC in the wallet on Polygon. Ensure sufficient balance before running.
- **Continuous operation**: The bot runs 24/7, cycling through markets automatically. It only executes orders at market expiration.
- **Resource usage**: Single-threaded async (Tokio); minimal CPU and memory footprint.
- **Order constraints**: Minimum order size is $1 USDC; minimum lot size is 0.01 shares. Orders below these thresholds are skipped.

## Security

- Never commit your `.env` file or expose `POLYMARKET_PRIVATE_KEY`.
- Use a dedicated wallet with only the USDC needed for trading — do not use a primary wallet.
- Review `executor.rs` and `config.rs` before running in live mode to understand how credentials are used.

## License

See [LICENSE](LICENSE) if present, or contact the project maintainer.
