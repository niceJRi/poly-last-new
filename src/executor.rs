use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::types::{BuyParams, ExecResult};

// ── Executor trait ────────────────────────────────────────────────────────────

#[async_trait]
pub trait Executor: Send + Sync {
    async fn execute_buy(&self, params: &BuyParams) -> Result<ExecResult>;
    fn is_live(&self) -> bool;
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
}

// ── Real executor ─────────────────────────────────────────────────────────────

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::U256;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer as _;
use anyhow::Context;
use rust_decimal::Decimal;

use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::{self, Credentials, Normal};
use polymarket_client_sdk::clob::types::{OrderType, Side, SignatureType};
use polymarket_client_sdk::clob::{Client, Config as ClobConfig};
use polymarket_client_sdk::POLYGON;

use crate::config::Config;

type NormalClient  = Client<Authenticated<Normal>>;
type BuilderClient = Client<Authenticated<auth::builder::Builder>>;

enum Inner {
    Builder(Arc<(BuilderClient, PrivateKeySigner)>),
    Normal(Arc<(NormalClient, PrivateKeySigner)>),
}

pub struct RealExecutor {
    inner: Inner,
    slippage_buffer: f64,
}

impl RealExecutor {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let private_key = cfg.private_key.as_deref()
            .context("POLYMARKET_PRIVATE_KEY required for real mode")?;

        let signer = PrivateKeySigner::from_str(private_key)
            .context("Failed to parse private key")?
            .with_chain_id(Some(POLYGON));

        let normal_client = Client::new(
            "https://clob.polymarket.com",
            ClobConfig::builder().use_server_time(true).build(),
        )?
        .authentication_builder(&signer)
        .signature_type(SignatureType::GnosisSafe)
        .authenticate()
        .await
        .context("CLOB authentication failed")?;

        if let (Some(bkey), Some(bsec), Some(bpass)) = (
            cfg.builder_api_key.as_deref(),
            cfg.builder_secret.as_deref(),
            cfg.builder_passphrase.as_deref(),
        ) {
            let key_uuid = Uuid::parse_str(bkey)
                .context("POLYMARKET_BUILDER_KEY must be a valid UUID")?;
            let builder_creds = Credentials::new(key_uuid, bsec.to_string(), bpass.to_string());
            let builder_cfg = auth::builder::Config::local(builder_creds);
            let builder_client = normal_client
                .promote_to_builder(builder_cfg)
                .await
                .context("Failed to promote to builder")?;

            return Ok(RealExecutor {
                inner: Inner::Builder(Arc::new((builder_client, signer))),
                slippage_buffer: cfg.slippage_buffer,
            });
        }

        Ok(RealExecutor {
            inner: Inner::Normal(Arc::new((normal_client, signer))),
            slippage_buffer: cfg.slippage_buffer,
        })
    }
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

        let order_value = raw_shares * price_f;
        if order_value < 1.0 {
            return Err(anyhow::anyhow!(
                "order value ${:.3} is below $1.00 minimum",
                order_value,
            ));
        }

        let token_u256 = U256::from_str(&params.token_id)
            .with_context(|| format!("bad token_id: {}", params.token_id))?;
        let size  = Decimal::from_str(&format!("{:.2}", raw_shares)).context("invalid shares")?;
        let price = Decimal::from_str(&format!("{:.2}", price_f)).context("invalid price")?;

        match &self.inner {
            Inner::Builder(pair) => {
                let (client, signer) = pair.as_ref();
                let signable = client
                    .limit_order()
                    .token_id(token_u256)
                    .side(Side::Buy)
                    .price(price)
                    .size(size)
                    .order_type(OrderType::FOK)
                    .build()
                    .await?;
                let signed = client.sign(signer, signable).await?;
                let resp   = client.post_order(signed).await?;
                Ok(ExecResult {
                    fill_price: price_f,
                    shares: raw_shares,
                    usdc: raw_shares * price_f,
                    order_id: resp.order_id.to_string(),
                    notes: format!("live_builder ask={:.4} buf={:.4}", params.ask_price, self.slippage_buffer),
                })
            }
            Inner::Normal(pair) => {
                let (client, signer) = pair.as_ref();
                let signable = client
                    .limit_order()
                    .token_id(token_u256)
                    .side(Side::Buy)
                    .price(price)
                    .size(size)
                    .order_type(OrderType::FOK)
                    .build()
                    .await?;
                let signed = client.sign(signer, signable).await?;
                let resp   = client.post_order(signed).await?;
                Ok(ExecResult {
                    fill_price: price_f,
                    shares: raw_shares,
                    usdc: raw_shares * price_f,
                    order_id: resp.order_id.to_string(),
                    notes: format!("live_normal ask={:.4} buf={:.4}", params.ask_price, self.slippage_buffer),
                })
            }
        }
    }

    fn is_live(&self) -> bool { true }
}
