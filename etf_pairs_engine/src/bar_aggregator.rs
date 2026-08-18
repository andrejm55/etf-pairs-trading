use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::America::New_York;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::alpaca::Quote;
use crate::config::SignalTimeframe;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingBarState {
    pub symbol: String,
    pub timeframe: SignalTimeframe,
    pub bucket_start: DateTime<Utc>,
    pub close: f64,
    pub close_ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkingBar {
    bucket_start: DateTime<Utc>,
    close: f64,
    close_ts: DateTime<Utc>,
}

impl WorkingBar {
    fn new(bucket_start: DateTime<Utc>, quote: &Quote) -> Self {
        let mid = quote.mid();
        Self {
            bucket_start,
            close: mid,
            close_ts: quote.ts,
        }
    }

    fn update(&mut self, quote: &Quote) {
        self.close = quote.mid();
        self.close_ts = quote.ts;
    }

    fn close_quote(&self, symbol: &str) -> Quote {
        Quote {
            symbol: symbol.to_string(),
            bid: self.close,
            ask: self.close,
            bid_size: 100.0,
            ask_size: 100.0,
            ts: self.close_ts,
        }
    }

    fn state(&self, symbol: &str, timeframe: SignalTimeframe) -> WorkingBarState {
        WorkingBarState {
            symbol: symbol.to_string(),
            timeframe,
            bucket_start: self.bucket_start,
            close: self.close,
            close_ts: self.close_ts,
        }
    }

    fn from_state(state: &WorkingBarState) -> Self {
        Self {
            bucket_start: state.bucket_start,
            close: state.close,
            close_ts: state.close_ts,
        }
    }
}

#[derive(Debug)]
pub struct BarAggregator {
    timeframes: Vec<SignalTimeframe>,
    bars: HashMap<(String, SignalTimeframe), WorkingBar>,
}

impl BarAggregator {
    pub fn new(timeframes: Vec<SignalTimeframe>) -> Self {
        Self {
            timeframes,
            bars: HashMap::new(),
        }
    }

    pub fn restore(&mut self, states: Vec<WorkingBarState>) {
        for state in states {
            if self.timeframes.contains(&state.timeframe) {
                self.bars.insert(
                    (state.symbol.clone(), state.timeframe),
                    WorkingBar::from_state(&state),
                );
            }
        }
    }

    pub fn snapshot(&self) -> Vec<WorkingBarState> {
        self.bars
            .iter()
            .map(|((symbol, timeframe), bar)| bar.state(symbol, *timeframe))
            .collect()
    }

    pub fn update(
        &mut self,
        quotes: &HashMap<String, Quote>,
    ) -> HashMap<SignalTimeframe, HashMap<String, Quote>> {
        let mut completed: HashMap<SignalTimeframe, HashMap<String, Quote>> = HashMap::new();
        for timeframe in self.timeframes.iter().copied() {
            for (symbol, quote) in quotes {
                let Some(bucket_start) = bucket_start(quote.ts, timeframe) else {
                    continue;
                };
                let key = (symbol.clone(), timeframe);
                match self.bars.get_mut(&key) {
                    Some(bar) if bar.bucket_start == bucket_start => bar.update(quote),
                    Some(bar) => {
                        completed
                            .entry(timeframe)
                            .or_default()
                            .insert(symbol.clone(), bar.close_quote(symbol));
                        *bar = WorkingBar::new(bucket_start, quote);
                    }
                    None => {
                        self.bars.insert(key, WorkingBar::new(bucket_start, quote));
                    }
                }
            }
        }
        completed
    }
}

fn bucket_start(ts: DateTime<Utc>, timeframe: SignalTimeframe) -> Option<DateTime<Utc>> {
    match timeframe {
        SignalTimeframe::Tick => None,
        SignalTimeframe::OneHour => intraday_bucket_start(ts, Duration::hours(1)),
        SignalTimeframe::FourHour => intraday_bucket_start(ts, Duration::hours(4)),
        SignalTimeframe::OneDay => session_open_utc(ts),
    }
}

fn intraday_bucket_start(ts: DateTime<Utc>, bucket: Duration) -> Option<DateTime<Utc>> {
    let session_open = session_open_utc(ts)?;
    let session_close = session_close_utc(ts)?;
    if ts < session_open || ts >= session_close {
        return None;
    }
    let elapsed = ts.signed_duration_since(session_open);
    let bucket_index = elapsed.num_seconds().div_euclid(bucket.num_seconds());
    Some(session_open + bucket * bucket_index as i32)
}

fn session_open_utc(ts: DateTime<Utc>) -> Option<DateTime<Utc>> {
    session_boundary_utc(ts, NaiveTime::from_hms_opt(9, 30, 0)?)
}

fn session_close_utc(ts: DateTime<Utc>) -> Option<DateTime<Utc>> {
    session_boundary_utc(ts, NaiveTime::from_hms_opt(16, 0, 0)?)
}

fn session_boundary_utc(ts: DateTime<Utc>, time: NaiveTime) -> Option<DateTime<Utc>> {
    let local = ts.with_timezone(&New_York);
    local_datetime_to_utc(local.date_naive(), time)
}

fn local_datetime_to_utc(date: NaiveDate, time: NaiveTime) -> Option<DateTime<Utc>> {
    match New_York.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(earliest, _) => Some(earliest.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(symbol: &str, ts: &str, mid: f64) -> Quote {
        Quote {
            symbol: symbol.to_string(),
            bid: mid,
            ask: mid,
            bid_size: 100.0,
            ask_size: 100.0,
            ts: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn emits_completed_hour_when_bucket_rolls() {
        let mut agg = BarAggregator::new(vec![SignalTimeframe::OneHour]);
        let mut quotes = HashMap::new();
        quotes.insert(
            "SPY".to_string(),
            quote("SPY", "2026-05-21T13:30:00Z", 100.0),
        );
        assert!(agg.update(&quotes).is_empty());

        quotes.insert(
            "SPY".to_string(),
            quote("SPY", "2026-05-21T13:59:00Z", 101.0),
        );
        assert!(agg.update(&quotes).is_empty());

        quotes.insert(
            "SPY".to_string(),
            quote("SPY", "2026-05-21T14:30:00Z", 102.0),
        );
        let completed = agg.update(&quotes);
        let bar_quote = &completed[&SignalTimeframe::OneHour]["SPY"];
        assert_eq!(bar_quote.mid(), 101.0);
    }

    #[test]
    fn anchors_intraday_buckets_to_new_york_session() {
        let ts = DateTime::parse_from_rfc3339("2026-05-21T15:12:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let bucket = bucket_start(ts, SignalTimeframe::OneHour).unwrap();
        assert_eq!(bucket.to_rfc3339(), "2026-05-21T14:30:00+00:00");
    }

    #[test]
    fn ignores_quotes_outside_regular_session() {
        let ts = DateTime::parse_from_rfc3339("2026-05-21T21:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(bucket_start(ts, SignalTimeframe::OneHour).is_none());
    }

    #[test]
    fn restores_snapshot_state() {
        let mut agg = BarAggregator::new(vec![SignalTimeframe::OneHour]);
        let mut quotes = HashMap::new();
        quotes.insert(
            "SPY".to_string(),
            quote("SPY", "2026-05-21T13:30:00Z", 100.0),
        );
        agg.update(&quotes);

        let snapshot = agg.snapshot();
        let mut restored = BarAggregator::new(vec![SignalTimeframe::OneHour]);
        restored.restore(snapshot);

        quotes.insert(
            "SPY".to_string(),
            quote("SPY", "2026-05-21T14:30:00Z", 101.0),
        );
        let completed = restored.update(&quotes);
        assert_eq!(
            completed[&SignalTimeframe::OneHour]["SPY"].ts.to_rfc3339(),
            "2026-05-21T13:30:00+00:00"
        );
    }
}
