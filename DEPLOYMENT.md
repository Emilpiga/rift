# Deployment

Operator notes for running the Rift Crawler dedicated server in
production. Game-design and code-architecture docs live in
[`ARCHITECTURE.md`](ARCHITECTURE.md); the launch checklist
lives in [`BEFORE_PUBLISHING.md`](BEFORE_PUBLISHING.md).

## TL;DR

```sh
# Build the server image
docker build -f Dockerfile.server -t rift-server:latest .

# Run it (UDP — netcode/renet is UDP-only)
docker run --rm -p 34000:34000/udp \
    -e DATABASE_URL=postgres://rift:rift@db:5432/rift \
    -e RIFT_PUBLIC=your.public.ip:34000 \
    rift-server:latest
```

For Fly.io specifically: see [`fly.toml`](fly.toml) — its
header comment walks through `flyctl launch` / `secrets set` /
`deploy`.

## Hosting choice

The server uses **UDP** (renet + netcode.io). This rules out
HTTP-only PaaS (Railway, Render, Heroku, Vercel, etc.) and
makes the hosting shortlist:

| Host                           | UDP | Notes                                                                         |
| ------------------------------ | --- | ----------------------------------------------------------------------------- |
| **Fly.io**                     | ✅  | First-class. `fly.toml` shipped. Cheapest path for a single-region launch.    |
| Hetzner / DigitalOcean / Vultr | ✅  | Plain VPS. Open the UDP port in the firewall and run the binary or the image. |
| AWS EC2                        | ✅  | Add a UDP rule to the security group. ALB/NLB needs a UDP target group.       |
| GCP Compute Engine             | ✅  | Allow UDP in the VPC firewall.                                                |
| Railway / Render / Heroku      | ❌  | HTTP only. Won't work.                                                        |

## Required runtime configuration

The server reads configuration from **environment variables**
or matching CLI flags (flag wins on conflict).

| Env var        | Flag             | Default                | Notes                                                                                                               |
| -------------- | ---------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `RIFT_BIND`    | `--bind`         | `0.0.0.0:34000`        | Socket the server opens. Use `fly-global-services:34000` on Fly.                                                    |
| `PORT`         | _(see `--bind`)_ | _(none)_               | Fallback for hosts that only set `PORT`. Bound on `0.0.0.0`.                                                        |
| `RIFT_PUBLIC`  | `--public`       | _(falls back to bind)_ | Address baked into connect tokens. Must be the player-facing IP:port. Required behind NAT, a load balancer, or Fly. |
| `DATABASE_URL` | `--database-url` | _(none → in-memory)_   | `postgres://user:pass@host:port/db`. Omit (or pass `--no-db`) for offline testing only.                             |
| `RUST_LOG`     | n/a              | `info`                 | Standard `env_logger` filter. Bump to `debug` to chase issues.                                                      |

**Required for any real deployment:** `RIFT_PUBLIC` and
`DATABASE_URL`. `RIFT_BIND` defaults are fine for a single-
machine VPS; Fly needs the `fly-global-services` override
(see `fly.toml`).

## Database

Postgres + sqlx. Migrations live in
`crates/rift-persistence/migrations` and are baked into the
runtime image. They run automatically on startup against
`DATABASE_URL` — no manual `sqlx migrate run` needed.

Recommended: provision a managed Postgres (Fly Postgres, Neon,
Supabase, RDS, etc.) so backups, point-in-time-recovery, and
replica failover are someone else's problem.

### Backups

**Automated nightly snapshots are a launch blocker** (see
[`BEFORE_PUBLISHING.md`](BEFORE_PUBLISHING.md) section 0). Until
the managed backup story is wired in, run a manual
`pg_dump --format=custom` before every deploy and stash it
somewhere off the host.

Restore drill:

```sh
pg_restore --clean --if-exists --no-owner \
    -d "$DATABASE_URL" path/to/backup.dump
```

Test the restore drill against a copy of production at least
once before you need it for real.

## Deploying a new version

1. **Tag the build.** `git tag -a v0.1.X -m "..."`,
   `git push --tags`. The tag becomes the image tag and the
   rollback target.
2. **Snapshot the database.** Manual `pg_dump` until the
   automated nightly is in place.
3. **Build & push the image.**
   ```sh
   docker build -f Dockerfile.server -t rift-server:v0.1.X .
   # On Fly: `flyctl deploy` builds via the remote builder
   # and applies the new release atomically.
   flyctl deploy --image rift-server:v0.1.X
   ```
4. **Verify.** Tail logs (`flyctl logs` or `docker logs -f`)
   and confirm: `rift-server ready on …`, then a clean Hello
   from a test client.
5. **Roll back.** `flyctl releases` lists every deploy;
   `flyctl deploy --image rift-server:v0.1.X-1` restores the
   previous tag.

## Graceful shutdown

The server installs a SIGINT / SIGTERM handler (Ctrl-C, `docker
stop`, `flyctl deploy` rolling restart) that:

1. Lets the current sim tick finish.
2. Runs one final `auto_save_all`.
3. Calls `PersistenceHandle::shutdown_blocking` so the worker
   drains its mailbox.
4. Exits cleanly.

A second signal during the drain bypasses the handler and
hard-kills the process. For real production: prefer
`flyctl deploy` (rolling restart, one machine at a time) or
`docker stop --time 30` to give the drain enough headroom.

## Networking (current state)

`rift-net` currently uses `ServerAuthentication::Unsecure` /
`ClientAuthentication::Unsecure` — clients are trusted to
declare their own `client_id`. **Do not consider this secure.**
Any public deployment is effectively open to impersonation.

The plan (tracked in
[`BEFORE_PUBLISHING.md`](BEFORE_PUBLISHING.md) section 0) is
to graduate to netcode.io's `Secure` mode, which requires:

- An **auth service** that issues signed connect tokens.
- A **signing key** shared between the auth service and the
  game server, loaded from `RIFT_CONNECT_TOKEN_KEY` (32-byte
  hex string in an env var, never checked into source).
- A **rotation procedure**: roll the key in two phases (deploy
  the new key alongside the old, switch the auth service to
  sign with the new key, retire the old key after the longest
  outstanding session has expired).

Until that lands, restrict the public IP to a private playtest
group via firewall rules or a VPN.

## Health checks

Fly's HTTP probe doesn't work against a UDP-only service — the
server has no TCP listener. Today's "is it alive" check is
the process exit code (Fly auto-restarts on crash) plus log
inspection. A small TCP `/healthz` sidecar that reports "sim
is ticking" is on the launch checklist.

## Troubleshooting

| Symptom                                                | Likely cause                                                                            |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| Client `Connection rejected by server: protocol …`     | Client and server disagree on `PROTOCOL_VERSION`. Roll one to match the other.          |
| Client hangs at "Connecting…" indefinitely             | UDP firewall block. Verify `nc -u -z host 34000` from the client side.                  |
| Server logs `transport update: …`                      | Usually a malformed packet, not a bug. Sustained errors → suspect a hostile client.     |
| `persistence: load_inventory failed for …`             | Postgres unreachable or schema drifted. Check `DATABASE_URL` and migration log on boot. |
| Rolling deploy drops a player's last few inventory ops | Auto-save is 30 s. Use `flyctl deploy` (rolling) not `flyctl machine restart` (hard).   |

## Contact

Operator escalation: emil@piga.nu
