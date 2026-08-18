use crate::bar_aggregator::WorkingBarState;
use crate::config::StorageConfig;
use crate::pairs::PairDecision;
use anyhow::Result;
use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::str::FromStr;

#[derive(Clone)]
pub struct AuditStore {
    enabled: bool,
    pool: Option<SqlitePool>,
}
impl AuditStore {
    pub async fn new(cfg: &StorageConfig) -> Result<Self> {
        if !cfg.enabled {
            return Ok(Self {
                enabled: false,
                pool: None,
            });
        }
        let url = format!("sqlite://{}", cfg.sqlite_path);
        let opts = SqliteConnectOptions::from_str(&url)?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await?;
        Ok(Self {
            enabled: true,
            pool: Some(pool),
        })
    }
    pub async fn migrate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        sqlx::query(r#"CREATE TABLE IF NOT EXISTS pair_signals(ts TEXT, pair_id TEXT, a TEXT, b TEXT, beta REAL, spread REAL, mean REAL, std REAL, z_score REAL, correlation REAL, action TEXT, reason TEXT, leg_notional REAL);"#).execute(p).await?;
        sqlx::query(r#"CREATE TABLE IF NOT EXISTS pair_orders(ts TEXT, pair_id TEXT, order_id TEXT, symbol TEXT, side TEXT, qty REAL, limit_price REAL, status TEXT);"#).execute(p).await?;
        sqlx::query(r#"CREATE TABLE IF NOT EXISTS pair_execution(ts TEXT, pair_id TEXT, event_type TEXT, detail TEXT);"#).execute(p).await?;
        sqlx::query(r#"CREATE TABLE IF NOT EXISTS risk_events(ts TEXT, event_type TEXT, pair_id TEXT, detail TEXT);"#).execute(p).await?;
        sqlx::query(r#"CREATE TABLE IF NOT EXISTS bar_aggregator_state(symbol TEXT NOT NULL, timeframe TEXT NOT NULL, state_json TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(symbol, timeframe));"#).execute(p).await?;
        Ok(())
    }

    pub async fn load_bar_states(&self) -> Result<Vec<WorkingBarState>> {
        if !self.enabled {
            return Ok(Vec::new());
        }
        let p = self.pool.as_ref().unwrap();
        let rows: Vec<(String,)> = sqlx::query_as("SELECT state_json FROM bar_aggregator_state")
            .fetch_all(p)
            .await?;
        rows.into_iter()
            .map(|(raw,)| Ok(serde_json::from_str(&raw)?))
            .collect()
    }

    pub async fn save_bar_states(&self, states: &[WorkingBarState]) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        let mut tx = p.begin().await?;
        sqlx::query("DELETE FROM bar_aggregator_state")
            .execute(&mut *tx)
            .await?;
        for state in states {
            sqlx::query(
                r#"INSERT INTO bar_aggregator_state(symbol, timeframe, state_json, updated_at) VALUES (?1, ?2, ?3, ?4)"#,
            )
            .bind(&state.symbol)
            .bind(state.timeframe.as_str())
            .bind(serde_json::to_string(state)?)
            .bind(Utc::now().to_rfc3339())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn record_signal(&self, d: &PairDecision) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        sqlx::query("INSERT INTO pair_signals VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)")
            .bind(d.ts.to_rfc3339())
            .bind(&d.pair_id)
            .bind(&d.a)
            .bind(&d.b)
            .bind(d.beta)
            .bind(d.spread)
            .bind(d.mean)
            .bind(d.std)
            .bind(d.z_score)
            .bind(d.correlation)
            .bind(format!("{:?}", d.action))
            .bind(&d.reason)
            .bind(d.leg_notional)
            .execute(p)
            .await?;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn record_order(
        &self,
        pair: &str,
        order_id: &str,
        symbol: &str,
        side: &str,
        qty: f64,
        limit_price: f64,
        status: &str,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        sqlx::query("INSERT INTO pair_orders VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
            .bind(Utc::now().to_rfc3339())
            .bind(pair)
            .bind(order_id)
            .bind(symbol)
            .bind(side)
            .bind(qty)
            .bind(limit_price)
            .bind(status)
            .execute(p)
            .await?;
        Ok(())
    }
    pub async fn record_execution(&self, pair: &str, event_type: &str, detail: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        sqlx::query("INSERT INTO pair_execution VALUES (?1,?2,?3,?4)")
            .bind(Utc::now().to_rfc3339())
            .bind(pair)
            .bind(event_type)
            .bind(detail)
            .execute(p)
            .await?;
        Ok(())
    }
    pub async fn record_risk_event(
        &self,
        event_type: &str,
        pair: &str,
        detail: &str,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let p = self.pool.as_ref().unwrap();
        sqlx::query("INSERT INTO risk_events VALUES (?1,?2,?3,?4)")
            .bind(Utc::now().to_rfc3339())
            .bind(event_type)
            .bind(pair)
            .bind(detail)
            .execute(p)
            .await?;
        Ok(())
    }
}
