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

def main():
    a = argparse.ArgumentParser()
    for k in ("repo", "name", "slug", "app-id", "company", "website"): a.add_argument("--" + k, required=True)
    a.add_argument("--icon"); a.add_argument("--source-url", default="https://SOURCE-REPO-URL-PLACEHOLDER")
    a.add_argument("--skip-icons", action="store_true")
    o = a.parse_args(); R = pathlib.Path(o.repo).resolve(); n = 0
    year = datetime.date.today().year

    # 1. tauri.conf.json
    cp = R / "src-tauri/tauri.conf.json"; c = json.loads(cp.read_text())
    c["productName"] = o.name; c["identifier"] = o.app_id
    c["bundle"]["shortDescription"] = "Algorithmic Trading Platform"
    c["bundle"]["longDescription"] = f"{o.name} by {o.company}, based on OpenAlgo Desktop (AGPL-3.0). Connects to Indian brokers and executes trades via webhooks and APIs."
    c["bundle"]["copyright"] = f"Copyright 2024 OpenAlgo; Modifications Copyright {year} {o.company}. AGPL-3.0."
    up = c["plugins"]["updater"]
    up["endpoints"] = ["https://UPDATE-SERVER-PLACEHOLDER.invalid/latest.json"]
    up["pubkey"] = "REPLACE_WITH_OUR_UPDATER_PUBKEY"
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
        n += edit(p, lambda s: s.replace(PUB, "REPLACE_WITH_OUR_UPDATER_PUBKEY")
                  .replace(UPREL + "/latest/download/latest.json", "https://UPDATE-SERVER-PLACEHOLDER.invalid/latest.json")
                  .replace(UPREL, o.source_url + "/releases"))
    for p in list((R / "src-tauri/src").rglob("*.rs")) + list((R / "src-tauri/tests").rglob("*.rs")):
        if p.name == "keystore.rs": continue
        n += edit(p, lambda s: rs_fix(s, o.name))
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
    ft = R / "src/components/layout/Footer.test.tsx"
    if ft.exists():
        n += edit(ft, lambda s: s.replace("'Copyright 2026'", f"'Copyright {year} {o.company}'")
                  .replace("'Open Source Algo Platform for Everyone'", "'Based on OpenAlgo (AGPL-3.0)'"))

    # 6. icons
    if o.icon and not o.skip_icons:
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
             "- Disabled upstream auto-updater endpoint/pubkey (placeholder for our own).\n"
             f"- Added one-time copy of the legacy `{OLD_ID}` data folder into the new app-id folder.\n"
             f"- Corresponding Source: {o.source_url}\n\n")
    cur = mp.read_text() if mp.exists() else head
    if entry not in cur: mp.write_text(cur + entry); n += 1
    print(f"rebrand done: {n} files changed")

if __name__ == "__main__":
    main()
