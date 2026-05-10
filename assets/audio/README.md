# Audio assets

All sound files for the rift client live here. Paths in code
(`SoundSpec::path`) are resolved relative to this directory.

## Conventions

```
assets/audio/
  vfx/        # Visual-effect sibling sounds (whooshes, impacts,
              # casts, charge-ups). One file per VFX preset.
  ambient/    # Long-running ambient loops driven by emitters
              # (torch crackle, portal hum, hub wind, rift
              # rumble).
  ui/         # Menu clicks, panel opens, item pickups, level-up
              # stings. Triggered by the UI layer, not spatial.
  music/      # Streamed background tracks. Use `.ogg`.
  monsters/   # Enemy roars, footsteps, attack telegraphs,
              # death cries. One subfolder per role.
```

## Format

`kira` (via `symphonia`) decodes `.ogg`, `.wav`, `.flac`, and
`.mp3` out of the box. **Use OGG Vorbis for SFX** — small files,
fast decode, no licensing concerns.

## Conventions

- SFX should be authored mono so the spatial mixer can pan
  them freely. Stereo files play back as-is on a spatial
  track but lose left/right cue accuracy.
- Loop files (torch crackle, portal hum) should have
  smoothly-tiling start/end so the seamless loop in
  `SoundSpec::looping = true` doesn't audibly click.
- Aim for −12 dBFS peak per file; the runtime mixes many at
  once and unbalanced sources will fight each other.

## Adding a new sound

1. Drop the file in the appropriate sub-folder.
2. Reference it from gameplay code as
   `SoundSpec::one_shot("vfx/fireball_impact.ogg")` or
   `SoundSpec::looping("ambient/torch_crackle.ogg")`.
3. Tune `min_distance` / `max_distance` for the spatial
   falloff you want — small (`1.0` / `15.0`) for footsteps,
   medium (`2.0` / `35.0`) for ability impacts, large
   (`5.0` / `80.0`) for boss roars and rift events.
