# Deploying to the Hetzner server

Plan: run the binary under systemd on 127.0.0.1:8080 and put Caddy in front for HTTPS.
Docker is the alternative (last section). Neither path has been run yet, so expect small fixes.

## Before going public

- [ ] Get permission or a licence from the blessed-rs author (the repo declares none) and keep the blessed.rs attribution links.
- [ ] Pick a domain and point its A/AAAA record at the server.
- [ ] Decide if the site is public. There's no auth and no rate limiting, and every new crate name triggers
      a crates.io and GitHub fetch. Fine for personal use; put basic auth in Caddy if unsure.

## 1. Build on the server (not on the Mac)

The Mac is arm64 and the server may be x86, so build there:

```
ssh server
curl https://sh.rustup.rs -sSf | sh
git clone <your repo> explicrate && cd explicrate   # or rsync the source over
cargo build --release
sudo install target/release/explicrate /usr/local/bin/   # or: cargo install explicrate, once published
```

## 2. User and data directory

```
sudo useradd --system --home /var/lib/explicrate --create-home --shell /usr/sbin/nologin explicrate
```

## 3. Copy the cache from your Mac (so the server never downloads the 2.3 GB dump)

Run on the Mac, in the project folder, after `explicrate rank` and `explicrate rank-stars`:

```
rsync -av cache/ server:/tmp/explicrate-cache/
ssh server 'sudo mkdir -p /var/lib/explicrate && sudo rsync -a /tmp/explicrate-cache/ /var/lib/explicrate/cache/ && sudo chown -R explicrate: /var/lib/explicrate'
```

The cache holds scraped pages, `feed.json`, `ranks.json`, crates.io/GitHub responses and the blessed.rs data.
Without it the first lookup rescrapes everything (about 20 seconds with a warm network, a few minutes cold).

## 4. systemd

```
sudo cp deploy/explicrate.service /etc/systemd/system/
echo 'GITHUB_TOKEN=ghp_...' | sudo tee /etc/explicrate.env && sudo chmod 600 /etc/explicrate.env   # optional
sudo systemctl daemon-reload
sudo systemctl enable --now explicrate
journalctl -u explicrate -f
curl localhost:8080/api/recent | head -c 200
```

On start the service warms the cache for the last-6-months crates in the background.

## 5. HTTPS with Caddy

```
sudo apt install caddy
sudo cp deploy/Caddyfile /etc/caddy/Caddyfile   # edit the domain first
sudo systemctl reload caddy
```

## Keeping it fresh

Run these on the server (as the explicrate user, with `EXPLICRATE_CACHE=/var/lib/explicrate/cache`):

- new issues: `explicrate sync` (weekly; the sidebar updates by itself afterwards)
- download ranks: `explicrate rank` every few months (2.3 GB download, or redo it on the Mac and rsync `ranks.json`)
- stars ranking: `explicrate rank-stars` with a `GITHUB_TOKEN`

A cron or systemd timer for `sync` is the next thing to add.

## Docker instead

```
docker build -t explicrate .
docker run -d --name explicrate --restart unless-stopped -p 127.0.0.1:8080:8080 -v explicrate-data:/data explicrate
# seed the volume with your cache
docker run --rm -v explicrate-data:/data -v "$PWD/cache":/src alpine cp -r /src/. /data/
```

Then use the same Caddy step.

## Known limits

- One request at a time (single-threaded server loop): a slow first lookup blocks others.
- Without a `GITHUB_TOKEN`, stars are missing after about 60 new repos per hour.
