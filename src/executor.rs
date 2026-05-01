use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::types::{BuyParams, ExecResult};

// ── Executor trait ────────────────────────────────────────────────────────────

#[async_trait]
pub trait Executor: Send + Sync {
    async fn execute_buy(&self, params: &BuyParams) -> Result<ExecResult>;
    fn is_live(&self) -> bool;
    fn wallet_address(&self) -> Option<String>;
    async fn fetch_usdc_balance(&self) -> f64;
}

// ── Test (paper) executor ─────────────────────────────────────────────────────

pub struct TestExecutor;

#[async_trait]
impl Executor for TestExecutor {
    async fn execute_buy(&self, params: &BuyParams) -> Result<ExecResult> {
        let fill_price = params.ask_price;
        let usdc = params.shares * fill_price;
        Ok(ExecResult {
            fill_price,
            shares: params.shares,
            usdc,
            order_id: format!("paper-{}", Uuid::new_v4()),
            notes: "paper_mode".to_string(),
        })
    }

    fn is_live(&self) -> bool { false }
    fn wallet_address(&self) -> Option<String> { None }
    async fn fetch_usdc_balance(&self) -> f64 { 0.0 }
}

// ── Real executor ─────────────────────────────────────────────────────────────

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{B256, U256};
use alloy::signers::Signer as _;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Context;
use rust_decimal::Decimal;

use polymarket_client_sdk_v2::auth::state::Authenticated;
use polymarket_client_sdk_v2::auth::Normal;
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::clob::types::{AssetType, OrderStatusType, OrderType, Side, SignatureType};
use polymarket_client_sdk_v2::clob::{Client, Config as ClobConfig};
use polymarket_client_sdk_v2::{derive_safe_wallet, POLYGON};

use crate::config::Config;

pub struct RealExecutor {
    client:          Arc<Client<Authenticated<Normal>>>,
    signer:          Arc<PrivateKeySigner>,
    slippage_buffer: f64,
    proxy_wallet:    Option<String>,
}

impl RealExecutor {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let private_key = cfg.private_key.as_deref()
            .context("POLYMARKET_PRIVATE_KEY required for real mode")?;

        let signer = PrivateKeySigner::from_str(private_key)
            .context("Failed to parse private key")?
            .with_chain_id(Some(POLYGON));

        // Builder code is an optional 32-byte hex string for trade attribution.
        let clob_config = if let Some(code_hex) = cfg.builder_code.as_deref() {
            let code = B256::from_str(code_hex)
                .context("POLYMARKET_BUILDER_CODE must be a 0x-prefixed 32-byte hex string")?;
            ClobConfig::builder().builder_code(code).build()
        } else {
            ClobConfig::default()
        };

        let client = Client::new("https://clob.polymarket.com", clob_config)?
            .authentication_builder(&signer)
            .signature_type(SignatureType::GnosisSafe)
            .authenticate()
            .await
            .context("CLOB V2 authentication failed")?;

        // Derive the Gnosis Safe proxy wallet address (same derivation the SDK uses internally).
        let proxy_wallet = derive_safe_wallet(signer.address(), POLYGON)
            .map(|addr| format!("{addr:#x}"));

        Ok(RealExecutor {
            client: Arc::new(client),
            signer: Arc::new(signer),
            slippage_buffer: cfg.slippage_buffer,
            proxy_wallet,
        })
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[async_trait]
impl Executor for RealExecutor {
    async fn execute_buy(&self, params: &BuyParams) -> Result<ExecResult> {
        // Round shares down to 2 decimal places (Polymarket LOT_SIZE = 0.01)
        let raw_shares = (params.shares * 100.0).floor() / 100.0;
        if raw_shares < 0.01 {
            return Err(anyhow::anyhow!("shares too small: {:.4} (min 0.01)", params.shares));
        }

        let raw_price = (params.ask_price + self.slippage_buffer).clamp(0.01, 0.99);
        let price_f   = (raw_price * 100.0).round() / 100.0;

        // CLOB V2 FOK: maker_amount = shares × price (USDC cost) must have ≤ 2 decimal
        // places (i.e. be a whole number of cents).  For shares and price both at 2 dec,
        // shares × price has ≤ 4 dec places.  We align shares DOWN to the nearest
        // multiple that makes the product exact to cents.
        //
        // Math: shares_cents × price_cents must be divisible by 100.
        //       The required step = 100 / gcd(price_cents, 100).
        let price_cents  = (price_f * 100.0).round() as u64;
        let step         = 100 / gcd(price_cents, 100);
        let raw_cents    = (raw_shares * 100.0) as u64;
        let adj_cents    = (raw_cents / step) * step;
        let adj_shares   = adj_cents as f64 / 100.0;

        if adj_shares < 0.01 {
            return Err(anyhow::anyhow!(
                "shares too small after cent-alignment: {:.2} (price={:.2})",
                adj_shares, price_f,
            ));
        }

        let order_value = adj_shares * price_f;
        if order_value < 1.0 {
            return Err(anyhow::anyhow!(
                "order value ${:.3} is below $1.00 minimum",
                order_value,
            ));
        }

        let token_u256 = U256::from_str(&params.token_id)
            .with_context(|| format!("bad token_id: {}", params.token_id))?;
        let size  = Decimal::from_str(&format!("{:.2}", adj_shares)).context("invalid shares")?;
        let price = Decimal::from_str(&format!("{:.2}", price_f)).context("invalid price")?;

        let resp = self.client
            .limit_order()
            .token_id(token_u256)
            .side(Side::Buy)
            .price(price)
            .size(size)
            .order_type(OrderType::FOK)
            .build_sign_and_post(&*self.signer)
            .await?;

        // ── Step 1: check the immediate API response ───────────────────────────
        // HTTP 200 does NOT guarantee a fill — status=Matched is only the off-chain
        // CLOB match; the Polygon settlement transaction is submitted asynchronously.
        if !resp.success || resp.status != OrderStatusType::Matched {
            return Err(anyhow::anyhow!(
                "order not filled: status={:?} success={} error={} orderID={}",
                resp.status, resp.success,
                resp.error_msg.as_deref().unwrap_or("none"),
                resp.order_id,
            ));
        }

        // Empty trade_ids means the CLOB matched but registered no actual trade.
        if resp.trade_ids.is_empty() {
            return Err(anyhow::anyhow!(
                "order MATCHED but no trade IDs registered (phantom match) — orderID={}",
                resp.order_id,
            ));
        }

        // ── Step 2: confirm on-chain settlement via USDC balance ───────────────
        // Polygon block time ≈ 2 s; poll up to 6 s for the balance to decrease.
        // The balance only decreases after the on-chain transaction is mined.
        // If the market is resolving and the exchange rejects the tx, balance stays
        // unchanged and we treat the order as failed.
        let expected_usdc = adj_shares * price_f;
        let balance_before = self.fetch_usdc_balance().await;
        let settled = self.wait_for_settlement(balance_before, expected_usdc * 0.5).await;
        if !settled {
            return Err(anyhow::anyhow!(
                "order MATCHED but USDC balance unchanged after 6s (balance=${:.2}) — \
                 on-chain settlement failed: orderID={}",
                balance_before, resp.order_id,
            ));
        }

        // ── Step 3: record the actual fill ────────────────────────────────────
        // Use response amounts when available; fall back to our calculated values.
        let filled_shares = if resp.taking_amount > Decimal::ZERO {
            resp.taking_amount.try_into().unwrap_or(adj_shares)
        } else {
            adj_shares
        };
        let filled_usdc = if resp.making_amount > Decimal::ZERO {
            resp.making_amount.try_into().unwrap_or(expected_usdc)
        } else {
            expected_usdc
        };

        Ok(ExecResult {
            fill_price: price_f,
            shares:     filled_shares,
            usdc:       filled_usdc,
            order_id:   resp.order_id.to_string(),
            notes:      format!("live ask={:.4} buf={:.4}", params.ask_price, self.slippage_buffer),
        })
    }

    fn is_live(&self) -> bool { true }

    fn wallet_address(&self) -> Option<String> {
        self.proxy_wallet.clone()
    }

    async fn fetch_usdc_balance(&self) -> f64 {
        let req = BalanceAllowanceRequest::builder()
            .asset_type(AssetType::Collateral)
            .build();
        self.client.balance_allowance(req).await
            .map(|r| r.balance.try_into().unwrap_or(0.0))
            .unwrap_or(0.0)
    }

}

impl RealExecutor {
    /// Poll the USDC balance every 500 ms until it drops by at least `min_drop`
    /// (confirming on-chain settlement), or until 6 seconds have elapsed.
    async fn wait_for_settlement(&self, balance_before: f64, min_drop: f64) -> bool {
        for _ in 0..12 {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let now = self.fetch_usdc_balance().await;
            if balance_before - now >= min_drop {
                return true;
            }
        }
        false
    }
}
