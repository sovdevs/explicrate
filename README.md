# explicrate

Look up a crate by the name you'd give `cargo add`: what it is, stats, and mentions in
This Week in Rust (crate of the week) and Scientific Computing in Rust Monthly (crate of the month / new releases).

## Run

```
cargo install explicrate
explicrate anyhow thiserror   # look up one or more crates
explicrate serve              # web page on http://127.0.0.1:8080
explicrate serve 0.0.0.0:3000 # custom address
explicrate sync               # rescrape both feeds
explicrate prefetch           # warm the cache for the last-6-months crates
explicrate recent             # crates first mentioned in the last 6 months
explicrate make-seed          # (maintainers) regenerate src/seed.json before a release; use GITHUB_TOKEN
explicrate rank               # build download ranks from the crates.io db dump (2.3GB, run now and then)
GITHUB_TOKEN=$(gh auth token) explicrate rank-stars   # build the stars ranking
```

The cache lives in `$XDG_CACHE_HOME/explicrate` (default `~/.cache/explicrate`); set `EXPLICRATE_CACHE=/some/dir` to move it.
The binary bundles a seed (last 6 months of mentions, every blessed.rs crate with its use case, and their crates.io data),
so lookups work immediately. Two optional one-time downloads add more:
`explicrate sync` scrapes all ~700 newsletter issues for the full mention history (a few minutes), and
`explicrate rank` downloads the 2.3GB crates.io dump to build the downloads ranking.

## How it works

- **Feeds:** all 674 TWiR issues (Crate of the Week) and all scientificcomputing.rs monthlies
  (crate of the month, new releases), about 850 mentions, stored in `feed.json` in the cache.
  The first lookup scrapes automatically if the feed is missing. Re-run `sync` to pick up new issues.
- **Lookup:** always queries crates.io (description, version, downloads total and recent, license,
  dates, keywords, repo and docs links), then GitHub for stars, and adds any feed mentions.
  A crate crates.io doesn't know shows "not found on crates.io".
- **Cache:** everything is fetched once and kept: crates.io and GitHub responses and scraped pages (only the
  archive indexes and the newest SciComp issue expire after a day). Delete files in the cache directory to refetch.
  `prefetch` (also run in the background when `serve` starts) warms the cache for the last-6-months crates.
- **Web:** `GET /` serves the page, `GET /api/crate/<name>` returns the same data as JSON.
- **Test:** `cargo test` covers the two page parsers.

## Deploy

See `DEPLOY.md` in the repository (systemd unit and Caddy config in `deploy/`, `Dockerfile` in the root).

## Ranks and sidebar

- **Downloads rank:** position among crates created in the same year (from the crates.io db dump), shown as top-% and decile.
- **Stars rank:** position among the featured crates (everything in the two feeds) only, not by year.
  Needs `rank-stars` and a `GITHUB_TOKEN`; without one GitHub allows about 60 repos an hour.
- **Sidebar:** crates whose first-ever mention in either feed is within the last ~6 months. Click to look one up.
  `GET /api/recent` returns the same list as JSON.

## Use cases (blessed.rs)

Use cases come from [blessed.rs](https://blessed.rs) (`data/crates.json` in nicoburns/blessed-rs, fetched once into the cache and never bundled in this crate).
A matching crate gets its blessed.rs group, subgroup, purpose, notes and role (recommended / see also), plus a link
back to the matching section. The purpose also appears on mentions and in the sidebar. The purpose notes
(which compare the alternatives) are also what the later "alternatives" feature can build on.

**Before deploying publicly:** the blessed-rs repo declares no licence, so ask its author for permission or a
licence (an issue on the repo) and keep the attribution link. 

## Test results

- `anyhow`: v1.0.105, 6,734 stars, 1 mention (one of three crates of the week, TWiR 308, 2019).
- `thiserror`: v2.0.21, 5,641 stars, 0 mentions in either feed.
- `jiff`: 2 mentions (TWiR 592, SciComp Monthly #24 crate of the month).
- `nonexistent-crate-xyz`: not found on crates.io, 0 mentions.

## Limits

- Matching is by crate name only. TWiR mentions that don't link the crate by name or via crates.io are missed.
- Blurbs are the raw paragraph text. In TWiR 308, `anyhow`'s blurb includes the other two crates.
- GitHub allows 60 unauthenticated calls an hour, after which stars show `?`.
- Alternatives are not implemented yet.
