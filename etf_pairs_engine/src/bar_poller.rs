use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeSet, HashMap};

use crate::alpaca::{AlpacaClient, Quote};
use crate::config::SignalTimeframe;

pub struct AlpacaBarPoller {
    client: AlpacaClient,
    symbols: Vec<String>,
    timeframes: Vec<SignalTimeframe>,
    feed: String,
    poll_interval: Duration,
    last_poll: HashMap<SignalTimeframe, DateTime<Utc>>,
    last_emitted: HashMap<SignalTimeframe, DateTime<Utc>>,
}

impl AlpacaBarPoller {
    pub fn new(
        client: AlpacaClient,
        symbols: Vec<String>,
        timeframes: Vec<SignalTimeframe>,
        feed: String,
        poll_seconds: u64,
    ) -> Self {
        Self {
            client,
            symbols,
            timeframes,
            feed,
            poll_interval: Duration::seconds(poll_seconds.max(15) as i64),
            last_poll: HashMap::new(),
            last_emitted: HashMap::new(),
        }
    }

    pub async fn poll(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<HashMap<SignalTimeframe, HashMap<String, Quote>>> {
        let mut out = HashMap::new();
        for timeframe in self.timeframes.iter().copied() {
            if self
                .last_poll
                .get(&timeframe)
                .map(|last| *last + self.poll_interval > now)
                .unwrap_or(false)
            {
                continue;
            }
            self.last_poll.insert(timeframe, now);
            if let Some((ts, quotes)) = self.latest_completed_quotes(timeframe, now).await? {
                if self
                    .last_emitted
                    .get(&timeframe)
                    .map(|last| *last >= ts)
                    .unwrap_or(false)
                {
                    continue;
                }
                self.last_emitted.insert(timeframe, ts);
                out.insert(timeframe, quotes);
            }
        }
        Ok(out)
    }

    async fn latest_completed_quotes(
        &self,
        timeframe: SignalTimeframe,
        now: DateTime<Utc>,
    ) -> Result<Option<(DateTime<Utc>, HashMap<String, Quote>)>> {
        let start = now - lookback(timeframe);
        let bars = self
            .client
            .historical_bars_with_feed(
                &self.symbols,
                alpaca_timeframe(timeframe),
                start,
                now,
                &self.feed,
                10_000,
            )
            .await?;

        let cutoff = now - completion_lag(timeframe);
        let mut common: Option<BTreeSet<DateTime<Utc>>> = None;
        for symbol in &self.symbols {
            let Some(series) = bars.get(symbol) else {
                return Ok(None);
            };
            let completed: BTreeSet<_> = series
                .iter()
                .filter(|bar| bar.t + bar_duration(timeframe) <= cutoff)
                .map(|bar| bar.t)
                .collect();
            if completed.is_empty() {
                return Ok(None);
            }
            common = match common {
                None => Some(completed),
                Some(current) => Some(current.intersection(&completed).copied().collect()),
            };
        }

        let Some(ts) = common.and_then(|set| set.into_iter().next_back()) else {
            return Ok(None);
        };
        let mut quotes = HashMap::new();
        for symbol in &self.symbols {
            let Some(bar) = bars
                .get(symbol)
                .and_then(|series| series.iter().find(|bar| bar.t == ts))
            else {
                return Ok(None);
            };
            quotes.insert(
                symbol.clone(),
                Quote {
                    symbol: symbol.clone(),
                    bid: bar.c,
                    ask: bar.c,
                    bid_size: 100.0,
                    ask_size: 100.0,
                    ts: bar.t,
                },
            );
        }
        Ok(Some((ts, quotes)))
    }
}

fn alpaca_timeframe(timeframe: SignalTimeframe) -> &'static str {
    match timeframe {
        SignalTimeframe::Tick => "1Min",
        SignalTimeframe::OneHour => "1Hour",
        SignalTimeframe::FourHour => "4Hour",
        SignalTimeframe::OneDay => "1Day",
    }
}

fn bar_duration(timeframe: SignalTimeframe) -> Duration {
    match timeframe {
        SignalTimeframe::Tick => Duration::minutes(1),
        SignalTimeframe::OneHour => Duration::hours(1),
        SignalTimeframe::FourHour => Duration::hours(4),
        SignalTimeframe::OneDay => Duration::days(1),
    }
}

fn completion_lag(timeframe: SignalTimeframe) -> Duration {
    bar_duration(timeframe) + Duration::minutes(2)
}

fn lookback(timeframe: SignalTimeframe) -> Duration {
    match timeframe {
        SignalTimeframe::Tick => Duration::hours(2),
        SignalTimeframe::OneHour => Duration::days(3),
        SignalTimeframe::FourHour => Duration::days(10),
        SignalTimeframe::OneDay => Duration::days(14),
    }
}
