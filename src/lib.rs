//! mw5save: MechWarrior 5 Mercenaries save file library.
//!
//! Format knowledge derived from JJ's MW5 Merc Save Editor (MIT,
//! github.com/jonayetjubaer-cmyk/JJs-MW5-Merc-Save-Editor) and verified
//! against real saves by byte-identical round-trip.

pub mod campaign;
pub mod corrupt;
pub mod diagnose;
pub mod io;
pub mod prop;
pub mod repair;
pub mod save;

pub use prop::{ArrayBody, Property, PropertyList, StructBody, Value};
pub use save::{Archive, Model, SaveFile};
