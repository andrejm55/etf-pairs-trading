use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::config::AlpacaConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    pub ts: DateTime<Utc>,
}
impl Quote {
    pub fn mid(&self) -> f64 {
        (self.bid + self.ask) * 0.5
    }
    pub fn spread_bps(&self) -> f64 {
        ((self.ask - self.bid) / self.mid()) * 10_000.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub symbol: String,
    pub status: String,
    #[serde(default)]
    pub tradable: bool,
    #[serde(default)]
    pub marginable: bool,
    #[serde(default)]
    pub shortable: bool,
    #[serde(default)]
    pub easy_to_borrow: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewOrder {
    pub symbol: String,
    pub qty: f64,
    pub side: Side,
    pub limit_price: f64,
    pub client_order_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderStatus {
    pub id: String,
    pub client_order_id: String,
    pub symbol: String,
    pub status: String,
    pub qty: f64,
    pub filled_qty: f64,
}

impl OrderStatus {
    pub fn is_filled(&self) -> bool {
        self.status == "filled" || self.filled_qty >= self.qty
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "filled" | "canceled" | "expired" | "rejected" | "suspended"
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub qty: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Bar {
    pub t: DateTime<Utc>,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
    #[serde(default)]
    pub n: Option<u64>,
    #[serde(default)]
    pub vw: Option<f64>,
}

#[async_trait]
pub trait BrokerGateway {
    async fn latest_quotes(&self, symbols: &[String]) -> Result<HashMap<String, Quote>>;
    async fn submit_limit_order(&self, order: NewOrder) -> Result<OrderAck>;
    async fn order_status(&self, order_id: &str) -> Result<OrderStatus>;
    async fn cancel_order(&self, order_id: &str) -> Result<()>;
    async fn open_positions(&self) -> Result<Vec<Position>>;
    async fn asset(&self, symbol: &str) -> Result<Asset>;
    async fn account_equity(&self) -> Result<f64>;
}

#[derive(Clone)]
pub struct AlpacaClient {
    http: Client,
    trading_base: String,
    data_base: String,
    key: String,
    secret: String,
}

impl AlpacaClient {
    pub fn from_config(cfg: &AlpacaConfig) -> Result<Self> {
        let key =
            std::env::var(&cfg.api_key_env).map_err(|_| anyhow!("missing {}", cfg.api_key_env))?;
        let secret = std::env::var(&cfg.secret_key_env)
            .map_err(|_| anyhow!("missing {}", cfg.secret_key_env))?;
        let configured_trading_base = if cfg.trading_base_url.trim().is_empty() && cfg.paper {
            "https://paper-api.alpaca.markets".to_string()
        } else {
            cfg.trading_base_url.clone()
        };
        let trading_base =
            std::env::var("ALPACA_TRADING_ENDPOINT").unwrap_or(configured_trading_base);
        let data_base =
            std::env::var("ALPACA_DATA_ENDPOINT").unwrap_or_else(|_| cfg.data_base_url.clone());
        Ok(Self {
            http: Client::new(),
            trading_base: v2_base(&trading_base),
            data_base: v2_base(&data_base),
            key,
            secret,
        })
    }
    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("APCA-API-KEY-ID", &self.key)
            .header("APCA-API-SECRET-KEY", &self.secret)
    }

    pub async fn historical_bars_with_feed(
        &self,
        symbols: &[String],
        timeframe: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        feed: &str,
        limit: usize,
    ) -> Result<HashMap<String, Vec<Bar>>> {
        #[derive(Deserialize)]
        struct BarsResponse {
            bars: HashMap<String, Vec<Bar>>,
            next_page_token: Option<String>,
        }

        let url = format!("{}/stocks/bars", self.data_base);
        let symbols = symbols.join(",");
        let start = start.to_rfc3339();
        let end = end.to_rfc3339();
        let limit = limit.clamp(1, 10_000).to_string();
        let mut page_token: Option<String> = None;
        let mut out: HashMap<String, Vec<Bar>> = HashMap::new();

        loop {
            let mut query = vec![
                ("symbols", symbols.as_str()),
                ("timeframe", timeframe),
                ("start", start.as_str()),
                ("end", end.as_str()),
                ("adjustment", "all"),
                ("feed", feed),
                ("limit", limit.as_str()),
            ];
            if let Some(token) = page_token.as_deref() {
                query.push(("page_token", token));
            }

            let resp: BarsResponse = self
                .auth(self.http.get(&url))
                .query(&query)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            for (symbol, mut bars) in resp.bars {
                out.entry(symbol).or_default().append(&mut bars);
            }

            page_token = resp.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        for bars in out.values_mut() {
            bars.sort_by_key(|bar| bar.t);
        }
        Ok(out)
    }
}

fn v2_base(raw: &str) -> String {
    let base = raw.trim_end_matches('/');
    if base.ends_with("/v2") {
        base.to_string()
    } else {
        format!("{base}/v2")
    }
}

#[derive(Deserialize)]
struct AlpacaLatestQuote {
    quote: Option<AlpacaQuoteInner>,
}
#[derive(Deserialize)]
struct AlpacaQuoteInner {
    bp: f64,
    ap: f64,
    bs: Option<f64>,
    #[serde(rename = "as")]
    ask_sz: Option<f64>,
    t: DateTime<Utc>,
}

#[async_trait]
impl BrokerGateway for AlpacaClient {
    async fn latest_quotes(&self, symbols: &[String]) -> Result<HashMap<String, Quote>> {
        let mut out = HashMap::new();
        for sym in symbols {
            let url = format!("{}/stocks/{}/quotes/latest", self.data_base, sym);
            let resp: AlpacaLatestQuote = self
                .auth(self.http.get(url))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if let Some(q) = resp.quote {
                out.insert(
                    sym.clone(),
                    Quote {
                        symbol: sym.clone(),
                        bid: q.bp,
                        ask: q.ap,
                        bid_size: q.bs.unwrap_or(0.0),
                        ask_size: q.ask_sz.unwrap_or(0.0),
                        ts: q.t,
                    },
                );
            }
        }
        Ok(out)
    }
    async fn submit_limit_order(&self, order: NewOrder) -> Result<OrderAck> {
        #[derive(Serialize)]
        struct Body<'a> {
            symbol: &'a str,
            qty: String,
            side: &'a str,
            #[serde(rename = "type")]
            typ: &'a str,
            time_in_force: &'a str,
            limit_price: String,
            client_order_id: &'a str,
        }
        let side = match order.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let body = Body {
            symbol: &order.symbol,
            qty: format!("{:.0}", order.qty),
            side,
            typ: "limit",
            time_in_force: "day",
            limit_price: format!("{:.2}", order.limit_price),
            client_order_id: &order.client_order_id,
        };
        let url = format!("{}/orders", self.trading_base);
        let ack: OrderAck = self
            .auth(self.http.post(url).json(&body))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(ack)
    }

    async fn order_status(&self, order_id: &str) -> Result<OrderStatus> {
        #[derive(Deserialize)]
        struct RawOrder {
            id: String,
            client_order_id: String,
            symbol: String,
            status: String,
            qty: String,
            filled_qty: String,
        }

        let url = format!("{}/orders/{}", self.trading_base, order_id);
        let raw: RawOrder = self
            .auth(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(OrderStatus {
            id: raw.id,
            client_order_id: raw.client_order_id,
            symbol: raw.symbol,
            status: raw.status,
            qty: raw.qty.parse().unwrap_or(0.0),
            filled_qty: raw.filled_qty.parse().unwrap_or(0.0),
        })
    }

    async fn cancel_order(&self, order_id: &str) -> Result<()> {
        let url = format!("{}/orders/{}", self.trading_base, order_id);
        self.auth(self.http.delete(url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn open_positions(&self) -> Result<Vec<Position>> {
        #[derive(Deserialize)]
        struct RawPosition {
            symbol: String,
            qty: String,
        }

        let url = format!("{}/positions", self.trading_base);
        let raw: Vec<RawPosition> = self
            .auth(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(raw
            .into_iter()
            .map(|p| Position {
                symbol: p.symbol,
                qty: p.qty.parse().unwrap_or(0.0),
            })
            .collect())
    }

    async fn asset(&self, symbol: &str) -> Result<Asset> {
        let url = format!("{}/assets/{}", self.trading_base, symbol);
        Ok(self
            .auth(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    async fn account_equity(&self) -> Result<f64> {
        #[derive(Deserialize)]
        struct Account {
            equity: String,
        }
        let url = format!("{}/account", self.trading_base);
        let a: Account = self
            .auth(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(a.equity.parse().unwrap_or(0.0))
    }
}

#[derive(Clone)]
pub struct MockGateway {
    step: Arc<Mutex<u64>>,
    orders: Arc<Mutex<HashMap<String, OrderStatus>>>,
}
impl MockGateway {
    pub fn new(_symbols: Vec<String>) -> Self {
        Self {
            step: Arc::new(Mutex::new(0)),
            orders: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
#[async_trait]
impl BrokerGateway for MockGateway {
    async fn latest_quotes(&self, symbols: &[String]) -> Result<HashMap<String, Quote>> {
        let mut s = self.step.lock();
        *s += 1;
        let t = *s as f64;
        let mut out = HashMap::new();
        for (i, sym) in symbols.iter().enumerate() {
            let base = match sym.as_str() {
                "QQQ" => 440.0,
                "XLK" => 220.0,
                "SMH" => 250.0,
                "SPY" => 520.0,
                _ => 100.0,
            };
            let px = base + (t / 30.0).sin() * (i as f64 + 1.0) * 0.25 + (t / 85.0).cos() * 0.10;
            out.insert(
                sym.clone(),
                Quote {
                    symbol: sym.clone(),
                    bid: px - 0.01,
                    ask: px + 0.01,
                    bid_size: 100.0,
                    ask_size: 100.0,
                    ts: Utc::now(),
                },
            );
        }
        Ok(out)
    }
    async fn submit_limit_order(&self, order: NewOrder) -> Result<OrderAck> {
        let id = Uuid::new_v4().to_string();
        self.orders.lock().insert(
            id.clone(),
            OrderStatus {
                id: id.clone(),
                client_order_id: order.client_order_id.clone(),
                symbol: order.symbol.clone(),
                status: "filled".into(),
                qty: order.qty,
                filled_qty: order.qty,
            },
        );
        Ok(OrderAck {
            id,
            client_order_id: order.client_order_id,
            symbol: order.symbol,
            status: "filled".into(),
        })
    }
    async fn order_status(&self, order_id: &str) -> Result<OrderStatus> {
        self.orders
            .lock()
            .get(order_id)
            .cloned()
            .ok_or_else(|| anyhow!("mock order {order_id} not found"))
    }
    async fn cancel_order(&self, _order_id: &str) -> Result<()> {
        Ok(())
    }
    async fn open_positions(&self) -> Result<Vec<Position>> {
        Ok(Vec::new())
    }
    async fn asset(&self, symbol: &str) -> Result<Asset> {
        Ok(Asset {
            symbol: symbol.into(),
            status: "active".into(),
            tradable: true,
            marginable: true,
            shortable: true,
            easy_to_borrow: true,
        })
    }
    async fn account_equity(&self) -> Result<f64> {
        Ok(100000.0)
    }
}
