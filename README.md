# The Bridge

**The Bridge** is a desktop trading app by [Derivatives Indicators](https://derivativesindicators.com) for Indian brokers:
charts, order entry, strategy tools, webhooks (TradingView, GoCharting and more) and a local API,
running entirely on your own computer.

> Based on [OpenAlgo Desktop](https://github.com/marketcalls/openalgo-desktop) (Copyright 2024 OpenAlgo),
> licensed under the GNU AGPL-3.0. The Bridge is a modified version; see [MODIFICATIONS.md](MODIFICATIONS.md).

## Download
Get the latest Windows installer (.exe) or macOS disk image (.dmg) from [Releases](https://github.com/derivativesindicators/the-bridge/releases/latest).
Installed copies check for updates automatically and ask before installing.

## Build from source
See [WINDOWS_BUILD.md](WINDOWS_BUILD.md). In short: install Node 22 and Rust, then
```
npm ci
npx tauri build --bundles nsis
```

## Links
- Website: https://derivativesindicators.com
- Support: admin@derivativesindicators.com
- X: https://x.com/i_amArko
- YouTube: https://www.youtube.com/@DerivativesIndicators
- Source code: https://github.com/derivativesindicators/the-bridge

## Licence
GNU Affero General Public License v3.0, see [License.md](License.md). The complete corresponding source
of every release is in this repository. Original work Copyright 2024 OpenAlgo; modifications Copyright Derivatives Indicators.
