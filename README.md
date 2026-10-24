# MW5 Save Editor

A free, open-source save game editor and **save repair tool** for
MechWarrior 5: Mercenaries. Single native Windows executable. No installer,
no runtime, no network access, no telemetry.

Works with vanilla saves and with saves from modded games including
YAML (Yet Another Mechlab). Tested against DLC9 / game version 1.15.

## Download

Grab `MW5SaveEditor.exe` from the
[latest release](https://github.com/DarkfuseSC/mw5-save-editor/releases/latest).
Put it anywhere and run it. Each release lists SHA256 hashes.

## What it does

- **Browse** every save in your SaveGames folder, grouped by campaign, newest first.
- **Edit** C-Bills, reputation, days elapsed, mech custom names, pilot callsigns,
  salaries and skill experience, faction standings.
- **Coyote's Mission Pack**: view and edit every entry of the pack's own save
  state (its `CoyoteSaveGameConfig` table: aerospace fighter counts, event
  flags, lance AI settings and so on). The same tab lists every mod that keeps
  state in the save.
- **Diagnose** a save that will not load: the tool names the damage class
  (truncated file, bad size field, bad nested archive length, trailing garbage,
  missing model) with the exact byte offset.
- **Repair** the recoverable classes automatically. Repairs are verified by
  re-parsing the result; on the synthetic test set every repaired file is
  byte-identical to the original undamaged save.
- **Safe writes**: a timestamped `.bak` is written beside the original before
  any save, the new file is written to a temp name and renamed into place, and
  the written bytes are re-diagnosed.

### What it cannot do

A truncated save (the game crashed mid-write) has lost data that nothing can
reconstruct. The tool tells you so and points you at the previous autosave in
the same campaign folder.

## Where saves live

`%LOCALAPPDATA%\MW5Mercs\Saved\SaveGames\<campaign id>\<save id>.sav`

Steam, GOG and Epic all use this folder. Xbox / Game Pass saves are wrapped in
a container format and are not supported yet.

## Command line

`mw5save.exe` ships alongside the GUI for scripting and bug reports:

```
mw5save roundtrip  <file|dir>     parse, reserialize, verify byte-identical
mw5save diagnose   <file|dir>     classify damage
mw5save repair     <in> <out>     repair one file
mw5save repair-all <dir> <outdir> repair every .sav in a folder
mw5save models     <file>         list gameplay models in the save
mw5save dump       <file> [model] print the property tree
mw5save find       <file> <text>  search every path and value, all levels
mw5save actors     <file> [class] list persistent actors (mod state) / dump one
mw5save coyote     <file>         list Coyote's Mission Pack entries
mw5save fixtures   <donor> <dir>  generate synthetic corrupted saves (dev)
```

When reporting a save that fails to open, attach the output of
`mw5save diagnose yoursave.sav`.

## Building

Rust 1.80 or newer. `cargo build --release` produces `MW5SaveEditor.exe` and
`mw5save.exe`. No other dependencies. `build.sh` shows the cross-compile
invocation used for releases (Linux host, `x86_64-pc-windows-gnu`).

## How it works

MW5 saves are a bare Unreal Engine 4 tagged-property list (not a GVAS file)
with three nested archives: the top-level list, `PersistentModelData`, and
one archive per mech record. The parser is lossless by construction: anything
it does not understand is kept as raw bytes and written back verbatim, and
every size field is recomputed from content on write. See
`docs/FORMAT.md` for the format notes, including the MW5-specific deviations
from stock GVAS that break generic parsers.

## Credits

- Format knowledge was bootstrapped from
  [JJ's MW5 Merc Save Editor](https://github.com/jonayetjubaer-cmyk/JJs-MW5-Merc-Save-Editor)
  (MIT), whose Python parser documented the tagged-property quirks, and
  confirmed against real saves by byte-identical round-trip.
- [Scarab](https://github.com/altNightHawk/Scarab) (MIT) for the approach to
  reading game and mod asset catalogs.
- cszolee79 and the YAML team for the mod that makes this worth doing.

## License

MIT. See `LICENSE`.

Save editing is not supported by the game's developers. Back up your saves.
