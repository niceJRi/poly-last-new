# poly-last-new

A Polymarket last-minute trading bot written in Rust. Monitors short-duration BTC/ETH price prediction markets (5-minute and 15-minute intervals), determines the winner at market expiration using a live Binance price stream, and immediately buys winning-outcome shares during a 25-second post-market window.

## Strategy

When a market ends, the bot compares the **end price** (current Binance price) to the **beat price** (the Binance price captured at the exact moment the market's bucket timestamp started). If the end price is higher, UP wins; otherwise DOWN wins. The bot then scans the winner-side orderbook for asks below $1.00 and buys them — these shares resolve to $1.00, yielding profit.

The beat price is captured at the precise second the new market bucket starts, **even if the previous market's 25-second trading window is still open**. This ensures the beat price always matches the true market starting price.

## Features

- **Binance live price stream** — polls `api.binance.com` every 250 ms, no API key required
- **Accurate beat price** — captured at the bucket boundary timestamp, not at transition time
- **25-second post-market window** — scans and trades winner-side asks after market ends
- **Two execution modes** — paper trading (test) and live trading (real)
- **Real-time terminal UI** — beat price, current price, difference, 5 ask levels for UP and DOWN
- **Per-market CSV trade logs** in `data/` folder
- **PnL calculator binary** — `cargo run --bin pnl` sums actual profit/loss from all CSV files

## Environment Setup

### 1. Install Rust

If Rust is not already installed, use `rustup` (the official installer):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the on-screen prompts (the default installation is fine). Then activate the toolchain in your current shell:

```bash
source "$HOME/.cargo/env"
```

Verify the installation:

```bash
rustc --version   # e.g. rustc 1.78.0
cargo --version   # e.g. cargo 1.78.0
```

The project requires **stable Rust** (edition 2021). To update an existing installation:

```bash
rustup update stable
```

### 2. System Dependencies

The project uses `rustls` with the AWS-LC crypto provider, which is bundled and requires no extra system libraries. No OpenSSL or Polygon RPC credentials are needed.

On **Ubuntu / Debian** you may need the C build toolchain if it is not present:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config
```

On **macOS** the Xcode command-line tools cover this:

```bash
xcode-select --install
```

### 3. Clone and Build

```bash
git clone <repo>
cd poly-last-new
cargo build --release
```

Compiled binaries will be placed in `target/release/`.

### 4. Create Your `.env` File

```bash
cp .env.example .env
```

Open `.env` and set at minimum:

```env
MARKET=btc-5m          # btc-5m | btc-15m | eth-5m | eth-15m
TRADE_AMOUNT=10.0      # USDC to spend per trade
MAX_TRADES=3           # max orders per post-market window (0 = unlimited)
SLIPPAGE_BUFFER=0.02   # price buffer added to ask price
```

For **live trading** also add your Polymarket credentials:

```env
POLYMARKET_PRIVATE_KEY=0x...
POLYMARKET_BUILDER_KEY=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
POLYMARKET_BUILDER_SECRET=...
POLYMARKET_BUILDER_PASSPHRASE=...
```

### 5. Verify Setup (paper mode)

Run the paper-trading bot to confirm everything connects correctly:

```bash
cargo run --bin bot_test
```

You should see the terminal UI appear with live BTC/ETH price data within a few seconds.

## Installation

```bash
git clone <repo>
cd poly-last-new
cargo build --release
```

### Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `MARKET` | No | `btc-5m` | Market to trade: `btc-5m`, `btc-15m`, `eth-5m`, `eth-15m` |
| `TRADE_AMOUNT` | No | `10.0` | USDC to spend per trade after winner is confirmed |
| `MAX_TRADES` | No | `0` | Max orders per market (0 = unlimited) |
| `SLIPPAGE_BUFFER` | No | `0.02` | Price buffer added to ask when placing orders (e.g. `0.02` = 2 cents) |
| `POLYMARKET_PRIVATE_KEY` | Live only | — | Wallet private key (`0x…`) for signing orders |
| `POLYMARKET_BUILDER_KEY` | Optional | — | Builder API key (UUID) — falls back to standard API if absent |
| `POLYMARKET_BUILDER_SECRET` | Optional | — | Builder API secret |
| `POLYMARKET_BUILDER_PASSPHRASE` | Optional | — | Builder API passphrase |

> `ORDER_USDC` and `MAX_TRADES_PER_MARKET` are accepted as backward-compatible aliases for `TRADE_AMOUNT` and `MAX_TRADES`.

## Usage

### Paper Trading (no real orders)

```bash
cargo run --bin bot_test
```

Simulates fills at ask prices with no credentials required. Good for verifying strategy and configuration.

### Live Trading (real USDC orders)

```bash
cargo run --bin bot_real
```

Places real FOK limit orders on the Polymarket CLOB. Waits 3 seconds on startup — press `Ctrl+C` to abort.

**Requirements for live mode:**
- `POLYMARKET_PRIVATE_KEY` set in `.env`
- Sufficient USDC in the configured wallet on Polygon

### PnL Calculator

```bash
cargo run --bin pnl
```

Reads `data/pnl_summary.csv` and prints a per-market breakdown with total PnL, win rate, and ROI.

### Production Build

```bash
cargo build --release
# target/release/bot_test
# target/release/bot_real
# target/release/pnl
```

## Terminal Display

During operation the terminal shows:

```
  ──── BTC Price vs Beat Price ──────────────────────────────
  Beat price   :      $83,450.00
  Current price:      $83,525.00
  Difference   :  +$75.00  (+0.090%)  →  UP ↑

  ──── Orderbook — 5 Ask Levels ─────────────────────────────
  UP   asks — 5 level(s)
    Price      Size     Value$       Cum$
    0.8500   120.000    $102.00    $102.00
    0.8600    85.000     $73.10    $175.10
    ...

  DOWN asks — 5 level(s)
    ...

  ▶ POST-MARKET WINDOW — 18s remaining
  ══ MARKET ENDED  Winner: UP  Beat: $83,450.00  End: $83,525.00 ══
  Winner (UP) asks below $1.00 — 3 level(s)
    ...
```

## Project Structure

```
poly-last-new/
├── Cargo.toml
├── .env.example
├── data/                       # Auto-created; CSV logs written here
│   ├── <slug>_trades.csv       # One file per market
│   └── pnl_summary.csv         # Aggregated PnL across all markets
└── src/
    ├── bin/
    │   ├── bot_test.rs         # Paper trading entry point
    │   ├── bot_real.rs         # Live trading entry point
    │   └── pnl.rs              # Standalone PnL calculator
    ├── api.rs                  # Polymarket Gamma + CLOB API calls
    ├── binance.rs              # Binance price stream (250 ms polling)
    ├── config.rs               # .env configuration loading
    ├── display.rs              # Terminal UI rendering
    ├── engine.rs               # Main trading loop & state machine
    ├── executor.rs             # Order execution (test vs. live)
    ├── csv_log.rs              # Trade and PnL CSV logging
    ├── types.rs                # Core data structures
    └── lib.rs                  # Module exports
```

## Data Logs

All logs are written to the `data/` directory (created automatically).

### Per-market trade log — `data/<slug>_trades.csv`

| Column | Description |
|---|---|
| `executed_at` | UTC timestamp of execution |
| `market_slug` | Market identifier (e.g. `btc-updown-5m-1776795300`) |
| `outcome` | `UP` or `DOWN` |
| `shares` | Shares purchased |
| `usdc_spent` | USDC amount used |
| `fill_price` | Fill price per share |
| `order_id` | On-chain order ID or paper trade UUID |
| `is_live` | `true` for real orders, `false` for paper |

### PnL summary — `data/pnl_summary.csv`

| Column | Description |
|---|---|
| `slug` | Market identifier |
| `beat_price` | Binance price at market bucket start |
| `end_price` | Binance price at market expiration |
| `winner` | Correct outcome (`up` / `down`) |
| `our_outcome` | Outcome we purchased |
| `shares` | Shares bought |
| `usdc_spent` | USDC spent |
| `fill_price` | Average fill price |
| `pnl` | Profit/loss: `shares − usdc_spent` (win) or `−usdc_spent` (loss) |
| `resolved` | Whether Polymarket confirmed the winner on-chain |
| `executed_at` | Trade timestamp |
| `order_id` | Associated order ID |

## Supported Markets

| Market key | Asset | Duration |
|---|---|---|
| `btc-5m` | BTC/USD | 5 minutes |
| `btc-15m` | BTC/USD | 15 minutes |
| `eth-5m` | ETH/USD | 5 minutes |
| `eth-15m` | ETH/USD | 15 minutes |

Market slugs follow the format `{asset}-updown-{interval}-{unix_timestamp}`, e.g. `btc-updown-5m-1776795300`. The timestamp in the slug is the bucket start time — this is exactly the moment the beat price is captured.

## External APIs

| Service | Purpose |
|---|---|
| Binance public REST | Live BTC/ETH price stream (no key required) |
| Polymarket Gamma API | Market metadata, token IDs, on-chain resolution |
| Polymarket CLOB API | Orderbook snapshots, order placement |

## Operational Notes

- **Beat price timing**: Captured at the exact bucket boundary, even during the previous market's 25-second window. The bot does not wait for the transition to record it.
- **Post-market window**: 25 seconds after market end. The orderbook is refreshed every 200 ms during this window. Trading stops when either time runs out, budget is exhausted, or `MAX_TRADES` is reached.
- **Order type**: FOK (fill-or-kill) — fills immediately at the submitted price or is rejected. No resting orders.
- **Minimum order**: $1.00 USDC; minimum lot size 0.01 shares. Orders below these thresholds are skipped.
- **Wallet funding**: Live mode requires USDC in the Polygon wallet. Use a dedicated wallet with only the USDC needed for trading.

## Security

- Never commit your `.env` file or expose `POLYMARKET_PRIVATE_KEY`.
- Use a dedicated wallet — do not use a primary wallet.
- Review `executor.rs` and `config.rs` before running in live mode.

## License

See [LICENSE](LICENSE) if present, or contact the project maintainer.
