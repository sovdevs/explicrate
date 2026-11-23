use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, thread::sleep, time::{Duration, SystemTime}};

const UA: &str = "explicrate/0.1 (crate lookup CLI)";
const DAY: u64 = 86400;

type R<T> = Result<T, Box<dyn std::error::Error>>;

fn cache_dir() -> PathBuf {
    if let Ok(d) = std::env::var("EXPLICRATE_CACHE") { return d.into() }
    let base = std::env::var("XDG_CACHE_HOME").map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("explicrate")
}

fn cache_name(url: &str) -> String {
    url.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Data bundled in the binary (src/seed.json, made by `make-seed`): cache file name -> body.
fn seed() -> &'static std::collections::HashMap<String, String> {
    static S: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();
    S.get_or_init(|| serde_json::from_str(include_str!("seed.json")).unwrap_or_default())
}

/// GET with on-disk cache. ttl None = cache forever.
fn fetch(url: &str, ttl: Option<u64>) -> R<String> {
    let path = cache_dir().join(cache_name(url));
    if let Ok(m) = fs::metadata(&path) {
        let age = SystemTime::now().duration_since(m.modified()?).map(|d| d.as_secs()).unwrap_or(0);
        if ttl.map_or(true, |t| age < t) {
            return Ok(fs::read_to_string(&path)?);
        }
    }
    if !path.exists() {
        if let Some(b) = seed().get(&cache_name(url)) {
            fs::create_dir_all(cache_dir())?;
            fs::write(&path, b)?;
            return Ok(b.clone());
        }
    }
    sleep(Duration::from_millis(150)); // ponytail: fixed politeness delay, no rate-limit parsing
    let mut req = ureq::get(url).timeout(Duration::from_secs(20)).set("User-Agent", UA);
    if let (true, Ok(t)) = (url.starts_with("https://api.github.com/"), std::env::var("GITHUB_TOKEN")) {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let body = req.call()?.into_string()?;
    fs::create_dir_all(cache_dir())?;
    fs::write(&path, &body)?;
    Ok(body)
}

#[derive(Serialize, Deserialize, Clone)]
struct Mention { #[serde(default)] date: String, #[serde(default)] usecase: Option<String>, name: String, source: String, issue: String, issue_url: String, blurb: String }

/// ISO date from an issue url: TWiR /blog/YYYY/MM/DD/, SciComp /monthly/YYYY-MM.
fn date_of(url: &str) -> String {
    if let Some(r) = url.split("/blog/").nth(1) {
        return r.split('/').take(3).collect::<Vec<_>>().join("-");
    }
    format!("{}-01", url.rsplit('/').next().unwrap_or(""))
}

fn sel(s: &str) -> Selector { Selector::parse(s).unwrap() }
fn text(e: ElementRef) -> String { e.text().collect::<Vec<_>>().join("").split_whitespace().collect::<Vec<_>>().join(" ") }
fn siblings(h: ElementRef) -> impl Iterator<Item = ElementRef> {
    h.next_siblings().filter_map(ElementRef::wrap)
}
fn crate_from_href(href: &str) -> Option<String> {
    href.split("crates.io/crates/").nth(1).map(|r| r.split(['/', '?', '#']).next().unwrap().to_string())
}

fn scrape_twir(url: &str, html: &str, out: &mut Vec<Mention>) {
    let doc = Html::parse_document(html);
    let Some(h) = doc.select(&sel("#crate-of-the-week")).next() else { return };
    let issue = doc.select(&sel("title")).next().map(text).unwrap_or_default().split(" · ").next().unwrap_or("").to_string();
    for p in siblings(h).take_while(|e| !matches!(e.value().name(), "h1" | "h2")).filter(|e| e.value().name() == "p") {
        let t = text(p);
        if t.starts_with("Thanks") || t.starts_with("Submit") || t.starts_with("Please submit") { continue }
        for a in p.select(&sel("a")) {
            let href = a.value().attr("href").unwrap_or("");
            out.push(Mention {
                date: date_of(url), usecase: None,
                name: crate_from_href(href).unwrap_or_else(|| text(a)),
                source: "This Week in Rust (crate of the week)".into(),
                issue: issue.clone(), issue_url: url.into(), blurb: t.clone(),
            });
        }
    }
}

fn scrape_scicomp(url: &str, html: &str, out: &mut Vec<Mention>) {
    let doc = Html::parse_document(html);
    let issue = text(doc.select(&sel("h1")).next().unwrap());
    for h in doc.select(&sel("h2,h3")) {
        let (htext, tag) = (text(h), h.value().name());
        let is_cotm = tag == "h2" && htext.starts_with("Crate of the month");
        if !is_cotm && tag != "h3" { continue }
        let body: Vec<_> = siblings(h).take_while(|e| !matches!(e.value().name(), "h1" | "h2" | "h3")).collect();
        let name = std::iter::once(h).chain(body.iter().copied()).flat_map(|e| e.select(&sel("a")).collect::<Vec<_>>())
            .find_map(|a| crate_from_href(a.value().attr("href")?));
        let Some(name) = name else { continue };
        out.push(Mention {
            date: date_of(url), usecase: None,
            name,
            source: format!("Scientific Computing in Rust ({})", if is_cotm { "crate of the month" } else { "new release" }),
            issue: issue.clone(), issue_url: url.into(),
            blurb: body.iter().map(|e| text(*e)).filter(|t| !t.starts_with("crates.io")).collect::<Vec<_>>().join(" "),
        });
    }
}

/// Scrape every issue of both sources into cache/feed.json.
fn sync() -> R<Vec<Mention>> {
    let mut out = vec![];
    let idx = fetch("https://this-week-in-rust.org/blog/archives/index.html", Some(DAY))?;
    let mut urls: Vec<String> = Html::parse_document(&idx).select(&sel("a")).filter_map(|a| a.value().attr("href"))
        .filter(|h| h.contains("/blog/") && h.contains("this-week-in-rust-")).map(String::from).collect();
    urls.sort(); urls.dedup();
    for (i, u) in urls.iter().enumerate() {
        eprint!("\rTWiR {}/{}", i + 1, urls.len());
        match fetch(u, None) { Ok(h) => scrape_twir(u, &h, &mut out), Err(e) => eprintln!("\n{u}: {e}") }
    }
    let all = fetch("https://scientificcomputing.rs/monthly/all", Some(DAY))?;
    let mut months: Vec<String> = Html::parse_document(&all).select(&sel("a")).filter_map(|a| a.value().attr("href"))
        .filter(|h| h.starts_with("/monthly/2")).map(|h| format!("https://scientificcomputing.rs{h}")).collect();
    months.sort(); months.dedup();
    for (i, u) in months.iter().enumerate() {
        eprint!("\rSciComp {}/{}", i + 1, months.len());
        // newest issue may still change; older ones are frozen
        let ttl = if i + 1 == months.len() { Some(DAY) } else { None };
        match fetch(u, ttl) { Ok(h) => scrape_scicomp(u, &h, &mut out), Err(e) => eprintln!("\n{u}: {e}") }
    }
    eprintln!();
    fs::write(cache_dir().join("feed.json"), serde_json::to_string(&out)?)?;
    fs::write(cache_dir().join("feed.full"), "")?;
    Ok(out)
}

fn feed() -> R<Vec<Mention>> {
    match fs::read_to_string(cache_dir().join("feed.json")) {
        Ok(s) => {
            let f: Vec<Mention> = serde_json::from_str(&s)?;
            if f.iter().any(|m| m.date.is_empty()) { sync() } else { Ok(f) }
        }
        Err(_) => match seed().get("feed.json") {
            Some(b) => { fs::create_dir_all(cache_dir())?; fs::write(cache_dir().join("feed.json"), b)?; Ok(serde_json::from_str(b)?) }
            None => sync(),
        },
    }
}

/// True until `sync` has scraped every issue (the bundled seed only has recent mentions).
fn feed_partial() -> bool { !cache_dir().join("feed.full").exists() }

#[derive(Serialize)]
struct CratesIo {
    version: String, description: String, downloads: u64, recent_downloads: Option<u64>,
    created: String, updated: String, license: String, keywords: Vec<String>,
    repository: Option<String>, documentation: Option<String>, homepage: Option<String>,
}
#[derive(Serialize)]
struct Report { name: String, crates_io: Option<CratesIo>, stars: Option<u64>, mentions: Vec<Mention>, downloads_rank: Option<Rank>, stars_rank: Option<Rank>, blessed: Vec<Blessed>, feed_partial: bool }

fn s(v: &serde_json::Value) -> Option<String> { v.as_str().map(String::from) }

fn crates_io(name: &str) -> Option<CratesIo> {
    let j: serde_json::Value = serde_json::from_str(&fetch(&format!("https://crates.io/api/v1/crates/{name}"), None).ok()?).ok()?;
    let c = &j["crate"];
    Some(CratesIo {
        version: s(&c["max_version"])?, description: s(&c["description"]).unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
        downloads: c["downloads"].as_u64()?, recent_downloads: c["recent_downloads"].as_u64(),
        created: s(&c["created_at"]).unwrap_or_default(), updated: s(&c["updated_at"]).unwrap_or_default(),
        license: s(&j["versions"][0]["license"]).unwrap_or_default(),
        keywords: c["keywords"].as_array().map(|a| a.iter().filter_map(s).collect()).unwrap_or_default(),
        repository: s(&c["repository"]), documentation: s(&c["documentation"]), homepage: s(&c["homepage"]),
    })
}

fn repo_api(repo: &str) -> Option<String> {
    let path = repo.split("github.com/").nth(1)?.trim_end_matches('/').trim_end_matches(".git");
    Some(format!("https://api.github.com/repos/{}", path.split('/').take(2).collect::<Vec<_>>().join("/")))
}

fn stars(repo: &str) -> Option<u64> {
    let j: serde_json::Value = serde_json::from_str(&fetch(&repo_api(repo)?, None).ok()?).ok()?;
    j["stargazers_count"].as_u64()
}

#[derive(Serialize, Deserialize, Default)]
struct Ranks { by_year: std::collections::BTreeMap<String, Vec<u64>>, stars: Vec<u64> } // each vec sorted ascending

#[derive(Serialize)]
struct Rank { top_pct: f64, decile: u32, of: usize, cohort: String }

fn rank_in(v: &[u64], x: u64, cohort: String) -> Option<Rank> {
    if v.is_empty() { return None }
    let greater = v.len() - v.partition_point(|&d| d <= x);
    let top_pct = (greater + 1) as f64 / v.len() as f64 * 100.0;
    Some(Rank { top_pct, decile: (top_pct / 10.0).ceil().clamp(1.0, 10.0) as u32, of: v.len(), cohort })
}

fn ranks() -> Ranks {
    fs::read_to_string(cache_dir().join("ranks.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_ranks(r: &Ranks) -> R<()> {
    fs::create_dir_all(cache_dir())?;
    Ok(fs::write(cache_dir().join("ranks.json"), serde_json::to_string(r)?)?)
}

/// Stream the crates.io db dump, keep only per-creation-year download counts (~2GB download).
fn rank_downloads() -> R<()> {
    let resp = ureq::get("https://static.crates.io/db-dump.tar.gz").set("User-Agent", UA).call()?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(resp.into_reader()));
    let (mut year, mut dl) = (std::collections::HashMap::new(), std::collections::HashMap::new());
    for e in tar.entries()? {
        let e = e?;
        let path = e.path()?.to_string_lossy().into_owned();
        let crates = path.ends_with("data/crates.csv");
        if !crates && !path.ends_with("data/crate_downloads.csv") { continue }
        let mut rd = csv::Reader::from_reader(e);
        let h = rd.headers()?.clone();
        let col = |n: &str| h.iter().position(|x| x == n).ok_or(format!("no column {n}"));
        let (id, v) = if crates { (col("id")?, col("created_at")?) } else { (col("crate_id")?, col("downloads")?) };
        for rec in rd.records() {
            let rec = rec?;
            let k: u64 = rec[id].parse()?;
            if crates { year.insert(k, rec[v][..4].to_string()); } else { dl.insert(k, rec[v].parse::<u64>()?); }
        }
        if !year.is_empty() && !dl.is_empty() { break }
    }
    let mut r = ranks();
    r.by_year.clear();
    for (id, y) in year { r.by_year.entry(y).or_default().push(dl.get(&id).copied().unwrap_or(0)); }
    r.by_year.values_mut().for_each(|v| v.sort_unstable());
    println!("{} crates in {} cohorts", r.by_year.values().map(Vec::len).sum::<usize>(), r.by_year.len());
    save_ranks(&r)
}

/// Stars for every crate in the feeds (needs GITHUB_TOKEN beyond ~60 repos/hour; rerun resumes from cache).
fn rank_stars() -> R<()> {
    let mut names: Vec<String> = feed()?.iter().map(|m| norm(&m.name)).filter(|n| !n.contains(' ')).collect();
    names.sort(); names.dedup();
    let mut v = vec![];
    for (i, n) in names.iter().enumerate() {
        eprint!("\r{}/{} ({} with stars)", i + 1, names.len(), v.len());
        if let Some(st) = crates_io(n).and_then(|c| c.repository).as_deref().and_then(stars) { v.push(st) }
    }
    eprintln!();
    v.sort_unstable();
    let mut r = ranks();
    r.stars = v;
    println!("{} featured crates with stars", r.stars.len());
    save_ranks(&r)
}

/// Write src/seed.json: recent + blessed crates' cached data and the matching feed mentions.
fn make_seed() -> R<()> {
    const BLESSED: &str = "https://raw.githubusercontent.com/nicoburns/blessed-rs/main/data/crates.json";
    let bl = blessed();
    let mut files = std::collections::BTreeMap::new();
    files.insert(cache_name(BLESSED), fetch(BLESSED, None)?);
    let mut names: Vec<String> = bl.values().flatten().map(|b| b.name.clone()).collect();
    names.extend(recent()?.into_iter().map(|m| m.name));
    names.sort(); names.dedup();
    for (i, n) in names.iter().enumerate() {
        eprintln!("seed {}/{} {n}", i + 1, names.len());
        let Some(c) = crates_io(n) else { continue };
        files.insert(cache_name(&format!("https://crates.io/api/v1/crates/{n}")), serde_json::json!({
            "crate": {"max_version": c.version, "description": c.description, "downloads": c.downloads, "recent_downloads": c.recent_downloads,
                      "created_at": c.created, "updated_at": c.updated, "keywords": c.keywords, "repository": c.repository,
                      "documentation": c.documentation, "homepage": c.homepage},
            "versions": [{"license": c.license}]}).to_string());
        if let Some(st) = c.repository.as_deref().and_then(stars) {
            files.insert(cache_name(&repo_api(c.repository.as_deref().unwrap()).unwrap()), serde_json::json!({"stargazers_count": st}).to_string());
        }
    }
    let cut = cutoff()?;
    let keep: Vec<Mention> = feed()?.into_iter().filter(|m| m.date >= cut || bl.contains_key(&norm(&m.name))).collect();
    files.insert("feed.json".into(), serde_json::to_string(&keep)?);
    let out = serde_json::to_string(&files)?;
    fs::write("src/seed.json", &out)?;
    println!("src/seed.json: {} files, {} KB, {} mentions", files.len(), out.len() / 1024, keep.len());
    Ok(())
}

/// Warm the cache (crates.io + stars) for the recent crates so sidebar clicks are instant.
fn prefetch() -> R<()> {
    let r = recent()?;
    for (i, m) in r.iter().enumerate() {
        eprintln!("prefetch {}/{} {}", i + 1, r.len(), m.name);
        let _ = lookup(&m.name);
    }
    Ok(())
}

fn cutoff() -> R<String> {
    let days = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs() as i64 / 86400 - 183;
    // civil-from-days (H. Hinnant)
    let z = days + 719468; let era = z.div_euclid(146097); let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let (m, d) = (if mp < 10 { mp + 3 } else { mp - 9 }, doy - (153 * mp + 2) / 5 + 1);
    Ok(format!("{:04}-{:02}-{:02}", yoe + era * 400 + (m <= 2) as i64, m, d))
}

/// Crates whose first-ever mention in either feed is within the last ~6 months, newest first.
fn recent() -> R<Vec<Mention>> {
    let cutoff = cutoff()?;
    let mut f = feed()?;
    f.sort_by(|a, b| a.date.cmp(&b.date));
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Mention> = f.into_iter().filter(|m| !m.name.contains(' ') && seen.insert(norm(&m.name))).filter(|m| m.date >= cutoff).collect();
    out.reverse();
    let bl = blessed();
    for m in &mut out { m.usecase = bl.get(&norm(&m.name)).and_then(|v| v.first()).map(|b| b.purpose.clone()) }
    Ok(out)
}

/// A crate's entry in blessed.rs (https://blessed.rs), used as the use-case source.
#[derive(Serialize, Clone)]
struct Blessed { name: String, group: String, subgroup: String, purpose: String, role: &'static str, notes: String, purpose_notes: String, url: String }

fn strip_html(h: &str) -> String {
    let (mut out, mut tag) = (String::new(), false);
    for c in h.chars() { match c { '<' => tag = true, '>' => { tag = false; out.push(' ') } _ if !tag => out.push(c), _ => {} } }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// blessed.rs data, keyed by normalised crate name. Empty if unavailable (it's optional).
fn blessed() -> std::collections::HashMap<String, Vec<Blessed>> {
    let mut map: std::collections::HashMap<String, Vec<Blessed>> = Default::default();
    let Ok(body) = fetch("https://raw.githubusercontent.com/nicoburns/blessed-rs/main/data/crates.json", None) else { return map };
    let Ok(j) = serde_json::from_str::<serde_json::Value>(&body) else { return map };
    let st = |v: &serde_json::Value| strip_html(v.as_str().unwrap_or(""));
    for g in j["crate_groups"].as_array().into_iter().flatten() {
        for sg in g["subgroups"].as_array().into_iter().flatten() {
            let url = format!("https://blessed.rs/crates#section-{}-subsection-{}", g["slug"].as_str().unwrap_or(""), sg["slug"].as_str().unwrap_or(""));
            for p in sg["purposes"].as_array().into_iter().flatten() {
                for (key, role) in [("recommendations", "recommended"), ("see_also", "see also")] {
                    for c in p[key].as_array().into_iter().flatten() {
                        map.entry(norm(c["name"].as_str().unwrap_or(""))).or_default().push(Blessed {
                            name: c["name"].as_str().unwrap_or("").into(), group: st(&g["name"]), subgroup: st(&sg["name"]), purpose: st(&p["name"]), role,
                            notes: st(&c["notes"]), purpose_notes: st(&p["notes"]), url: url.clone(),
                        });
                    }
                }
            }
        }
    }
    map
}

fn norm(n: &str) -> String { n.to_lowercase().replace('_', "-") }

fn lookup(name: &str) -> R<Report> {
    let n = norm(name);
    let bl = blessed().remove(&n).unwrap_or_default();
    let usecase = bl.first().map(|b| b.purpose.clone());
    let mentions = feed()?.into_iter().filter(|m| norm(&m.name) == n).map(|m| Mention { usecase: usecase.clone(), ..m }).collect();
    let c = crates_io(name);
    let stars = c.as_ref().and_then(|c| c.repository.as_deref()).and_then(stars);
    let rk = ranks();
    let downloads_rank = c.as_ref().and_then(|c| {
        let y = &c.created[..4.min(c.created.len())];
        rank_in(rk.by_year.get(y)?, c.downloads, format!("crates created in {y}"))
    });
    let stars_rank = stars.and_then(|s| rank_in(&rk.stars, s, "featured crates".into()));
    Ok(Report { name: name.into(), crates_io: c, stars, mentions, downloads_rank, stars_rank, blessed: bl, feed_partial: feed_partial() })
}

fn print(r: &Report) {
    println!("{}", r.name);
    match &r.crates_io {
        None => println!("  not found on crates.io"),
        Some(c) => {
            println!("  {}\n  version {} · license {} · created {} · updated {}", c.description, c.version, c.license, &c.created[..10.min(c.created.len())], &c.updated[..10.min(c.updated.len())]);
            println!("  downloads {} ({} recent) · stars {}", c.downloads, c.recent_downloads.unwrap_or(0), r.stars.map_or("?".into(), |s| s.to_string()));
            for (k, rk) in [("downloads", &r.downloads_rank), ("stars", &r.stars_rank)] {
                if let Some(rk) = rk { println!("  {k} rank: top {}% (decile {}) of {} {}", if rk.top_pct < 0.1 { "<0.1".into() } else { format!("{:.1}", rk.top_pct) }, rk.decile, rk.of, rk.cohort) }
            }
            println!("  keywords: {}", c.keywords.join(", "));
            for (k, v) in [("repo", &c.repository), ("docs", &c.documentation), ("home", &c.homepage)] {
                if let Some(v) = v { println!("  {k}: {v}") }
            }
        }
    }
    for b in &r.blessed {
        println!("  use case: {} › {} › {} [{}] {}\n    {}", b.group, b.subgroup, b.purpose, b.role, b.notes, b.url);
    }
    println!("  mentions: {}{}", r.mentions.len(), if r.feed_partial { "  (recent issues only; run `explicrate sync` for full history)" } else { "" });
    for m in &r.mentions { println!("   - [{} · {}] {}", m.source, m.issue, m.blurb) }
}

fn serve(addr: &str) -> R<()> {
    let srv = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    eprintln!("http://{addr}");
    std::thread::spawn(|| { let _ = prefetch(); });
    for req in srv.incoming_requests() {
        let url = req.url().to_string();
        let (body, ctype) = match url.strip_prefix("/api/crate/") {
            _ if url == "/api/recent" => (serde_json::to_string(&recent().unwrap_or_default())?, "application/json"),
            Some(n) if n.chars().all(|c| c.is_ascii_alphanumeric() || "-_%".contains(c)) => {
                let n = n.replace("%2F", "");
                let j = match lookup(&n) { Ok(r) => serde_json::to_string(&r)?, Err(e) => serde_json::json!({"error": e.to_string()}).to_string() };
                (j, "application/json")
            }
            Some(_) => (r#"{"error":"bad crate name"}"#.into(), "application/json"),
            None => (include_str!("index.html").into(), "text/html; charset=utf-8"),
        };
        let _ = req.respond(tiny_http::Response::from_string(body).with_header(
            tiny_http::Header::from_bytes("Content-Type", ctype).unwrap()));
    }
    Ok(())
}

fn main() -> R<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["sync"] => { let n = sync()?.len(); println!("{n} mentions"); }
        ["make-seed"] => make_seed()?,
        ["prefetch"] => prefetch()?,
        ["rank"] => rank_downloads()?,
        ["rank-stars"] => rank_stars()?,
        ["recent"] => for m in recent()? { println!("{}  {}  ({})", m.date, m.name, m.source) },
        ["serve"] => serve("127.0.0.1:8080")?,
        ["serve", addr] => serve(addr)?,
        [] => eprintln!("usage: explicrate <crate>... | sync | prefetch | rank | rank-stars | recent | serve [addr]"),
        _ => for n in &a { print(&lookup(n)?) },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_twir_and_scicomp() {
        let mut v = vec![];
        scrape_twir("u", r#"<title>Issue 1 · TWiR</title><h1 id="crate-of-the-week">x</h1><p>This week's crate is <a href="https://crates.io/crates/nom-supreme">nom-supreme</a>, utils.</p><p>Thanks to <a href="x">Z</a></p><h1>next</h1>"#, &mut v);
        assert_eq!(v.len(), 1); assert_eq!(v[0].name, "nom-supreme"); assert_eq!(v[0].issue, "Issue 1");
        scrape_scicomp("u", r#"<h1>#1</h1><h2>Crate of the month: Jiff</h2><p><a href="https://crates.io/crates/jiff">crates.io</a></p><p>Dates.</p><h2>New releases</h2><h3><a href="https://crates.io/crates/oximo">oximo v1</a></h3><p>Opt.</p>"#, &mut v);
        assert_eq!(v.len(), 3); assert_eq!(v[1].name, "jiff"); assert_eq!(v[1].blurb, "Dates."); assert_eq!(v[2].name, "oximo");
    }
}
