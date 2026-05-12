//! Wire / persistence (de)serialisation for [`super::item::Item`].
//!
//! Split out of `item.rs` so the index- and id-keyed packing
//! formats live alongside each other (and away from the roll
//! pipeline + tooltip builder). The struct definition and the
//! gameplay-derived accessors stay in `item.rs`.

use super::affixes::AFFIX_POOL;
use super::item::{Item, LootProvenance, RolledAffix, RolledRiftTouched};
use super::rarity::Rarity;

impl Item {
    /// Pack the rolled item into a wire-friendly tuple of static-pool
    /// indices: `(base_id, rarity_byte, ilvl, [(affix_id, value)],
    /// anchored, unique_id, unique_pick)`. `rift-game` is
    /// dependency-free of the wire crate by design, so the
    /// network layer wraps this tuple in its own struct.
    ///
    /// `unique_id` is the stable string id of the matched
    /// [`super::uniques::UniqueDef`] (or `None` for procedural
    /// legendaries / non-legendaries). `unique_pick` is the
    /// per-instance pool index for pool-roll uniques (today only
    /// Mirrorglass); `None` otherwise. Both default to `None` so
    /// pre-Phase-4 senders / receivers stay backwards-compatible
    /// once the carrier (`ItemBlob`) defaults its mirror fields.
    ///
    /// # Panics
    ///
    /// Panics if `self.base` doesn't live inside [`super::BASE_ITEMS`]
    /// or one of the rolled affix defs doesn't live inside
    /// [`AFFIX_POOL`]. Both invariants are guaranteed for items
    /// produced by [`Item::roll`].
    pub fn to_wire(
        &self,
    ) -> (
        u16,
        u8,
        u16,
        Vec<(u16, f32)>,
        bool,
        Option<&'static str>,
        Option<u8>,
    ) {
        // Match by `id` rather than pointer identity — `BASE_ITEMS`
        // and `AFFIX_POOL` are `pub const` slices, so each access
        // can produce a fresh copy with different addresses.
        let base_id = super::items::BASE_ITEMS
            .iter()
            .position(|b| b.id == self.base.id)
            .expect("base item id not in BASE_ITEMS") as u16;
        let affixes = self
            .affixes
            .iter()
            .map(|a| {
                let id = AFFIX_POOL
                    .iter()
                    .position(|d| d.id == a.def.id)
                    .expect("affix id not in AFFIX_POOL") as u16;
                (id, a.value)
            })
            .collect();
        (
            base_id,
            self.rarity as u8,
            self.ilvl as u16,
            affixes,
            self.anchored,
            self.unique_id,
            self.unique_pick,
        )
    }

    /// Inverse of [`Item::to_wire`]. Returns `None` if any index is
    /// out of bounds (mismatched build / corrupted save).
    ///
    /// `unstable` is **not** part of `to_wire`'s tuple because
    /// the field was added later and we want the existing
    /// (base, rarity, ilvl, affixes, anchored) signature to keep
    /// working unchanged for every call-site. Wire / blob-level
    /// transports thread `unstable` separately (see
    /// `ItemBlob::unstable`); the constructed item starts
    /// stable and the caller flips the flag if the carrier
    /// payload says so.
    pub fn from_wire(
        base_id: u16,
        rarity_byte: u8,
        ilvl: u16,
        affixes: &[(u16, f32)],
        anchored: bool,
        provenance: Option<LootProvenance>,
        unique_id: Option<&'static str>,
        unique_pick: Option<u8>,
    ) -> Option<Self> {
        let base = super::items::BASE_ITEMS.get(base_id as usize)?;
        let rarity = match rarity_byte {
            0 => Rarity::Common,
            1 => Rarity::Magic,
            2 => Rarity::Rare,
            3 => Rarity::Legendary,
            _ => return None,
        };
        let mut rolled = Vec::with_capacity(affixes.len());
        for &(id, value) in affixes {
            let def = AFFIX_POOL.get(id as usize)?;
            rolled.push(RolledAffix { def, value });
        }
        Some(Self {
            base,
            rarity,
            ilvl: ilvl as u32,
            affixes: rolled,
            anchored,
            // `unstable` is not encoded in `to_wire`'s tuple to
            // keep the existing 5-arity contract; the carrier
            // (`ItemBlob`) sets it post-construction. Default
            // here is `false` so blob-less reconstructions
            // (tests, debug paths) come out stable.
            unstable: false,
            provenance,
            unique_id,
            unique_pick,
            // Rift-touched is threaded separately from the
            // `to_wire` tuple — same pattern as `unstable`. The
            // carrier (`ItemBlob.rift_touched`) sets it
            // post-construction; bare reconstructions (tests,
            // debug paths) come out without a rift-touched line.
            rift_touched: None,
        })
    }

    /// Pack the rolled item into a tuple keyed by *stable* string
    /// ids (`BaseItem.id`, `AffixDef.id`) suitable for long-term
    /// storage. Unlike [`Item::to_wire`] this does not depend on
    /// the in-process pool ordering, so saved rows survive a
    /// rebuild that reorders `BASE_ITEMS` / `AFFIX_POOL`.
    ///
    /// # Panics
    ///
    /// Panics if `self.base` or any affix def doesn't carry an id
    /// — both invariants hold for items produced by [`Item::roll`].
    pub fn to_persisted(
        &self,
    ) -> (
        String,
        u8,
        u16,
        Vec<(String, f32)>,
        bool,
        Option<String>,
        Option<u8>,
    ) {
        let affixes = self
            .affixes
            .iter()
            .map(|a| (a.def.id.to_string(), a.value))
            .collect();
        (
            self.base.id.to_string(),
            self.rarity as u8,
            self.ilvl as u16,
            affixes,
            self.anchored,
            self.unique_id.map(|s| s.to_string()),
            self.unique_pick,
        )
    }

    /// Inverse of [`Item::to_persisted`]. Returns `None` if any
    /// id is unknown (item dropped from a pool that has since
    /// been pruned, or a corrupt row).
    pub fn from_persisted(
        base_id: &str,
        rarity_byte: u8,
        ilvl: u16,
        affixes: &[(String, f32)],
        anchored: bool,
        provenance: Option<LootProvenance>,
        unique_id: Option<&str>,
        unique_pick: Option<u8>,
    ) -> Option<Self> {
        let base = super::items::BASE_ITEMS.iter().find(|b| b.id == base_id)?;
        let rarity = match rarity_byte {
            0 => Rarity::Common,
            1 => Rarity::Magic,
            2 => Rarity::Rare,
            3 => Rarity::Legendary,
            _ => return None,
        };
        let mut rolled = Vec::with_capacity(affixes.len());
        for (id, value) in affixes {
            let def = AFFIX_POOL.iter().find(|d| d.id == id.as_str())?;
            rolled.push(RolledAffix { def, value: *value });
        }
        Some(Self {
            base,
            rarity,
            ilvl: ilvl as u32,
            affixes: rolled,
            anchored,
            // Persisted items are by definition stable — the
            // unstable lifecycle ends at extraction, which is
            // the gate that allows persistence in the first
            // place. Any row in the DB therefore reads back as
            // stable, full stop.
            unstable: false,
            provenance,
            // Resolve the persisted unique-id back to the static
            // catalogue's `&'static str` so equality / lookup
            // against the live `UNIQUES` table works. An unknown
            // id (catalogue pruned, row from a future build)
            // degrades to `None` — the item still loads as a
            // procedural legendary; only the authored name /
            // effect is lost.
            unique_id: unique_id.and_then(|s| super::uniques::find(s).map(|u| u.id)),
            unique_pick,
            // Rift-touched threaded separately by the persistence
            // layer (`server::handlers::session`) — same pattern
            // as `unstable`. Plain `from_persisted` calls without
            // a carrying row come out without a rift-touched
            // line.
            rift_touched: None,
        })
    }

    /// Convert the in-memory [`Item::rift_touched`] into the
    /// `(pool_index, value, depth)` triple the wire / blob
    /// transports use. Returns `None` when the item has no
    /// rift-touched line **or** the rolled def isn't in the
    /// live [`super::affixes::RIFT_TOUCHED_POOL`] (defensive —
    /// shouldn't happen for items produced by
    /// [`super::roll::roll_rift_touched`]). The blob field is
    /// `#[serde(default)]` so a `None` deserialises cleanly on
    /// pre-Phase-5 receivers.
    pub fn rift_touched_to_wire(&self) -> Option<(u16, f32, u16)> {
        let rt = self.rift_touched.as_ref()?;
        let idx = super::affixes::RIFT_TOUCHED_POOL
            .iter()
            .position(|d| d.id == rt.def.id)?;
        Some((idx as u16, rt.value, rt.depth))
    }

    /// Inverse of [`Item::rift_touched_to_wire`]. Resolves the
    /// pool index back to a `&'static AffixDef` so the in-memory
    /// shape carries a live pointer. Returns `None` if the
    /// index is out of bounds (mismatched build / corrupt
    /// payload) — the caller can degrade by dropping the line
    /// rather than the whole item.
    pub fn rift_touched_from_wire(triple: Option<(u16, f32, u16)>) -> Option<RolledRiftTouched> {
        let (idx, value, depth) = triple?;
        let def = super::affixes::RIFT_TOUCHED_POOL.get(idx as usize)?;
        Some(RolledRiftTouched { def, value, depth })
    }

    /// Persistence twin of [`Item::rift_touched_to_wire`]:
    /// returns the rolled line keyed by stable string id rather
    /// than pool index. Survives a rebuild that reorders
    /// [`super::affixes::RIFT_TOUCHED_POOL`].
    pub fn rift_touched_to_persisted(&self) -> Option<(String, f32, i16)> {
        let rt = self.rift_touched.as_ref()?;
        Some((rt.def.id.to_string(), rt.value, rt.depth as i16))
    }

    /// Inverse of [`Item::rift_touched_to_persisted`]. Looks up
    /// the stable id in [`super::affixes::RIFT_TOUCHED_POOL`];
    /// returns `None` when the id is unknown (catalogue pruned
    /// between builds) so the caller can drop the line and
    /// keep loading the rest of the item.
    pub fn rift_touched_from_persisted(
        triple: Option<(&str, f32, i16)>,
    ) -> Option<RolledRiftTouched> {
        let (id, value, depth) = triple?;
        let def = super::affixes::RIFT_TOUCHED_POOL
            .iter()
            .find(|d| d.id == id)?;
        Some(RolledRiftTouched {
            def,
            value,
            depth: depth.max(0) as u16,
        })
    }
}
