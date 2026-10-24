# MW5 Mercenaries save format notes

Everything here was verified by byte-identical round-trip over 183 real saves
(vanilla and YAML, game 1.14 and 1.15).

## Container

- A `.sav` is a bare UE4 tagged-property list starting at byte 0. There is no
  GVAS header and no magic. Files that do not start with a small positive FString
  length (`saved_contacts_*.sav`, `UserSaveMetaData.sav`, Instant Action blobs)
  are different formats.
- The top-level list ends with the name `None` followed by a 4-byte trailer.
- `PersistentModelData` is an `ArrayProperty<ByteProperty>` whose payload is
  `[i32 length][nested property list][footer]`. That nested list holds ~28
  `ObjectProperty` gameplay models, each `[class path FString][property list][footer]`.
- Some saves also carry an `Owner` entry in that list whose payload is only an
  object path. It is a back-reference, not a model.
- Mechs: `SaveStateModel.MechLoadoutWrappers[]` struct array; each element's
  `ByteData` is another `[i32 length][property list][footer]` archive containing
  `MarketItemMech`. `MechLoadoutType` (absent, `MarketLoadout`,
  `BuybackMarketLoadout`, `ColdStorageLoadout`) tells active-bay mechs from
  market and storage records.

## Property entry

`[name FString][type FString][size i64][type extras][payload]`

FString: `i32 len`; `len > 0` is ASCII including NUL, `len < 0` is UTF-16LE
with `|len|` code units including NUL, `0` is empty.

## Deviations from stock GVAS

These are what break generic UE4 save parsers on MW5:

| Type | Extras after size | Payload region |
|------|-------------------|----------------|
| StructProperty | struct name FString, 16-byte GUID | `size + 1` bytes; first byte uncounted |
| ArrayProperty | element type FString | `size + 1` bytes; first byte uncounted. Payload = `i32 count` + elements. For struct arrays: ONE inner tag `[name]["StructProperty"][i64 total bytes of all elements][struct name][GUID][1 byte]`, then each element is a property list with no per-element header |
| MapProperty / SetProperty | key (and value) type FStrings | `size + 1` bytes, kept opaque |
| BoolProperty | none | size is 0; `[value byte][1 byte]` inline, no payload |
| EnumProperty, ByteProperty (scalar) | enum name FString, 1 byte | payload is an FString with the enum value |
| everything else | 1 byte | `size` bytes |

Raw (non-tagged) struct bodies: `Vector`, `Vector2D`, `Rotator`, `Quat`,
`Guid`, `DateTime`, `Timespan`, `Color`, `LinearColor`, `Transform`, `Box`,
`Box2D`, and `GameplayTagContainer` (an engine struct with a custom serializer;
a clean save has several hundred).

## Useful paths

| Field | Path |
|-------|------|
| C-Bills | `InventoryModel.AvailableCBills.Value` (Int64) |
| Reputation | `MercCompanyModel.Reputation` (Int) |
| Start date | `TimelineModel.StartingDateTicks` (Int64, 100 ns ticks since 0001-01-01) |
| Days elapsed | `TimelineModel.TotalTimeElapsed` (Int) |
| Mech chassis | wrapper archive `MarketItemMech.Item.ItemData.MechDataAssetId.ID.PrimaryAssetName` |
| Mech custom name | `MarketItemMech.Item.ItemData.CustomName` (StrProperty, may be absent) |
| Pilots | `RosterModel.PilotRoster[]`: `PersonaData.Callsign` / `FullName` (TextProperty), `SalaryCBills`, `Skills.<Skill>.TotalExp`, `LockedPlayerSlot` |
| Factions | `MercCompanyModel.FactionStandingList.Standings[]`: `Faction.ID.PrimaryAssetName`, `Standing` |

## Damage classes the tool recognises

| Kind | Symptom | Repair |
|------|---------|--------|
| Truncated | outer and inner PMD lengths agree and overrun EOF | none; data is gone |
| SizeMismatch | outer `PersistentModelData` size disagrees with inner archive length, inner plausible | outer := inner + 4 |
| ArchiveLength | inner `i32` length of PMD or a wrapper `ByteData` is wrong | inner := enclosing size - 4 (only applied if the region then parses) |
| TrailingBytes | more than 4 bytes after the top-level `None` | truncate |
| OpaqueStruct / OpaqueStructArray | a tagged struct that fails to parse (silent fallback) | reported only |
| MissingModel | a required model is absent | reported only |
