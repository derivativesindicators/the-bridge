#!/usr/bin/env python3
"""Rebrand OpenAlgo Desktop (AGPL-3.0). Idempotent: safe to run repeatedly.
Usage: rebrand.py --repo DIR --name "The Bridge" --slug the-bridge --app-id com.x.y
       --company "Co" --website https://x.com --icon logo.png [--source-url URL] [--skip-icons]
"""
import argparse, json, re, shutil, subprocess, datetime, pathlib, sys

OLD_ID = "com.openalgo.desktop"
KEEP = re.compile(r"copyright|licen[cs]e|agpl|based on openalgo|marketcalls|openalgo\.in|@openalgo|openalgoHQ", re.I)
WORD = re.compile(r"(?<![A-Za-z0-9_@-])OpenAlgo(?![A-Za-z0-9_])")
RS_STR = re.compile(r'"(?:[^"\\\n]|\\.)*"')

def edit(p, fn):
    p = pathlib.Path(p); old = p.read_text(encoding="utf-8"); new = fn(old)
    if new != old: p.write_text(new, encoding="utf-8"); return 1
    return 0

def brand_text(s, name):
    s = s.replace("OpenAlgo Desktop", name)
    return WORD.sub(name, s)

def rs_fix(text, name):
    out = []
    for line in text.split("\n"):
        st = line.lstrip()
        if not (st.startswith("//") or KEEP.search(line)):
            line = RS_STR.sub(lambda m: brand_text(m.group(0), name), line)
        out.append(line)
    return "\n".join(out)

def rs_multiline_fix(text, name):
    """Brand string-literal text that continues across lines (\\-continued strings)."""
    out, i, n, ins, start = [], 0, len(text), False, 0
    while i < n:
        c = text[i]
        if not ins:
            if text.startswith("//", i):
                j = text.find("\n", i); j = n if j < 0 else j; i = j; continue
            if text.startswith("/*", i):
                j = text.find("*/", i); i = n if j < 0 else j + 2; continue
            if c == "'" and i + 2 < n and (text[i+2] == "'" or (text[i+1] == "\\" and i + 3 < n and text[i+3] == "'")):
                i += 3 if text[i+2] == "'" else 4; continue
            if c == '"': ins, start = True, i + 1
            i += 1; continue
        if c == "\\": i += 2; continue
        if c == '"':
            seg = text[start:i]
            if "\n" in seg: out.append((start, i))
            ins = False
        i += 1
    for a_, b_ in reversed(out):
        seg = "\n".join(l if KEEP.search(l) else brand_text(l, name) for l in text[a_:b_].split("\n"))
        text = text[:a_] + seg + text[b_:]
    return text

def ts_fix(text, name):
    out = []
    for line in text.split("\n"):
        st = line.lstrip()
        if not (st.startswith(("import ", "//", "*", "/*", "export {", "} from")) or KEEP.search(line)):
            line = brand_text(line, name)
        out.append(line)
    return "\n".join(out)

MIGRATE = '''    let data_dir = app.path().app_data_dir()?;
    // REBRAND: first run under the new app id adopts the old OpenAlgo data folder (copy, original kept).
    migrate_legacy_data_dir(&data_dir);'''
MIGRATE_FN = '''
/// REBRAND: copy the upstream OpenAlgo data folder into the new app-id folder once.
fn migrate_legacy_data_dir(new_dir: &std::path::Path) {
    fn copy(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let (s, d) = (e.path(), dst.join(e.file_name()));
            if e.file_type()?.is_dir() { copy(&s, &d)? } else if e.file_name() != "LOCK" && !e.file_name().to_string_lossy().ends_with(".lock") { std::fs::copy(&s, &d)?; }
        }
        Ok(())
    }
    if new_dir.exists() { return; }
    let Some(parent) = new_dir.parent() else { return };
    let old = parent.join("%OLD%");
    if old.is_dir() && old != new_dir {
        match copy(&old, new_dir) {
            Ok(()) => tracing::info!("Copied legacy data folder {:?} to {:?}", old, new_dir),
            Err(e) => { tracing::error!("Legacy data copy failed: {}", e); let _ = std::fs::remove_dir_all(new_dir); }
        }
    }
}
'''.replace("%OLD%", OLD_ID)

README = """# {name}

**{name}** is a desktop trading app by [{company}]({web}) for Indian brokers:
charts, order entry, strategy tools, webhooks (TradingView, GoCharting and more) and a local API,
running entirely on your own computer.

> Based on [OpenAlgo Desktop](https://github.com/marketcalls/openalgo-desktop) (Copyright 2024 OpenAlgo),
> licensed under the GNU AGPL-3.0. {name} is a modified version; see [MODIFICATIONS.md](MODIFICATIONS.md).

## Download
Get the latest Windows installer (.exe) or macOS disk image (.dmg) from [Releases]({src}/releases/latest).
Installed copies check for updates automatically and ask before installing.

## Build from source
See [WINDOWS_BUILD.md](WINDOWS_BUILD.md). In short: install Node 22 and Rust, then
```
npm ci
npx tauri build --bundles nsis
```

## Links
- Website: {web}
- Support: {email}
- X: {x}
- YouTube: {yt}
- Source code: {src}

## Licence
GNU Affero General Public License v3.0, see [License.md](License.md). The complete corresponding source
of every release is in this repository. Original work Copyright 2024 OpenAlgo; modifications Copyright {company}.
"""

DOWNLOAD_PAGE = """import { Apple, Download as DownloadIcon, ExternalLink, Github, Monitor } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'

// REBRAND: download page for %NAME% (installers are published on GitHub Releases).
const RELEASES = '%SRC%/releases'
const LATEST = `${RELEASES}/latest`

const platforms = [
  { id: 'windows', title: 'Windows', icon: Monitor, file: 'Windows installer (.exe, 64-bit)', note: 'Windows 10 or 11.' },
  { id: 'macos', title: 'macOS', icon: Apple, file: 'macOS disk image (.dmg, Apple Silicon and Intel)', note: 'Not yet signed by Apple: right-click the app and choose Open the first time.' },
]

export default function Download() {
  return (
    <div className="container mx-auto max-w-4xl px-4 py-10 space-y-8">
      <div className="text-center space-y-2">
        <h1 className="text-3xl font-bold">Download %NAME%</h1>
        <p className="text-muted-foreground">
          Get the latest version. Installed copies check for updates and ask before installing.
        </p>
      </div>
      <div className="grid gap-6 md:grid-cols-2">
        {platforms.map((p) => (
          <Card key={p.id}>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <p.icon className="h-5 w-5" />
                {p.title}
              </CardTitle>
              <CardDescription>{p.file}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <Button asChild className="w-full">
                <a href={LATEST} target="_blank" rel="noopener noreferrer">
                  <DownloadIcon className="mr-2 h-4 w-4" />
                  Download %NAME% for {p.title}
                </a>
              </Button>
              <p className="text-xs text-muted-foreground">{p.note}</p>
            </CardContent>
          </Card>
        ))}
      </div>
      <div className="flex flex-wrap justify-center gap-3">
        <Button variant="outline" asChild>
          <a href={RELEASES} target="_blank" rel="noopener noreferrer">
            <ExternalLink className="mr-2 h-4 w-4" />
            All releases
          </a>
        </Button>
        <Button variant="outline" asChild>
          <a href="%SRC%" target="_blank" rel="noopener noreferrer">
            <Github className="mr-2 h-4 w-4" />
            Source code
          </a>
        </Button>
      </div>
      <p className="text-center text-xs text-muted-foreground">
        %NAME% is based on OpenAlgo Desktop and licensed under the GNU AGPL-3.0.
      </p>
    </div>
  )
}
"""

def dark_tile(src, bg, out):
    """The logo has white text/B on transparency, invisible on light headers
    and taskbars. Composite it on a dark rounded square (1024px, ~22% corner
    radius, 6% padding) so it reads on any background. Idempotent."""
    from PIL import Image, ImageDraw
    logo = Image.open(src).convert("RGBA")
    S = 1024
    tile = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    mask = Image.new("L", (S * 4, S * 4), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, S * 4 - 1, S * 4 - 1), radius=int(S * 4 * 0.22), fill=255)
    mask = mask.resize((S, S), Image.LANCZOS)
    tile.paste(Image.new("RGBA", (S, S), bg), (0, 0), mask)
    pad = int(S * 0.06)
    inner = S - 2 * pad
    lw, lh = logo.size
    sc = inner / max(lw, lh)
    logo = logo.resize((max(1, int(lw * sc)), max(1, int(lh * sc))), Image.LANCZOS)
    tile.alpha_composite(logo, ((S - logo.width) // 2, (S - logo.height) // 2))
    out = pathlib.Path(out); out.parent.mkdir(parents=True, exist_ok=True)
    tile.save(out)
    return out


def main():
    a = argparse.ArgumentParser()
    for k in ("repo", "name", "slug", "app-id", "company", "website"): a.add_argument("--" + k, required=True)
    a.add_argument("--icon"); a.add_argument("--source-url", default="https://SOURCE-REPO-URL-PLACEHOLDER")
    a.add_argument("--skip-icons", action="store_true")
    a.add_argument("--icon-bg", default="#000000",
                   help="put the (transparent, white-text) logo on a dark rounded square; 'none' keeps it as is")
    a.add_argument("--x-url", default="https://x.com/i_amArko")
    a.add_argument("--youtube-url", default="https://www.youtube.com/@DerivativesIndicators")
    a.add_argument("--support-email", default="admin@derivativesindicators.com")
    a.add_argument("--updater-pubkey-file", help="base64 .pub from `tauri signer generate`")
    a.add_argument("--updater-key-file", help="private key; with --set-secrets stored as repo secret")
    a.add_argument("--updater-password-file")
    a.add_argument("--set-secrets", metavar="OWNER/REPO", help="gh secret set TAURI_SIGNING_PRIVATE_KEY[_PASSWORD] (idempotent)")
    o = a.parse_args(); R = pathlib.Path(o.repo).resolve(); n = 0
    year = datetime.date.today().year

    # 1. tauri.conf.json
    cp = R / "src-tauri/tauri.conf.json"; c = json.loads(cp.read_text())
    c["productName"] = o.name; c["identifier"] = o.app_id
    c["bundle"]["shortDescription"] = "Algorithmic Trading Platform"
    c["bundle"]["longDescription"] = f"{o.name} by {o.company}, based on OpenAlgo Desktop (AGPL-3.0). Connects to Indian brokers and executes trades via webhooks and APIs."
    c["bundle"]["copyright"] = f"Copyright 2024 OpenAlgo; Modifications Copyright {year} {o.company}. AGPL-3.0."
    up = c["plugins"]["updater"]
    old_pub, old_ep = up.get("pubkey"), (up.get("endpoints") or [None])[0]
    pub = pathlib.Path(o.updater_pubkey_file).read_text().strip() if o.updater_pubkey_file else "REPLACE_WITH_OUR_UPDATER_PUBKEY"
    ep = (o.source_url.rstrip("/") + "/releases/latest/download/latest.json") if o.source_url.startswith("https://github.com/") else "https://UPDATE-SERVER-PLACEHOLDER.invalid/latest.json"
    up["endpoints"] = [ep]
    up["pubkey"] = pub
    import base64
    key_id = base64.b64decode(pub).decode().splitlines()[0].split(": ")[-1] if o.updater_pubkey_file else None
    new = json.dumps(c, indent=2) + "\n"
    if cp.read_text() != new: cp.write_text(new); n += 1

    # 2. package names (Cargo lib name kept: openalgo_desktop_lib is a code identifier)
    # Cargo package name is kept as `openalgo-desktop`: the crate depends on itself as a dev-dependency
    # (test-support feature) and the MCP CLI/user-agent use it. Installer/exe names come from productName.
    n += edit(R / "src-tauri/Cargo.toml", lambda s: re.sub(r'(?m)^name = "[a-z0-9-]+"\n(version = "[^"]+"\n)', lambda m: 'name = "openalgo-desktop"\n' + m.group(1), s, count=1))
    n += edit(R / "package.json", lambda s: s.replace('"name": "openalgo-desktop"', f'"name": "{o.slug}"', 1))
    # updater URLs used in UI/release notes
    for f in ("src-tauri/src/services/update/mod.rs",):
        n += edit(R / f, lambda s: s.replace('"https://github.com/marketcalls/openalgo-desktop/releases"', f'"{o.source_url}/releases"'))

    # 3. data folder migration (DB file names deliberately kept)
    def mig(s):
        if "migrate_legacy_data_dir" in s: return s
        s = s.replace("    let data_dir = app.path().app_data_dir()?;", MIGRATE, 1)
        return s + MIGRATE_FN
    n += edit(R / "src-tauri/src/lib.rs", mig)

    # 4. user-visible strings
    PUB = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEzMzc5QTJBRTVDMEFCNzEKUldSeHE4RGxLcG8zRXllUlJUVll3VmR6ckMxZzY5QXFqaGxpc3VLMTEzSEcyaDRGeEluODdXTFEK"
    UPREL = "https://github.com/marketcalls/openalgo-desktop/releases"
    for p in list((R / "src-tauri/src").rglob("*.rs")) + list((R / "src-tauri/tests").rglob("*.rs")):
        # tests that pin the upstream updater key/address follow our placeholders
        def upd(s):
            for old in {PUB, "REPLACE_WITH_OUR_UPDATER_PUBKEY", old_pub} - {None, pub}: s = s.replace(f'"{old}"', f'"{pub}"')
            for old in {UPREL + "/latest/download/latest.json", "https://UPDATE-SERVER-PLACEHOLDER.invalid/latest.json", old_ep} - {None, ep}: s = s.replace(f'"{old}"', f'"{ep}"')
            s = s.replace(UPREL, o.source_url + "/releases").replace("https://SOURCE-REPO-URL-PLACEHOLDER", o.source_url)
            if key_id: s = re.sub(r"(untrusted comment: minisign public key: )[0-9A-F]{16}", r"\g<1>" + key_id, s)
            return s
        n += edit(p, upd)
    for p in list((R / "src-tauri/src").rglob("*.rs")) + list((R / "src-tauri/tests").rglob("*.rs")):
        if p.name == "keystore.rs": continue
        n += edit(p, lambda s: rs_multiline_fix(rs_fix(s, o.name), o.name))
    for p in list((R / "src").rglob("*.ts")) + list((R / "src").rglob("*.tsx")):
        n += edit(p, lambda s: ts_fix(s, o.name))
    for h in ("index.html", "openscript-runner.html"):
        if (R / h).exists(): n += edit(R / h, lambda s: brand_text(s, o.name))

    # 5. footer: own website + attribution + source link
    def foot(s):
        s = s.replace('href="https://www.openalgo.in"', f'href="{o.website}"')
        s = re.sub(r">\s*www\.openalgo\.in\s*<", ">" + re.sub(r"^https?://", "", o.website).rstrip("/") + "<", s)
        s = s.replace("<span>Copyright 2026</span>", f"<span>Copyright {year} {o.company}</span>")
        if "Based on OpenAlgo (AGPL-3.0)" not in s:
            s = s.replace('<span className="text-center">Open Source Algo Platform for Everyone</span>',
                '<span className="text-center">Based on OpenAlgo (AGPL-3.0)</span>\n'
                '          <a href="https://github.com/marketcalls/openalgo-desktop" className="text-primary hover:underline" target="_blank" rel="noopener noreferrer">Upstream</a>\n'
                f'          <a data-rebrand="source" href="{o.source_url}" className="text-primary hover:underline" target="_blank" rel="noopener noreferrer">Source code</a>', 1)
        else:
            s = re.sub(r'(data-rebrand="source" href=")[^"]*', r"\g<1>" + o.source_url, s)
        return s
    n += edit(R / "src/components/layout/Footer.tsx", foot)
    # upstream community links removed (own community links to be added later)
    def nosocial(s):
        s = re.sub(r'\n          <Button(?:(?!</Button>).)*?openalgo\.in/discord.*?</Button>', "", s, flags=re.S)
        if o.x_url: s = s.replace("https://x.com/openalgoHQ", o.x_url)
        else: s = re.sub(r'\n          <Button(?:(?!</Button>).)*?x\.com/openalgoHQ.*?</Button>', "", s, flags=re.S)
        if o.youtube_url: s = s.replace("https://www.youtube.com/@openalgo", o.youtube_url)
        else: s = re.sub(r'\n          <Button(?:(?!</Button>).)*?youtube\.com/@openalgo.*?</Button>', "", s, flags=re.S)
        # GitHub icon -> our source repo
        s = s.replace('href="https://github.com/marketcalls/openalgo"', f'href="{o.source_url}"')
        X_SVG = '<path d="M18.901 1.153h3.68l-8.04 9.19L24 22.846h-7.406l-5.8-7.584-6.638 7.584H.474l8.6-9.83L0 1.154h7.594l5.243 6.932ZM17.61 20.644h2.039L6.486 3.24H4.298Z" />'
        YT_SVG = '<path d="M23.498 6.186a3.016 3.016 0 0 0-2.122-2.136C19.505 3.545 12 3.545 12 3.545s-7.505 0-9.377.505A3.017 3.017 0 0 0 .502 6.186C0 8.07 0 12 0 12s0 3.93.502 5.814a3.016 3.016 0 0 0 2.122 2.136c1.871.505 9.376.505 9.376.505s7.505 0 9.377-.505a3.015 3.015 0 0 0 2.122-2.136C24 15.93 24 12 24 12s0-3.93-.502-5.814zM9.545 15.568V8.432L15.818 12l-6.273 3.568z" />'
        def btn(tag, url, title, label, svg):
            return (f'\n          <Button data-rebrand="{tag}" variant="ghost" size="icon" asChild className="h-8 w-8" title="{title}">\n'
                    f'            <a href="{url}" target="_blank" rel="noopener noreferrer" aria-label="{label}">\n'
                    f'              <svg className="h-4 w-4" fill="currentColor" viewBox="0 0 24 24">\n                {svg}\n              </svg>\n'
                    '            </a>\n          </Button>')
        s = re.sub(r'\n          <Button data-rebrand="social-[a-z]+".*?</Button>', "", s, flags=re.S)
        extra = (btn("social-x", o.x_url, "Follow us on X", "X (Twitter)", X_SVG) if o.x_url else "") + \
                (btn("social-youtube", o.youtube_url, "Subscribe on YouTube", "YouTube", YT_SVG) if o.youtube_url else "")
        anchor = '<Github className="h-4 w-4" />\n            </a>\n          </Button>'
        s = s.replace(anchor, anchor + extra, 1)
        return s
    n += edit(R / "src/components/layout/Footer.tsx", nosocial)
    ft = R / "src/components/layout/Footer.test.tsx"
    if ft.exists():
        n += edit(ft, lambda s: s.replace("'Copyright 2026'", f"'Copyright {year} {o.company}'")
                  .replace("'Open Source Algo Platform for Everyone'", "'Based on OpenAlgo (AGPL-3.0)'"))

    # 5b. links/details: upstream docs/community/support -> ours (credit + licence links kept)
    web = o.website.rstrip("/"); host = re.sub(r"^https?://", "", web)
    LINKS = [
        (r"https?://(?:www\.)?github\.com/marketcalls/openalgo(?![-\w])", o.source_url),
        (r"https?://docs\.openalgo\.in(?:/[\w./#-]*)?", web),
        (r"https?://(?:www\.)?openalgo\.in(?:/[\w./#-]*)?", web),
        (r"(?<=['\"])openalgo\.in(?=['\"])", host),
        (r"[\w.+-]+@openalgo\.in", o.support_email),
        (r"OpenAlgo YouTube channel", host),
    ]
    LABELS = [("Restart the openalgo service to apply.", f"Restart {o.name} to apply."),
              ("`openalgo-search-${ts}.csv`", f"`{o.slug}-search-${{ts}}.csv`"),("'Varsity'", "'Learn'"), ("Discord Community", "Community"), ("Join Discord", "Visit website"),
              ("label: 'GitHub'", "label: 'Source code'")]
    def links(s):
        for pat, rep in LINKS: s = re.sub(pat, rep, s)
        for a_, b_ in LABELS: s = s.replace(a_, b_)
        return s
    for p in list((R / "src").rglob("*.ts")) + list((R / "src").rglob("*.tsx")) + list((R / "public").rglob("*.md")) + list((R / "public").rglob("*.html")):
        if "chunkReload" in p.name or p.name == "desktop.test.ts": continue  # upstream bug citation; URL-handling fixtures
        n += edit(p, links)
    n += edit(R / "src-tauri/Cargo.toml", lambda s: re.sub(r'(?m)^authors = \[.*\]$', f'authors = ["{o.company}"]', s, count=1))
    pj = R / "package.json"
    def pkg(s):
        d = json.loads(s); d["author"] = f"{o.company} <{o.support_email}>"; d["homepage"] = web
        d["repository"] = {"type": "git", "url": o.source_url + ".git"}; d["license"] = "AGPL-3.0"
        return json.dumps(d, indent=2, ensure_ascii=False) + "\n"
    n += edit(pj, pkg)
    c = json.loads(cp.read_text()); c["bundle"]["publisher"] = o.company; c["bundle"]["homepage"] = web
    new = json.dumps(c, indent=2) + "\n"
    if cp.read_text() != new: cp.write_text(new); n += 1

    FAQ = {
        "What are the costs involved?": f"{o.name} is free and open-source under the AGPL-3.0 licence (it is based on OpenAlgo Desktop). There are no licensing fees or subscriptions. You only pay standard brokerage charges from your broker.",
        "Can I use The Bridge for my proprietary trading strategies?": "Yes. The AGPL-3.0 licence allows free use for your own trading. If you modify the app and let others use it over a network, you must make your modified source available to them.",
        "Can I rebrand The Bridge for commercial use?": "Under the AGPL-3.0 licence you may modify and redistribute it, provided your version stays under AGPL-3.0, keeps the copyright and licence notices, and credits the original OpenAlgo project.",
        "Can I charge others for using my The Bridge setup?": "If you provide a modified version as a service to others, the AGPL-3.0 licence requires you to offer them its source code.",
    }
    def faq(s):
        for q, ans in FAQ.items():
            q = q.replace("The Bridge", o.name); ans = ans.replace("'", "\\'")
            s = re.sub(r"(question: '" + re.escape(q) + r"',\s*answer:\s*)'(?:[^'\\]|\\.)*'", lambda m: m.group(1) + "'" + ans + "'", s)
        return s
    if (R / "src/pages/Faq.tsx").exists(): n += edit(R / "src/pages/Faq.tsx", faq)
    wb = R.parent / "WINDOWS_BUILD.md"
    if wb.exists() and not (R / "WINDOWS_BUILD.md").exists(): shutil.copy(wb, R / "WINDOWS_BUILD.md"); n += 1
    if (R / "WINDOWS_BUILD.md").exists():
        n += edit(R / "WINDOWS_BUILD.md", lambda s: s.replace("<YOUR-SOURCE-REPO-URL>", o.source_url))
    dl = R / "src/pages/Download.tsx"
    if dl.exists():
        page = DOWNLOAD_PAGE.replace("%NAME%", o.name).replace("%SRC%", o.source_url)
        n += edit(dl, lambda s: page)
    def dlbtn(s):
        s = re.sub(r'(<Download className="(?:h-5 w-5|mr-2 h-5 w-5|h-4 w-4 mr-2)" />\n\s*)Download\n', lambda m: m.group(1) + f"Download {o.name}\n", s)
        return s.replace('<Button size="sm">Download</Button>', f'<Button size="sm">Download {o.name}</Button>')
    for f in ("Home", "Faq", "NotFound", "ServerError"):
        if (R / f"src/pages/{f}.tsx").exists(): n += edit(R / f"src/pages/{f}.tsx", dlbtn)
    sg = R / "src-tauri/src/brokers/setup_guide.rs"
    if sg.exists():  # test expectation follows the rebranded banner wording
        n += edit(sg, lambda s: s.replace('banner.contains("back to openalgo")', f'banner.contains("back to {o.name.lower()}")'))
    readme = README.format(name=o.name, company=o.company, web=web, host=host, src=o.source_url,
                           email=o.support_email, x=o.x_url, yt=o.youtube_url)
    n += edit(R / "README.md", lambda s: readme)

    # 6. icons
    if o.icon and not o.skip_icons:
        if o.icon_bg.lower() != "none":
            o.icon = str(dark_tile(o.icon, o.icon_bg, R / "src-tauri/icons/.source-dark.png"))
        subprocess.run(["npx", "--yes", "tauri", "icon", str(pathlib.Path(o.icon).resolve())], cwd=R, check=True)
        ic = R / "src-tauri/icons"
        shutil.copy(o.icon, R / "public/logo.png"); shutil.copy(ic / "icon.ico", R / "public/favicon.ico")
        shutil.copy(ic / "128x128@2x.png", R / "public/apple-touch-icon.png")
        try:
            from PIL import Image
            Image.open(o.icon).convert("RGBA").resize((192, 192), Image.LANCZOS).save(R / "public/images/android-chrome-192x192.png")
        except ImportError:
            shutil.copy(ic / "128x128@2x.png", R / "public/images/android-chrome-192x192.png")
        for svg in ("openalgo-glyph.svg", "openalgo-mark.svg"):  # unused upstream marks
            (R / "public/images" / svg).unlink(missing_ok=True)

    # 7. MODIFICATIONS.md (AGPL 5a)
    mp = R / "MODIFICATIONS.md"
    head = ("# Modifications\n\nThis program is a modified version of OpenAlgo Desktop "
            "(https://github.com/marketcalls/openalgo-desktop, Copyright 2024 OpenAlgo), "
            "licensed under the GNU AGPL-3.0 (see License.md). Modified versions carry this notice per AGPL-3.0 section 5(a).\n\n")
    entry = (f"## {datetime.date.today()} - {o.company}\n"
             f"- Renamed product to \"{o.name}\" (package `{o.slug}`, app id `{o.app_id}`); window titles and user-visible text.\n"
             "- Replaced icons/logo. Footer: own website, 'Based on OpenAlgo (AGPL-3.0)', source-code link.\n"
             + ("- Auto-updater points to our own GitHub releases, signed with our own key.\n" if o.updater_pubkey_file else "- Disabled upstream auto-updater endpoint/pubkey (placeholder for our own).\n") +
             "- Removed upstream community (Discord/X/YouTube) links from the footer.\n"
             f"- Added one-time copy of the legacy `{OLD_ID}` data folder into the new app-id folder.\n"
             f"- Corresponding Source: {o.source_url}\n\n")
    cur = mp.read_text().replace("https://SOURCE-REPO-URL-PLACEHOLDER", o.source_url) if mp.exists() else head
    if entry not in cur: mp.write_text(cur + entry); n += 1
    # 8. updater signing secrets (never printed)
    if o.set_secrets and o.updater_key_file:
        for name, f in (("TAURI_SIGNING_PRIVATE_KEY", o.updater_key_file), ("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", o.updater_password_file)):
            if f:
                subprocess.run(["gh", "secret", "set", name, "--repo", o.set_secrets], input=pathlib.Path(f).read_bytes(), check=True, stdout=subprocess.DEVNULL)
                print(f"secret {name} set on {o.set_secrets}")
    print(f"rebrand done: {n} files changed")

if __name__ == "__main__":
    main()
