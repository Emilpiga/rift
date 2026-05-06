# Deploying Rift

This repo ships **two binaries** out of one Cargo workspace:

| Crate         | Binary                         | Pulls graphics deps? | Where it runs               |
| ------------- | ------------------------------ | -------------------- | --------------------------- |
| `rift-server` | `rift-server`                  | **No** (headless)    | Dedicated server / cloud VM |
| `rift-client` | `rift` (`rift.exe` on Windows) | Yes (Vulkan)         | Player's machine            |

The split is enforced at the `Cargo.toml` level: `rift-server`
intentionally does **not** depend on `rift-engine`, so the server
image stays tiny and free of Vulkan / winit / shaderc.

---

## Server

### Networking

The server uses **netcode.io / renet over UDP**. This rules out any
PaaS that only exposes inbound HTTP/TCP (Railway, Render, Heroku,
Cloudflare Workers, Vercel, …).

Hosts that _do_ support raw inbound UDP:

- **Fly.io** — recommended. UDP is a first-class service type, see
  `fly.toml`.
- **Hetzner Cloud / DigitalOcean / Vultr / Linode VPS** — open the
  UDP port in the firewall, run the docker image (or the bare
  binary) under systemd.
- **AWS EC2 / GCP Compute / Azure VM** — open the UDP port in the
  security group.

If a host claims to support "TCP/UDP" but only at the load-balancer
layer (e.g. AWS NLB), it'll work — point `RIFT_PUBLIC` at the
load-balancer's public IP:port.

### Configuration

`rift-server` reads both CLI flags and environment variables. Env
vars are used as defaults so the same image works locally with
docker-compose and in the cloud:

| Env            | Flag             | Purpose                                                            |
| -------------- | ---------------- | ------------------------------------------------------------------ |
| `PORT`         | (n/a)            | Cloud-provider idiom; bound on `0.0.0.0:$PORT`.                    |
| `RIFT_BIND`    | `--bind`         | Full bind socket address. Overrides `PORT`.                        |
| `RIFT_PUBLIC`  | `--public`       | Public address baked into connect tokens. **Required behind NAT.** |
| `DATABASE_URL` | `--database-url` | Postgres URL. Empty / `disabled` skips persistence.                |
| (none)         | `--no-db`        | Force-disables persistence regardless of env.                      |
| `RUST_LOG`     | (n/a)            | Standard `env_logger` filter. Defaults to `info`.                  |

### Build & run with Docker

```bash
docker build -f Dockerfile.server -t rift-server:latest .
docker run --rm \
    -p 34000:34000/udp \
    -e DATABASE_URL=postgres://rift:rift@host.docker.internal:55432/rift \
    -e RIFT_PUBLIC=YOUR.PUBLIC.IP:34000 \
    rift-server:latest
```

The `/udp` suffix on `-p` is mandatory — without it Docker only
forwards TCP and clients silently fail to connect.

### Deploy to Fly.io

```bash
flyctl auth login
flyctl launch --copy-config --no-deploy        # creates the app
flyctl postgres create --name rift-db          # provisions Postgres
flyctl postgres attach rift-db                 # injects DATABASE_URL
flyctl ips allocate-v4                         # gets a dedicated IP
flyctl secrets set RIFT_PUBLIC=<that-ip>:34000
flyctl deploy
```

After deploy, point your client at `<that-ip>:34000`:

```bash
rift --connect <that-ip>:34000
```

---

## Client

### Build a release binary

```bash
cargo build --release -p rift-client
```

The binary lands at `target/release/rift` (or `rift.exe`).

### Pack a redistributable bundle

The client needs the binary **plus** `assets/` (icons, models,
shaders, textures) at runtime. The packaging script collects
everything into a single zip / tarball:

```bash
# Linux / macOS
./scripts/package-client.sh

# Windows (PowerShell)
.\scripts\package-client.ps1
```

Each script produces `dist/rift-client-<os>-<arch>.zip` containing:

```
rift-client/
├── rift(.exe)
├── assets/
└── README.txt          ← brief "double-click rift, --connect host:port"
```

Send that one zip to a playtester. They unzip, run the binary,
done. No Cargo, no Rust toolchain, no Vulkan SDK on their side
(they only need a modern GPU driver with Vulkan support — every
mainstream 2018+ GPU has this out of the box).

### Connecting to a remote server

```bash
rift --connect game.example.com:34000
```

The client exposes the same `--connect` flag on every platform. If
the server is using a non-default port, include it.

---

## Local-only dev workflow (unchanged)

```bash
docker compose up -d                # start Postgres
cargo run -p rift-server            # listens on 0.0.0.0:34000
cargo run -p rift-client            # connects to 127.0.0.1:34000
```
