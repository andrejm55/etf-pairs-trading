# Risk And Limitations

This project is a research and paper-trading system under active development. It is not a live-capital/production trading platform.

## Implemented Controls

- max pair notional
- max total notional
- max open pairs
- max daily loss gate
- shortability and easy-to-borrow checks for short ETF legs
- quote spread filter
- max holding bar exits in the simulator
- startup position reconciliation for enabled Alpaca pairs
- SQLite audit tables for signals, orders, execution events, risk events, and bar state

## Known Limitations

- Historical backtests use simplified fills around bar closes.
- Backtests do not model partial fills, queue priority, borrow fees, market impact, exchange fees, tax effects, or trading halts.
- Paper/live execution still needs a fuller streaming, fill-aware order state machine.
- Dollar stop behavior is exact in the simulator but less precise in live/paper mode until fill state persistence is improved.
- Strategy results are sample-size sensitive and can overfit short windows.
- Alpaca paper fills may not match live liquidity.

## Required before any potential live-capital use

- streaming order/fill updates
- durable fill-aware pair state
- broker position reconciliation tests
- explicit kill switch
- stale order cancellation and rescue logic tests
- live markout and slippage monitoring
- longer multi-year out-of-sample validation
- failure-mode tests with a mocked broker server
