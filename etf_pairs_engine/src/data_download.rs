use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDate, Utc};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::alpaca::AlpacaClient;
use crate::config::AppConfig;

const TESTED_SYMBOLS: &str = "QQQ,XLK,SMH,SPY,IWM,DIA,XLE,OIH,XLF,KRE,IYR,VNQ,EFA,EEM"; //this is the universe of ETFs when stating "tested"

#[derive(Debug, Clone)]
pub struct DownloadBarsOptions {
    pub from: String,
    pub to: String,
    pub timeframe: String,
    pub symbols: String,
    pub feed: String,
    pub limit: usize,
    pub output: String,
}

pub async fn run(cfg: AppConfig, opts: DownloadBarsOptions) -> Result<()> {
    let from = parse_start(&opts.from)?;
    let to = parse_end(&opts.to)?;
    let symbols = parse_symbols(&opts.symbols);
    let client = AlpacaClient::from_config(&cfg.alpaca)?;
    let bars = client
        .historical_bars_with_feed(&symbols, &opts.timeframe, from, to, &opts.feed, opts.limit)
        .await?;

    let path = Path::new(&opts.output);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "symbol,t,o,h,l,c,v,n,vw")?;

    let mut total = 0usize;
    let mut sorted_symbols: Vec<_> = bars.keys().cloned().collect();
    sorted_symbols.sort();
    for symbol in sorted_symbols {
        if let Some(series) = bars.get(&symbol) {
            total += series.len();
            for bar in series {
                writeln!(
                    writer,
                    "{},{},{:.6},{:.6},{:.6},{:.6},{:.0},{},{}",
                    symbol,
                    bar.t.to_rfc3339(),
                    bar.o,
                    bar.h,
                    bar.l,
                    bar.c,
                    bar.v,
                    bar.n.map(|v| v.to_string()).unwrap_or_default(),
                    bar.vw.map(|v| format!("{v:.6}")).unwrap_or_default()
                )?;
            }
        }
    }
    writer.flush()?;

    println!("Downloaded historical bars");
    println!("symbols: {}", symbols.join(","));
    println!("timeframe: {}", opts.timeframe);
    println!("feed: {}", opts.feed);
    println!(
        "requested_range: {} to {}",
        from.date_naive(),
        to.date_naive()
    );
    println!("bars: {total}");
    println!("output: {}", opts.output);
    Ok(())
}

pub fn tested_symbols() -> &'static str {
    TESTED_SYMBOLS
}

fn parse_symbols(raw: &str) -> Vec<String> {
    let raw = if raw.eq_ignore_ascii_case("tested") {
        TESTED_SYMBOLS
    } else {
        raw
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_uppercase())
        .collect()
}

fn parse_start(raw: &str) -> Result<DateTime<Utc>> {
    Ok(NaiveDate::parse_from_str(raw, "%Y-%m-%d")?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("invalid start date"))?
        .and_utc())
}

fn parse_end(raw: &str) -> Result<DateTime<Utc>> {
    Ok(NaiveDate::parse_from_str(raw, "%Y-%m-%d")?
        .and_hms_opt(23, 59, 59)
        .ok_or_else(|| anyhow!("invalid end date"))?
        .and_utc())
}
