# Building The Bridge's Windows installer (NSIS)

These steps run on your Windows 10/11 PC in **PowerShell**. Set aside about 10 GB of disk space and 30-60 minutes for the first build.

## 1. One-time tools
```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup
winget install --id OpenJS.NodeJS.LTS
winget install --id Git.Git
```
Close PowerShell and open it again, then check:
```powershell
rustc -V; cargo -V; node -v; npm -v
```
WebView2 already comes with Windows 11. The installer also embeds its bootstrapper. Tauri downloads NSIS automatically on the first build.

## 2. Get the source
Copy the rebranded `openalgo-desktop` folder from the box, or clone **your** source repo once it exists:
```powershell
git clone https://github.com/derivativesindicators/the-bridge the-bridge; cd the-bridge
```
`rust-toolchain.toml` installs the right Rust version by itself.

## 3. (Only if you are re-branding from upstream) run the script
```powershell
python rebrand.py --repo . --name "The Bridge" --slug the-bridge --app-id com.derivativesindicators.thebridge `
  --company "Derivatives Indicators" --website https://derivativesindicators.com --icon logo.png --source-url https://github.com/derivativesindicators/the-bridge
```

## 4. Build
```powershell
npm ci
npx tauri build --bundles nsis
```
The installer is written to `src-tauri\target\release\bundle\nsis\The Bridge_1.0.5_x64-setup.exe`.

## 5. Optional
- **Code signing** (to avoid SmartScreen warnings): buy an OV/EV certificate, put its thumbprint in `bundle.windows.certificateThumbprint` in `src-tauri/tauri.conf.json`, and set `timestampUrl` (e.g. `http://timestamp.digicert.com`).
- **Your own auto-updates:**
  1. Generate a signing key: `npx tauri signer generate -w $HOME\.tauri\thebridge.key`. Keep it private and back it up.
  2. Put the public key into `plugins.updater.pubkey` and your `latest.json` URL into `endpoints`.
  3. Before building, set `$env:TAURI_SIGNING_PRIVATE_KEY` (and `..._PASSWORD`).
  4. Upload the `.exe`, the `.sig` and `latest.json` to your releases.
  Until you do this, update checks hit a placeholder URL and just fail quietly.
- **AGPL:** publish the exact source (git tag) for every installer you hand out, and keep the "Source code" link in the footer pointing at it.

## Troubleshooting
- `link.exe not found`: the VS Build Tools C++ workload is missing.
- Builds are slow or run out of memory: DuckDB is compiled from source. Try `$env:CARGO_BUILD_JOBS=4`.
- Antivirus locks `target\`: add an exclusion for the project folder.
