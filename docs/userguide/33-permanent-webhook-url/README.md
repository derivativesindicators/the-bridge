# Permanent webhook URL (TradingView alerts)

TradingView sends alerts from the internet. The Bridge runs on your computer
(`http://127.0.0.1:5050`), which the internet cannot reach. A *tunnel* gives
your computer a public HTTPS address that forwards to The Bridge. Set it up
once on **TradingView Configuration → Set up a permanent webhook URL**; the
address never changes, and The Bridge starts the tunnel by itself whenever the
app is open and you are signed in.

## Option A — ngrok (free, no domain needed, recommended)

Facts checked October 2026 (ngrok docs, "Free Plan Limits"): every ngrok
account, free included, gets **one dev domain** such as
`abc-123.ngrok-free.dev` (older accounts: `.ngrok-free.app`). It is assigned
automatically, is yours permanently and does not change when the tunnel
restarts. Free limits: 20,000 HTTP requests and 1 GB per month, 3 online
endpoints — far more than alerts need. (Browsers see an ngrok warning page on
free domains; TradingView's webhook calls are not affected.)

1. Sign up at <https://dashboard.ngrok.com/signup>.
2. Copy your token from **Your Authtoken**
   (<https://dashboard.ngrok.com/get-started/your-authtoken>).
3. Copy your dev domain from **Domains** (<https://dashboard.ngrok.com/domains>).
4. In The Bridge choose **A. ngrok**, paste both, **Save & start**.

The Bridge downloads the ngrok agent from ngrok's official site
(`bin.equinox.io`) the first time. It is not bundled: ngrok's terms allow
redistribution of the agent only when the distributor owns the ngrok account.

## Option B — Cloudflare Tunnel (your own domain)

Needs a domain whose DNS is on Cloudflare (free plan is fine).

1. Cloudflare Zero Trust dashboard → **Networks → Tunnels → Create a tunnel →
   Cloudflared**, name it (e.g. `the-bridge`).
2. On the install step copy the command shown (it contains the tunnel token).
   Do not run it.
3. Add a **public hostname**, e.g. `alerts.yourdomain.com`, service **HTTP**,
   URL `127.0.0.1:5050` (your HTTP port; shown in the wizard).
4. In The Bridge choose **B. Cloudflare**, paste the command (or just the
   token) and the hostname, **Save & start**.

The Bridge downloads `cloudflared` from Cloudflare's GitHub releases the first
time.

## After setup

* The public address becomes the **Host Server URL** (Profile → Broker), so the
  webhook URL and JSON on the TradingView page use it, e.g.
  `https://abc-123.ngrok-free.dev/api/v1/placesmartorder`.
* **Test webhook** calls `<public URL>/api/v1/ping` with your API key, the way
  an alert arrives, and shows whether it reached The Bridge.
* **Security:** while the tunnel runs, callers coming through it can reach
  only webhook routes (`/api/v1/…`, strategy/Chartink webhooks and broker
  sign-in callbacks); the app's pages and settings stay on this computer.
  Every order still needs your API key — keep it secret.
* The token is stored encrypted in The Bridge's database, handed to the tunnel
  program through its environment, never shown again and never logged.
* **Broker sign-in:** with a Host Server URL set, the automatic broker redirect
  URL becomes `<public URL>/<broker>/callback`. If your broker app (e.g. Kite
  Connect) is registered with `http://127.0.0.1:5050/zerodha/callback`, either
  update it there or set an explicit redirect URL in Broker Configuration.
* Stop, Start, Change or Remove the tunnel from the same card. Remove forgets
  the token and resets the Host Server URL.
