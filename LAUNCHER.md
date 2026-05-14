# Rift Launcher

Lightweight first-party launcher/updater for private playtests.

The launcher is a small executable you can share once. On startup it:

1. downloads a hosted manifest,
2. compares each listed file by size + SHA-256,
3. downloads only changed/missing files,
4. writes them into a local `game/` folder,
5. launches `game/rift.exe`.

No zip sharing is needed after the tester has the launcher.

The first time a dev-auth playtest client runs, it creates
`game/rift-playtest-user.txt`. That file is the tester's stable account identity,
so progress survives restarts and launcher updates. Testers can delete it to get
a fresh account, or edit it to a short unique name if you need to assign names
manually.

## Layout On A Tester Machine

```text
RiftPlaytest/
  rift-launcher.exe
  game/
    rift.exe
    rift-playtest-user.txt
    assets/
    README.txt
    .rift-launcher/
      installed-version.txt
      backup/
      tmp/
```

If the launcher was built without a baked manifest URL, place this file next to it:

```text
launcher-manifest-url.txt
```

with contents like:

```text
https://cdn.example.com/rift/playtest/manifest.txt
```

## Build A Client Update

The normal path is the one-command publisher from `scripts/rift.ps1`:

Create a local `.env` file first. It is ignored by git:

```dotenv
RIFT_DEFAULT_SERVER=YOUR_SERVER:34000
RIFT_LAUNCHER_BASE_URL=https://pub-275f646594684e8e95e129240c1882ca.r2.dev/playtest
RIFT_LAUNCHER_UPLOAD=rift-r2:rift/playtest
```

Then publish with:

```powershell
.\scripts\rift.ps1 -Release -Launcher
```

`RIFT_LAUNCHER_UPLOAD` can be either a local static-host folder or an `rclone`
destination such as Cloudflare R2, S3, Azure Blob, or a VPS target. The command:

1. builds the release client executable,
2. generates `dist/launcher-feed/manifest.txt` directly from `target/release/rift.exe` and `assets/`,
3. syncs `dist/launcher-feed` to the static host,
4. builds `dist/rift-launcher/rift-launcher.exe` with the manifest URL baked in.

The publisher side uses file-level patching:

- unchanged local feed files are not recopied,
- `rclone sync --checksum` uploads only changed files to the CDN,
- stale CDN files are removed when assets disappear locally.

Launcher publishing does not regenerate `dist/THIRD_PARTY.txt` on every run. If
the file exists, it is reused in the feed; if it does not exist, the feed omits
it. Refresh it manually after dependency/license changes:

```powershell
.\scripts\gen-third-party.ps1
```

The launcher side also uses file-level patching: playtesters only download files
whose size/hash differ from their local install. It is not byte-range binary
delta patching inside individual files, but for this asset layout it avoids the
expensive full reupload/redownload path.

Share this file with playtesters:

```text
dist/rift-launcher/rift-launcher.exe
```

The lower-level manual steps are still available when you want to inspect or
upload pieces yourself.

Build the release client:

```powershell
cargo build --release -p rift-client
```

Build the update feed directly from the release exe and assets:

```powershell
.\scripts\build-launcher-feed.ps1 `
  -BaseUrl "https://cdn.example.com/rift/playtest" `
  -Version "2026.05.14.1"
```

This produces:

```text
dist/launcher-feed/
  manifest.txt
  files/
    rift.exe
    assets/...
```

Upload the contents of `dist/launcher-feed` to the static host backing `BaseUrl`.

Any HTTPS static host works for playtests:

- Cloudflare R2 + public bucket/custom domain
- S3 + CloudFront
- Azure Static Web Apps / Blob static website
- a small VPS running nginx
- GitHub Releases raw asset URLs are possible, but clunky for per-file manifests

## Build The Shareable Launcher

Best tester UX: bake the manifest URL into the launcher so you share one exe.

```powershell
.\scripts\package-launcher.ps1 `
  -ManifestUrl "https://cdn.example.com/rift/playtest/manifest.txt"
```

Share:

```text
dist/rift-launcher/rift-launcher.exe
```

If you do not bake the URL, the script writes a placeholder config file:

```powershell
.\scripts\package-launcher.ps1
```

Then share both:

```text
dist/rift-launcher/rift-launcher.exe
dist/rift-launcher/launcher-manifest-url.txt
```

## Manifest Format

`manifest.txt` is intentionally simple and tab-separated:

```text
rift-launcher-manifest-v1
version	2026.05.14.1
entrypoint	rift.exe
file	rift.exe	12345678	<sha256>	https://cdn.example.com/rift/playtest/files/rift.exe
file	assets/shaders/forward_opaque.frag.spv	1234	<sha256>	https://cdn.example.com/rift/playtest/files/assets/shaders/forward_opaque.frag.spv
```

The launcher rejects absolute paths, `..`, malformed hashes, and size/hash mismatches.

## Launcher Options

```powershell
.\rift-launcher.exe --manifest URL
.\rift-launcher.exe --root game
.\rift-launcher.exe --jobs 8
.\rift-launcher.exe --no-launch
.\rift-launcher.exe -- --connect HOST:PORT
```

`--jobs` controls parallel local checks and downloads. The default is capped at
8 workers, which keeps R2/CDN fetches fast without opening an excessive number
of connections.

Resolution order for the manifest URL:

1. `--manifest URL`
2. `RIFT_LAUNCHER_MANIFEST_URL` environment variable
3. URL baked at compile time by `scripts/package-launcher.ps1 -ManifestUrl ...`
4. `launcher-manifest-url.txt` next to the launcher

## Current Limitations

- This is a lightweight playtest updater, not a signed production updater.
- It verifies SHA-256 for integrity but does not yet verify a signed manifest.
- It keeps backups of replaced files under `game/.rift-launcher/backup/<version>/`, but there is no rollback UI yet.
- It updates the game payload, not the launcher executable itself. For now, share a new launcher manually if the launcher changes.

## Future Hardening

- Sign `manifest.txt` with Ed25519 and bake the public key into the launcher.
- Add a small native UI/progress window instead of console output.
- Add rollback command: `rift-launcher.exe --rollback previous`.
- Add self-update through a helper executable.
