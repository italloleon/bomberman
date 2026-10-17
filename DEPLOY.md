# Deploying to the DigitalOcean Droplet

Two containers run on the Droplet:

- **bomber-game**: the Rust server (rooms, WebSocket, game loop), only reachable inside Docker.
- **bomber-web**: Caddy. It gets and renews the HTTPS certificate for your subdomain,
  serves the built client, and forwards `/api`, `/ws` and `/healthz` to the game server.

Images are **built on your PC and streamed to the Droplet over SSH**. The Droplet never
compiles anything, which matters because a release build of Rust can run out of memory
on a small Droplet.

## One-time setup

### 1. DNS

At your DNS provider, add an **A record** for the subdomain (e.g. `bomb.example.com`)
pointing at the Droplet's public IPv4. If the Droplet has IPv6, add an **AAAA** record too.
Check with `nslookup bomb.example.com` before continuing — Caddy needs it to resolve to
get a certificate.

### 2. SSH key (on your PC, in Git Bash)

```sh
ssh-keygen -t ed25519            # accept the defaults
cat ~/.ssh/id_ed25519.pub        # copy this line
```

Add the public key to the Droplet: DigitalOcean dashboard → your Droplet → Access →
Reset root password / or Settings → Security → SSH keys (for new Droplets), or if you
can already log in with a password: `ssh-copy-id root@<droplet-ip>`.
Test: `ssh root@<droplet-ip> echo ok`.

### 3. Prepare the Droplet

```sh
ssh root@<droplet-ip>
curl -fsSL https://get.docker.com | sh          # Docker + compose plugin
mkdir -p /opt/bomber
echo "DOMAIN=bomb.example.com" > /opt/bomber/.env
# If ufw is enabled, open the web ports (22 must stay open for SSH):
ufw allow 80/tcp && ufw allow 443/tcp && ufw allow 443/udp
```

If you use a DigitalOcean Cloud Firewall instead, allow inbound TCP 22, 80, 443 and UDP 443.

## Deploy (every time)

From the project root, in Git Bash, with Docker Desktop running:

```sh
DEPLOY_HOST=root@<droplet-ip> ./deploy/deploy.sh
```

It builds both images, streams them to the Droplet, restarts the stack, and finishes
with a health check against `https://<your-domain>/healthz`. The first deploy takes a
few minutes (compiling `wasm-pack`); later ones reuse Docker's cache.

Restarting the game server drops all rooms (they live in memory) — players get the
"connection lost" notice and need a new room. Deploy when nobody is mid-match.

## Useful commands

```sh
ssh root@<droplet-ip> 'cd /opt/bomber && docker compose ps'
ssh root@<droplet-ip> 'cd /opt/bomber && docker compose logs -f game'    # game server logs
ssh root@<droplet-ip> 'cd /opt/bomber && docker compose logs -f caddy'   # HTTPS / certificate logs
```

## Run the production stack locally

```sh
DOMAIN=build docker compose -f deploy/compose.yaml -f deploy/compose.build.yaml build
DOMAIN=:80 HTTP_PORT=8088 HTTPS_PORT=8443 docker compose -p bomber-local -f deploy/compose.yaml up -d
# → http://localhost:8088
DOMAIN=:80 docker compose -p bomber-local -f deploy/compose.yaml down
```
