//! Backtest and paper-trading analytics helpers.
//!
//! Live order/fill markouts should eventually be sourced from persisted broker events.
//! The current reporting path focuses on deterministic backtest
//! artifacts that can be committed as small examples or regenerated locally.

use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn ensure_report_dir(path: &str) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}

pub fn write_report_file(report_dir: &str, name: &str, body: &str) -> Result<()> {
    fs::write(Path::new(report_dir).join(name), body)?;
    Ok(())
}
