# MW5 Save Editor - Knowledgebase

Free save game editor for MechWarrior 5: Mercenaries with YAML (Yet Another Mechlab) support and corrupted-save repair. Published on Nexus Mods as a single exe. Repo stays private.

## Project
- Folder: `<code>\mw5-save-editor\`
- Crate: `mw5save` (lib + CLI). GUI (egui) comes after the parser is proven.
- Build: WSL2 cross-compile, `cargo build --release --target x86_64-pc-windows-gnu` (see the SubnetSweep build notes). Never build on Windows host (SAC blocks build scripts).
- No licensing, no telemetry, no network. Nexus audience.
- Config/logs/backups live beside the exe.

## Environment on dev box (2026-10-23)
- Game: GOG, `C:\Program Files (x86)\GOG Galaxy\Games\MW5Mercs`, was 1.14.389, updating to 1.15.398 (DLC9).
- Mods: `MW5Mercs\Mods\` - YAML 0.97.3.2 (to be updated to 0.99.3.1, requires 1.15.398), ModOptions, MechDelivery, CoopUnlocked, PurchaseSalvage, DropFees, advanced_zoom, Coyotesmission, Increasedloadouts, NoArtilleryCamShakeForDLC3.
- Saves: `%LOCALAPPDATA%\MW5Mercs\Saved\SaveGames\<campaign guid>\<save guid>.sav` + `Campaign.json`. Three campaigns, ~190 saves, 1.5 GB. Small test save: `7A6CFA9746C7105E135DA7825B7BF268\C46D1CB84C22D689D4CDF59C6FEE8936.sav` (1.3 MB).
- YAML 0.99 replaces YAML mechs with DLC9 versions; owned loadouts need fixing after update. That transition is the main source of "corrupted" saves to collect as fixtures.

## Save format (from JJ's MW5 Merc Save Editor, MIT, github.com/jonayetjubaer-cmyk/JJs-MW5-Merc-Save-Editor)
- NOT GVAS. File is a bare UE4 tagged-property list from byte 0. No header, no magic.
- Property entry: `[name FString][type FString][size i64][type-specific extras][payload]`, list ends with name `None`.
- FString: i32 len; >0 ASCII incl NUL; <0 UTF-16LE, |len| chars incl NUL; 0 empty.
- Quirks (differ from stock GVAS):
  - StructProperty: extras = struct name FString + 16-byte guid; payload region is `size + 1` bytes, first byte uncounted (0x00).
  - ArrayProperty: extras = element type FString; region `size + 1`, first byte uncounted. Payload = i32 count + elements. Struct arrays: ONE inner tag (name, "StructProperty", i64 total-bytes-of-all-elements, struct name, guid, 1 leading byte) then each element is a property list.
  - Map/SetProperty: extras = key/value type FStrings (set: one); region `size + 1`; kept opaque.
  - BoolProperty: size 0, value byte inline in tag, then 1 trailing byte, no payload.
  - Enum/ByteProperty scalar: extras = enum name FString, then 1 trailing byte, payload = FString enum value.
  - All other scalars: 1 trailing byte after size, then payload.
  - Raw (non-tagged) structs: Vector, Vector2D, Rotator, Quat, Guid, DateTime, Timespan, Color, LinearColor, Transform, Box, Box2D.
- Nesting: top list -> `PersistentModelData` (ArrayProperty<ByteProperty>, payload = i32 len + archive) -> property list of ~29 ObjectProperty models, each payload = `[class path FString][property list][footer]`. Models: SaveStateModel (MechLoadoutWrappers), MechStorageModel, TimelineModel, InventoryModel, MarketModel, StarMapModel, TravelModel, OperationsModel, MechTraitModel, ...
- Mechs: `SaveStateModel.MechLoadoutWrappers[]` each has `ByteData` = i32 len + nested property list containing `MarketItemMech`. `MechLoadoutType` enum distinguishes active bay / MarketLoadout / BuybackMarketLoadout / ColdStorageLoadout.
- Known past corruption cause (wmtorode editor 1.6.4): pilot data length miscalculated on reserialize. Our writer always recomputes sizes from content.

## Catalogs
- Scarab (Rust, MIT, github.com/altNightHawk/Scarab) reads game + mod .pak files (repak + unreal_asset) and emits item/mech/trait catalogs. Candidate for validating asset references against installed mods. Has a YetAnotherLegendaryMech compat shim.

## Decisions
- Lossless round-trip is the gate: open + save with no edits must be byte-identical for every save on the dev box.
- Unknown types stay raw blobs.
- Backup beside the original before any write.
- Repair = detect classes separately (truncation, length mismatch, missing mod asset refs, invalid loadouts) and apply targeted fixes.

- PMD list can contain an `Owner` ObjectProperty whose payload is ONLY an object path (no property list). It is a back-reference, not a model. `Model::parse` sets `is_reference` when nothing follows the class path; writer emits only the path. Present in 98 of 183 saves on the dev box.
- Non-campaign `.sav` files share the folder (`saved_contacts_*.sav`, `blocked_contacts_*.sav`, Instant Action loadout blobs). `save::looks_like_campaign_save` sniffs the first FString; the editor must not try to open these as campaigns.
- Typical save: 28 models + 1 reference, 95 MechLoadoutWrappers.

## Diagnoser (src/diagnose.rs) and fixtures (src/corrupt.rs)
- `mw5save diagnose <file|dir>` -> CLEAN / SUSPECT / FATAL with findings `Severity Kind path : detail`.
- Kinds: NotCampaignSave, Truncated, SizeMismatch, ArchiveLength, OpaqueStruct, OpaqueStructArray, MissingModel, TrailingBytes, Ok.
- Truncated vs SizeMismatch is decided by `probe_pmd`: outer i64 size vs inner i32 archive length around PersistentModelData. Both agree and overrun EOF = Truncated (file cut short, bytes lost). Disagree with inner plausible = SizeMismatch (outer field wrong; repair = outer := inner + 4). Disagree with outer plausible = ArchiveLength (repair = inner := outer - 4).
- Archive::parse errors at pos 4 or "bytes after nested archive" = ArchiveLength (the i32 prefix is the broken field). Applies to PMD and every MechLoadoutWrappers[i].ByteData.
- `GameplayTagContainer` is a raw engine struct (custom serializer). Added to RAW_STRUCTS; a clean save has ~462 of them (ContractModel mission tags, RosterModel pilot Attributes, SaveStateModel ServerTags). Any OTHER non-raw struct that falls back to raw is a Suspect finding.
- Every clean save on the dev box has exactly a 4-byte trailer after the top-level None. MAX_TRAILER = 4; more = TrailingBytes (Suspect).
- `mw5save fixtures <donor> <outdir>` writes 7 synthetic corruptions (truncate-mid, truncate-tail, size-inflate, size-shrink, pmd-length, wrapper-length, garbage-tail) and asserts classification. Donor: `reference\fixtures\donor\small-clean.sav` (copy of the 1.3 MB save). Output: `reference\fixtures\synthetic\`. 7/7 pass.
- Build: `wsl -e bash -lc "bash <project>/build.sh"` (login shell needed for cargo PATH; CRLF stripped). Target dir `a target dir off the synced folder`.

## Mod state / Coyote's Mission Pack
- Mods keep save state in `SaveStateModel.PersistentActors[]` (struct ActorSaveData: `ActorClass` ObjectProperty + `ByteData` nested archive, same shape as mech wrappers). Seen: CoyoteGlobals_C (46 KB), CoopUnlockedActor_C, DropFeesActor_C, PurchaseSalvageActor_C, plus base-game errands/achievements/DLC7 academy.
- Coyote: class `/Coyotesmission/functions/CoyoteGlobals.CoyoteGlobals_C`; archive holds `CoyoteSaveGameConfig` = struct array of `CoyoteSaveGameConfigValue` x114 with Blueprint-named members `id_19_<guid>` (Str), `stringVal_6_<guid>`, `floatVal_10_<guid>`, `boolVal_12_<guid>`, `intVal_18_<guid>`. Match members by prefix. Ids include ASF_* fighter counts, event_infantry_*, fb_got_*, Lance2_AI_*, treasurehunt_*, ai_enabled_*.
- `Campaign` parses all actor archives on open and flushes dirty ones back on save. Write test: changing ASF_light int 2->9 on a 10 MB save produced a file with exactly 1 byte different, CLEAN, round-trips. Fixture: `reference\fixtures\edits\coyote-asf-light-9.sav`.
- Coyote's other references (unit cards, quest ActionClass in CampaignArcModel, turret observation keys, mission quirks) are ordinary ObjectProperty/NameProperty refs; the missing-mod validator will cover them.
- Coyote install on dev box is 1.0 built for game 1.12.62; needs updating alongside YAML after the 1.15 update.

## Status
- 2026-10-23: round-trip gate PASSED. `mw5save roundtrip <SaveGames>` = 183 ok, 0 failed, 3 skipped. CLI exe 373 KB, SHA256 4607A8F165EEAABA2138936307CEE98229E8681399CC6DE419B66A7D546BA6F5. Git initialised, commit a146310.
- 2026-10-23 (later): diagnoser + synthetic fixtures done. `diagnose <SaveGames>` = 183 CLEAN, 4 NotCampaignSave (incl. new UserSaveMetaData.sav written by the 1.15 update). Fixtures 7/7. Exe 403 KB, SHA256 4f9fd32ca9708bf763b6d8f4ccb49b1f94f7da8fcb0029d2ccbbfd22c970f1f9.
- 2026-10-23 (evening): repair.rs done (5/7 synthetic fixtures repaired byte-identical to donor; 2 truncations correctly unrepairable). egui GUI `MW5SaveEditor.exe` (Browser, Overview, Mechs, Pilots, Factions, Coyote's Mission Pack / Mods, Diagnose+Repair). Coyote editor added and write-tested. Public-release files added (LICENSE MIT, README, docs/FORMAT.md, release workflow); history squashed to one commit as royvij on branch main. GitHub MCP token is BAD (Bad credentials); push pending repo creation. Roy has not yet tested the GUI himself.
- 2026-10-23 18:10: PUBLISHED. Public repo https://github.com/DarkfuseSC/mw5-save-editor (MIT). Pushed over SSH: key `%USERPROFILE%\.ssh\id_ed25519_darkfusesc`, ssh config Host `github.com-darkfusesc`, remote `git@github.com-darkfusesc:DarkfuseSC/mw5-save-editor.git`. Account DarkfuseSC (Chrome session), NOT royvij; royvij wincred stays for other repos. Tag v0.1.0 pushed; release workflow (windows-latest, cargo build --locked) run #1 started. GitHub MCP token still bad.
- Public repo hygiene: no company names, no machine paths (knowledgebase scrubbed), no binaries in git, author "Stealthy Sloth". Commit author email is the global git email.
- Next: confirm release #1 produced MW5SaveEditor.exe + SHA256SUMS; Nexus upload (page text can reuse README); YAML 0.99 + Coyote update after GOG finishes; mod-reference remover (strip /Coyotesmission/ or /YetAnother*/ refs) then pak catalog for add-item.
