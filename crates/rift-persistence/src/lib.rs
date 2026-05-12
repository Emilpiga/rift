//! Async persistence layer for the rift server.
//!
//! Owns a dedicated tokio multi-thread runtime on a worker OS thread
//! so the synchronous server tick loop never has to `await`. The
//! server interacts with this crate exclusively through
//! [`PersistenceHandle`], which forwards [`PersistenceMsg`]s to the
//! worker via an unbounded mpsc channel.
//!
//! Two flavours of message:
//!
//! * **Request/response** (e.g. [`PersistenceMsg::LoadOrCreate`])
//!   carry a `oneshot` reply channel. The server *can* block on the
//!   reply when it absolutely needs the result before continuing —
//!   typically once per session at `Hello` time.
//! * **Fire-and-forget** (e.g. [`PersistenceMsg::Save`]) return
//!   immediately on the sender side. The worker drains writes as
//!   fast as the database allows; if it falls behind, messages
//!   queue up but the gameplay loop never stalls.

use std::thread;

use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolOptions};
use tokio::sync::{mpsc, oneshot};
pub use uuid::Uuid;

/// One persisted character row, decoded from the `characters` table.
#[derive(Clone, Debug)]
pub struct CharacterRecord {
    pub id: Uuid,
    pub account_id: Uuid,
    pub name: String,
    pub class_id: String,
    /// Gender id (matches `rift_net::messages::Gender as u8`). Stored
    /// as `SMALLINT` to keep the schema tiny.
    pub gender: i16,
    pub level: i32,
    pub xp: i32,
    /// Six ability wire ids (see `rift_game::abilities::id`). Stored
    /// as `SMALLINT[]` so the column shape is independent of the
    /// (currently 6) bar slot count.
    pub loadout: [i16; 6],
    /// Highest rift floor this character has ever cleared
    /// (boss killed). `0` for fresh characters who haven't
    /// finished a floor yet. Drives the start-floor picker in
    /// the portal modal: the player can begin a run at any
    /// floor in `1..=deepest_cleared_floor`. When in a party,
    /// the leader is capped to `min` of every party member's
    /// value so nobody is dragged past their cleared content.
    pub deepest_cleared_floor: i32,
    /// Per-character salvage currency. Minted by salvaging
    /// items in the bag (yield scales with rarity + ilvl) and
    /// spent on stash expansion / future crafting. Stored as
    /// `INTEGER NOT NULL DEFAULT 0` (see
    /// `20260508000002_character_shards.sql`); legitimate
    /// totals stay well under `i32::MAX`.
    pub shards: i32,
}

/// One persisted inventory row. Keys items by *stable* string ids
/// (`BaseItem.id`, `AffixDef.id`) so the row survives a rebuild
/// that reorders the static `BASE_ITEMS` / `AFFIX_POOL` pools.
/// `rift-server` round-trips this to / from `rift_game::loot::Item`
/// via `Item::to_persisted` / `Item::from_persisted`.
#[derive(Clone, Debug)]
pub struct PersistedItem {
    pub base_id: String,
    /// Rarity discriminant byte (Common=0, Magic=1, Rare=2, Legendary=3).
    pub rarity: i16,
    pub ilvl: i32,
    /// `(affix string id, rolled value)` pairs.
    pub affixes: Vec<(String, f32)>,
    /// `Some(slot_byte)` when this row is currently equipped
    /// (matches `rift_game::loot::EquipSlot::to_u8`); `None` for
    /// rows sitting in the bag. Fresh pickups default to `None`.
    pub equipped_slot: Option<i16>,
    /// 0-based position the player last saw this row at in the
    /// bag (or stash). Equipped rows write a value too but it's
    /// ignored on load since `equipped_slot` routes the row.
    /// Append paths can leave this at 0 — the SQL writer will
    /// pick `MAX(slot_index)+1` instead.
    pub slot_index: i32,
    /// `true` if this item rolled the rare "Anchored" trait
    /// at drop time. Anchored items survive the wipe-on-death
    /// loot reset on the server. Legendary-only — the column
    /// is `false` for every other rarity.
    pub anchored: bool,
    /// Stash-only: which tab this row belongs to. Ignored
    /// (always `0`) for inventory rows. Tabs are dense
    /// `[0..n)`; the count is implied by the highest index in
    /// `stash_tabs` for the character.
    pub tab_index: i16,
    /// Pickup-eligibility lineage. `Some` carries the character
    /// UUIDs that shared the originating expedition; `None`
    /// is the legacy state for rows that pre-date the
    /// provenance system. The runtime self-binds `None` to
    /// the holding character on first interaction.
    ///
    /// Stored in the `provenance` column as `UUID[] NULL`.
    pub provenance: Option<Vec<Uuid>>,
    /// Stable string id of the matched
    /// `rift_game::loot::uniques::UniqueDef`. `None` for
    /// procedural legendaries and non-legendaries. Stored as
    /// `unique_id TEXT NULL`.
    pub unique_id: Option<String>,
    /// Per-instance pool index for pool-roll uniques (today
    /// Mirrorglass). `None` for `Fixed` uniques and non-uniques.
    /// Stored as `unique_pick SMALLINT NULL` (-1 sentinel on
    /// load is converted to `None` defensively).
    pub unique_pick: Option<i16>,
    /// Rift-touched bonus line earned past
    /// [`rift_game::loot::RIFT_TOUCHED_MIN_FLOOR`]. Stored as
    /// three nullable columns: `rift_touched_id TEXT NULL`,
    /// `rift_touched_value REAL NULL`,
    /// `rift_touched_depth SMALLINT NULL`. `Some` only when
    /// all three are populated; we never store a partial row.
    /// Survives extraction (it's a permanent identity line) so
    /// the wipe path leaves the column untouched. Defaults to
    /// `None` so legacy rows + the migration's default fill
    /// hydrate cleanly.
    pub rift_touched: Option<(String, f32, i16)>,
}

/// One persisted stash-tab metadata row. Items live in their
/// own table (`stash_items`) and are joined by
/// `(character_id, tab_index)`.
#[derive(Clone, Debug)]
pub struct PersistedStashTab {
    pub tab_index: i16,
    pub name: String,
    /// Packed `0xRRGGBB`.
    pub color: i32,
}

/// Internal JSONB representation of a single affix entry. Kept
/// stable so manual SQL inspection / future migrations stay
/// readable.
#[derive(Serialize, Deserialize)]
struct AffixJson {
    id: String,
    v: f32,
}

/// Defensive packer for the three rift-touched columns. Returns
/// `Some(_)` only when **all three** are populated; a partial row
/// (any subset of the three is `NULL`) decodes as `None`,
/// matching the migration's intent that the trio is written
/// atomically.
fn rift_touched_from_columns(
    id: Option<String>,
    value: Option<f32>,
    depth: Option<i16>,
) -> Option<(String, f32, i16)> {
    match (id, value, depth) {
        (Some(i), Some(v), Some(d)) => Some((i, v, d)),
        _ => None,
    }
}

/// Worker mailbox. Constructed by [`spawn`]; cloneable so multiple
/// server subsystems can write through one shared handle.
#[derive(Clone)]
pub struct PersistenceHandle {
    tx: mpsc::UnboundedSender<PersistenceMsg>,
}

/// Messages the server sends to the persistence worker.
#[allow(clippy::large_enum_variant)] // request/response variants carry one-shots
pub enum PersistenceMsg {
    /// Look up the character by `character_name`; if it doesn't
    /// exist, create it under the account identified by
    /// `account_key` (creating the account too if missing). The
    /// reply contains the freshly-loaded or freshly-created
    /// [`CharacterRecord`].
    ///
    /// `account_key` is the issuer-tagged storage form
    /// (`"dev:<name>"` / `"steam:<id>"`); `display_name` is
    /// the human-readable label persisted alongside it for the
    /// roster reply / future UI.
    ///
    /// Server typically `blocking_recv()`s on the reply during
    /// the `Hello` handshake — we can't accept the player into
    /// the world without their level / xp.
    LoadOrCreate {
        account_key: String,
        display_name: String,
        character_name: String,
        class_id: String,
        gender: i16,
        reply: oneshot::Sender<Result<CharacterRecord, PersistenceError>>,
    },

    /// Persist the latest snapshot of a character. Fire-and-forget
    /// from the server's perspective. Worker UPSERTs by `id`.
    /// Used for periodic auto-saves *and* the final-save on
    /// disconnect.
    Save { record: CharacterRecord },

    /// List every character row that belongs to `account_key`.
    /// Creates the account row if it doesn't exist yet so the
    /// caller can immediately offer "Create New Character". The
    /// reply contains `(account_id, characters)`; an empty
    /// `characters` vec means a brand-new account.
    ///
    /// `account_key` is the issuer-tagged storage form;
    /// `display_name` is only used on first-insert (it's not
    /// touched if the row already exists, so a Steam persona
    /// rename will be picked up by `LoadOrCreate` next session).
    ListAccountCharacters {
        account_key: String,
        display_name: String,
        reply: oneshot::Sender<Result<(Uuid, Vec<CharacterRecord>), PersistenceError>>,
    },

    /// Drain the queue and stop the worker. The reply fires once
    /// every preceding message has been processed (so the server
    /// can rely on this for a clean shutdown that loses no writes).
    Shutdown { reply: oneshot::Sender<()> },

    /// Load every inventory row belonging to `character_id`. The
    /// reply contains the items in `acquired_at` order so the
    /// server can rebuild the bag in deterministic order. Used
    /// once per session at the `Hello` handshake.
    LoadInventory {
        character_id: Uuid,
        reply: oneshot::Sender<Result<Vec<PersistedItem>, PersistenceError>>,
    },

    /// Persist a freshly-picked item under `character_id`.
    /// Fire-and-forget — `try_pickup_loot` already mutated the
    /// authoritative in-memory state.
    AppendInventoryItem {
        character_id: Uuid,
        item: PersistedItem,
    },

    /// Replace every `inventory_items` row owned by
    /// `character_id` with `items` in a single transaction.
    /// Used by the equipment subsystem on every equip / unequip
    /// (and any future bag rewrite) so the persisted snapshot is
    /// always consistent with what the in-memory bag +
    /// equipment hold. Inventories are small enough that
    /// DELETE + INSERT every change is cheaper than threading a
    /// per-item UUID through the in-memory `Item` type.
    /// Fire-and-forget — a dropped write is recoverable on the
    /// next gameplay action that re-syncs.
    ResetCharacterInventory {
        character_id: Uuid,
        items: Vec<PersistedItem>,
    },

    /// Load every `stash_items` row belonging to `character_id`
    /// in `acquired_at` order alongside the per-tab metadata
    /// rows from `stash_tabs`. Tab list is dense `[0..n)`; if
    /// the table is empty the server seeds a default tab 0.
    /// Used at Hello time.
    LoadStash {
        character_id: Uuid,
        reply:
            oneshot::Sender<Result<(Vec<PersistedStashTab>, Vec<PersistedItem>), PersistenceError>>,
    },

    /// Replace every `stash_items` + `stash_tabs` row owned by
    /// `character_id` with the given snapshot in a single
    /// transaction. Mirrors
    /// [`Self::ResetCharacterInventory`] in shape and is used
    /// by every deposit / withdraw / tab-purchase event.
    /// Fire-and-forget.
    ResetCharacterStash {
        character_id: Uuid,
        tabs: Vec<PersistedStashTab>,
        items: Vec<PersistedItem>,
    },
}

/// Errors returned through [`PersistenceMsg::LoadOrCreate`].
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("worker has shut down")]
    WorkerGone,
}

impl PersistenceHandle {
    /// Send a `LoadOrCreate` request and synchronously block until
    /// the worker replies. Intended for the `Hello` handshake.
    pub fn load_or_create_blocking(
        &self,
        account_key: String,
        display_name: String,
        character_name: String,
        class_id: String,
        gender: i16,
    ) -> Result<CharacterRecord, PersistenceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(PersistenceMsg::LoadOrCreate {
                account_key,
                display_name,
                character_name,
                class_id,
                gender,
                reply,
            })
            .map_err(|_| PersistenceError::WorkerGone)?;
        rx.blocking_recv()
            .map_err(|_| PersistenceError::WorkerGone)?
    }

    /// Queue a fire-and-forget save. Returns `false` if the worker
    /// has shut down (server should already be tearing down in
    /// that case, so we just log on the caller side).
    pub fn save(&self, record: CharacterRecord) -> bool {
        self.tx.send(PersistenceMsg::Save { record }).is_ok()
    }

    /// List the characters belonging to `account_key`, creating
    /// the account row if missing. Blocks the calling thread until
    /// the worker replies — server uses this during the
    /// `Hello` handshake before the player has picked a
    /// character to play.
    pub fn list_account_characters_blocking(
        &self,
        account_key: String,
        display_name: String,
    ) -> Result<(Uuid, Vec<CharacterRecord>), PersistenceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(PersistenceMsg::ListAccountCharacters {
                account_key,
                display_name,
                reply,
            })
            .map_err(|_| PersistenceError::WorkerGone)?;
        rx.blocking_recv()
            .map_err(|_| PersistenceError::WorkerGone)?
    }

    /// Block until the worker has processed every queued message
    /// and then exited. Idempotent-ish: a second call after the
    /// worker is gone simply returns `Ok(())`.
    pub fn shutdown_blocking(&self) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(PersistenceMsg::Shutdown { reply }).is_ok() {
            let _ = rx.blocking_recv();
        }
    }

    /// Load the persisted inventory for `character_id`. Blocks the
    /// calling thread until the worker replies — server uses this
    /// once per session right after `load_or_create_blocking` so
    /// the player's bag is hot before they walk into the world.
    pub fn load_inventory_blocking(
        &self,
        character_id: Uuid,
    ) -> Result<Vec<PersistedItem>, PersistenceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(PersistenceMsg::LoadInventory {
                character_id,
                reply,
            })
            .map_err(|_| PersistenceError::WorkerGone)?;
        rx.blocking_recv()
            .map_err(|_| PersistenceError::WorkerGone)?
    }

    /// Queue a fire-and-forget inventory append. Returns `false`
    /// if the worker has shut down.
    pub fn append_inventory_item(&self, character_id: Uuid, item: PersistedItem) -> bool {
        self.tx
            .send(PersistenceMsg::AppendInventoryItem { character_id, item })
            .is_ok()
    }

    /// Queue a fire-and-forget bag-rewrite. Replaces every
    /// inventory row owned by `character_id` with `items` in a
    /// single transaction. Used after equip / unequip; see
    /// [`PersistenceMsg::ResetCharacterInventory`].
    pub fn reset_character_inventory(&self, character_id: Uuid, items: Vec<PersistedItem>) -> bool {
        self.tx
            .send(PersistenceMsg::ResetCharacterInventory {
                character_id,
                items,
            })
            .is_ok()
    }

    /// Load the persisted stash for `character_id`. Blocks the
    /// calling thread until the worker replies — server uses
    /// this once per session right after `load_inventory_blocking`
    /// so the player's stash is hot before they enter the world.
    pub fn load_stash_blocking(
        &self,
        character_id: Uuid,
    ) -> Result<(Vec<PersistedStashTab>, Vec<PersistedItem>), PersistenceError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(PersistenceMsg::LoadStash {
                character_id,
                reply,
            })
            .map_err(|_| PersistenceError::WorkerGone)?;
        rx.blocking_recv()
            .map_err(|_| PersistenceError::WorkerGone)?
    }

    /// Queue a fire-and-forget stash rewrite. Replaces every
    /// stash row + tab row owned by `character_id` in a single
    /// transaction.
    pub fn reset_character_stash(
        &self,
        character_id: Uuid,
        tabs: Vec<PersistedStashTab>,
        items: Vec<PersistedItem>,
    ) -> bool {
        self.tx
            .send(PersistenceMsg::ResetCharacterStash {
                character_id,
                tabs,
                items,
            })
            .is_ok()
    }
}

/// Spawn the persistence worker. Connects to `database_url`, runs
/// the embedded migrations, and parks a dedicated OS thread that
/// owns a tokio multi-thread runtime.
///
/// Returns once the migrations have completed successfully — i.e.
/// the schema is guaranteed to exist by the time this returns
/// `Ok`. The worker keeps running until [`PersistenceHandle::
/// shutdown_blocking`] is called or the last `tx` is dropped.
pub fn spawn(database_url: String) -> anyhow::Result<PersistenceHandle> {
    // Build the runtime + pool synchronously on the calling thread
    // so any setup error (bad URL, DB down, migrations failing) is
    // surfaced before the server enters its main loop.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("rift-persistence")
        .enable_all()
        .build()?;

    let pool: PgPool = rt.block_on(async {
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok::<_, anyhow::Error>(pool)
    })?;

    let (tx, rx) = mpsc::unbounded_channel::<PersistenceMsg>();

    // Move the runtime + pool onto a dedicated OS thread. Any
    // server thread can post messages without ever touching async
    // code itself.
    thread::Builder::new()
        .name("rift-persistence-worker".into())
        .spawn(move || {
            rt.block_on(worker_loop(pool, rx));
        })?;

    Ok(PersistenceHandle { tx })
}

/// Drain `rx` until shutdown, dispatching each message to the
/// matching SQL operation. Each request runs on the runtime's
/// thread pool via `tokio::spawn` so a slow query can't block the
/// next-message dispatch — important for the unbounded fire-and-
/// forget save path.
async fn worker_loop(pool: PgPool, mut rx: mpsc::UnboundedReceiver<PersistenceMsg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            PersistenceMsg::LoadOrCreate {
                account_key,
                display_name,
                character_name,
                class_id,
                gender,
                reply,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let res = load_or_create(
                        &pool,
                        &account_key,
                        &display_name,
                        &character_name,
                        &class_id,
                        gender,
                    )
                    .await;
                    let _ = reply.send(res);
                });
            }
            PersistenceMsg::Save { record } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = save(&pool, &record).await {
                        log::warn!("persistence: save failed for {}: {e}", record.name);
                    }
                });
            }
            PersistenceMsg::ListAccountCharacters {
                account_key,
                display_name,
                reply,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let res = list_account_characters(&pool, &account_key, &display_name).await;
                    let _ = reply.send(res);
                });
            }
            PersistenceMsg::Shutdown { reply } => {
                // Stop pulling new work. Already-spawned tasks
                // continue to completion thanks to the runtime's
                // own shutdown semantics below; we close the pool
                // explicitly to wait for them.
                pool.close().await;
                let _ = reply.send(());
                return;
            }
            PersistenceMsg::LoadInventory {
                character_id,
                reply,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let res = load_inventory(&pool, character_id).await;
                    let _ = reply.send(res);
                });
            }
            PersistenceMsg::AppendInventoryItem { character_id, item } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = append_inventory_item(&pool, character_id, &item).await {
                        log::warn!("persistence: append item failed for {character_id}: {e}");
                    }
                });
            }
            PersistenceMsg::ResetCharacterInventory {
                character_id,
                items,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = reset_character_inventory(&pool, character_id, &items).await {
                        log::warn!("persistence: reset inventory failed for {character_id}: {e}");
                    }
                });
            }
            PersistenceMsg::LoadStash {
                character_id,
                reply,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    let res = load_stash(&pool, character_id).await;
                    let _ = reply.send(res);
                });
            }
            PersistenceMsg::ResetCharacterStash {
                character_id,
                tabs,
                items,
            } => {
                let pool = pool.clone();
                tokio::spawn(async move {
                    if let Err(e) = reset_character_stash(&pool, character_id, &tabs, &items).await
                    {
                        log::warn!("persistence: reset stash failed for {character_id}: {e}");
                    }
                });
            }
        }
    }
    // Sender side dropped: nothing left to do, let the runtime drop.
    pool.close().await;
}

/// Look up `character_name` in `characters`. If absent, create the
/// row — reusing the existing `accounts` row keyed by
/// `account_key` if one already exists, otherwise inserting a
/// fresh account in the same transaction.
///
/// `account_key` is the issuer-tagged storage form
/// (`"dev:<name>"` / `"steam:<id>"`). `display_name` is
/// persisted on first-insert and ignored thereafter.
async fn load_or_create(
    pool: &PgPool,
    account_key: &str,
    display_name: &str,
    character_name: &str,
    class_id: &str,
    gender: i16,
) -> Result<CharacterRecord, PersistenceError> {
    let mut tx = pool.begin().await?;

    // Look up or create the parent account. Plain SELECT…INSERT
    // is fine here — we hold a transaction so a concurrent insert
    // would block on the unique-key constraint, and we re-check
    // after commit on the next handshake.
    let existing_account: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM accounts WHERE account_key = $1")
            .bind(account_key)
            .fetch_optional(&mut *tx)
            .await?;
    let account_id = match existing_account {
        Some((id,)) => id,
        None => {
            let id = Uuid::new_v4();
            // `accounts.name` is now nullable, but populating it
            // alongside `account_key` keeps any in-flight tooling /
            // dashboards that still display the legacy column
            // useful. Identical value to `account_key` so there's
            // no semantic drift between the two.
            sqlx::query(
                "INSERT INTO accounts (id, account_key, display_name, name) \
                 VALUES ($1, $2, $3, $2)",
            )
            .bind(id)
            .bind(account_key)
            .bind(display_name)
            .execute(&mut *tx)
            .await?;
            id
        }
    };

    // Character names are unique *per account*. Look up first;
    // only insert if the row genuinely doesn't exist for this
    // account.
    if let Some(rec) = fetch_by_account_and_name(&mut tx, account_id, character_name).await? {
        tx.commit().await?;
        return Ok(rec);
    }

    let character_id = Uuid::new_v4();
    // Empty bar except for Steady Shot in slot 0. Mirrors
    // `rift_game::loadout::Loadout::default_hero()`. The 255
    // entries are the EMPTY_SLOT sentinel.
    let default_loadout: [i16; 6] = [0, 255, 255, 255, 255, 255];
    sqlx::query(
        "INSERT INTO characters \
         (id, account_id, name, class_id, gender, level, xp, loadout, deepest_cleared_floor, shards) \
         VALUES ($1, $2, $3, $4, $5, 1, 0, $6, 0, 0)",
    )
    .bind(character_id)
    .bind(account_id)
    .bind(character_name)
    .bind(class_id)
    .bind(gender)
    .bind(&default_loadout[..])
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(CharacterRecord {
        id: character_id,
        account_id,
        name: character_name.to_string(),
        class_id: class_id.to_string(),
        gender,
        level: 1,
        xp: 0,
        loadout: default_loadout,
        deepest_cleared_floor: 0,
        shards: 0,
    })
}

/// Resolve `account_key` and return every character row that
/// belongs to it. Creates the account row if missing (using
/// `display_name` for the human-readable label) so a brand-new
/// player still gets a stable `account_id`.
async fn list_account_characters(
    pool: &PgPool,
    account_key: &str,
    display_name: &str,
) -> Result<(Uuid, Vec<CharacterRecord>), PersistenceError> {
    let mut tx = pool.begin().await?;
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM accounts WHERE account_key = $1")
            .bind(account_key)
            .fetch_optional(&mut *tx)
            .await?;
    let account_id = match existing {
        Some((id,)) => id,
        None => {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO accounts (id, account_key, display_name, name) \
                 VALUES ($1, $2, $3, $2)",
            )
            .bind(id)
            .bind(account_key)
            .bind(display_name)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    let rows: Vec<(Uuid, String, String, i16, i32, i32, Vec<i16>, i32, i32)> = sqlx::query_as(
        "SELECT id, name, class_id, gender, level, xp, loadout, deepest_cleared_floor, shards \
         FROM characters WHERE account_id = $1 ORDER BY created_at",
    )
    .bind(account_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let characters = rows
        .into_iter()
        .map(
            |(id, name, class_id, gender, level, xp, loadout, deepest_cleared_floor, shards)| {
                CharacterRecord {
                    id,
                    account_id,
                    name,
                    class_id,
                    gender,
                    level,
                    xp,
                    loadout: loadout_from_vec(loadout),
                    deepest_cleared_floor,
                    shards,
                }
            },
        )
        .collect();
    Ok((account_id, characters))
}

async fn fetch_by_account_and_name(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: Uuid,
    name: &str,
) -> Result<Option<CharacterRecord>, PersistenceError> {
    let row: Option<(Uuid, String, String, i16, i32, i32, Vec<i16>, i32, i32)> = sqlx::query_as(
        "SELECT id, name, class_id, gender, level, xp, loadout, deepest_cleared_floor, shards \
         FROM characters WHERE account_id = $1 AND name = $2",
    )
    .bind(account_id)
    .bind(name)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(id, name, class_id, gender, level, xp, loadout, deepest_cleared_floor, shards)| {
            CharacterRecord {
                id,
                account_id,
                name,
                class_id,
                gender,
                level,
                xp,
                loadout: loadout_from_vec(loadout),
                deepest_cleared_floor,
                shards,
            }
        },
    ))
}

async fn save(pool: &PgPool, rec: &CharacterRecord) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE characters SET \
            class_id = $2, gender = $3, level = $4, xp = $5, \
            loadout = $6, deepest_cleared_floor = $7, shards = $8, updated_at = now() \
         WHERE id = $1",
    )
    .bind(rec.id)
    .bind(&rec.class_id)
    .bind(rec.gender)
    .bind(rec.level)
    .bind(rec.xp)
    .bind(&rec.loadout[..])
    .bind(rec.deepest_cleared_floor)
    .bind(rec.shards)
    .execute(pool)
    .await?;
    Ok(())
}

/// Convert a postgres `SMALLINT[]` row read into a fixed-size
/// `[i16; 6]`. Pads with the default loadout if the column came
/// back short or oversized so a manually-edited DB row can't
/// crash the worker.
fn loadout_from_vec(v: Vec<i16>) -> [i16; 6] {
    // Mirrors `Loadout::default_hero()` — Steady Shot in slot 0,
    // every other slot empty. Used as a fallback so a
    // manually-edited DB row can't crash the worker.
    let default: [i16; 6] = [0, 255, 255, 255, 255, 255];
    let mut out = default;
    for (i, slot) in v.into_iter().take(6).enumerate() {
        out[i] = slot;
    }
    out
}

/// Read every `inventory_items` row belonging to `character_id`,
/// ordered oldest-first so the server can rebuild the bag in
/// pickup order.
async fn load_inventory(
    pool: &PgPool,
    character_id: Uuid,
) -> Result<Vec<PersistedItem>, PersistenceError> {
    let rows: Vec<(
        String,
        i16,
        i32,
        sqlx::types::Json<Vec<AffixJson>>,
        Option<i16>,
        i32,
        bool,
        Option<Vec<Uuid>>,
        Option<String>,
        Option<i16>,
        Option<String>,
        Option<f32>,
        Option<i16>,
    )> = sqlx::query_as(
        "SELECT base_id, rarity, ilvl, affixes, equipped_slot, slot_index, anchored, provenance, unique_id, unique_pick, \
                rift_touched_id, rift_touched_value, rift_touched_depth \
             FROM inventory_items \
             WHERE character_id = $1 \
             ORDER BY equipped_slot NULLS LAST, slot_index, acquired_at, id",
    )
    .bind(character_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                base_id,
                rarity,
                ilvl,
                affixes,
                equipped_slot,
                slot_index,
                anchored,
                provenance,
                unique_id,
                unique_pick,
                rt_id,
                rt_value,
                rt_depth,
            )| {
                PersistedItem {
                    base_id,
                    rarity,
                    ilvl,
                    affixes: affixes.0.into_iter().map(|a| (a.id, a.v)).collect(),
                    equipped_slot,
                    slot_index,
                    anchored,
                    tab_index: 0,
                    provenance,
                    unique_id,
                    unique_pick,
                    rift_touched: rift_touched_from_columns(rt_id, rt_value, rt_depth),
                }
            },
        )
        .collect())
}

/// Insert one rolled drop under `character_id`. The PK is freshly
/// minted here so the caller doesn't have to track per-item ids.
async fn append_inventory_item(
    pool: &PgPool,
    character_id: Uuid,
    item: &PersistedItem,
) -> Result<(), sqlx::Error> {
    let affixes_json: Vec<AffixJson> = item
        .affixes
        .iter()
        .map(|(id, v)| AffixJson {
            id: id.clone(),
            v: *v,
        })
        .collect();
    // Pickups land at the end of the bag. Compute the next
    // free `slot_index` inline so concurrent appends don't
    // collide on a stale client-side counter.
    sqlx::query(
        "INSERT INTO inventory_items \
         (id, character_id, base_id, rarity, ilvl, affixes, equipped_slot, slot_index, anchored, provenance, unique_id, unique_pick, rift_touched_id, rift_touched_value, rift_touched_depth) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, \
            COALESCE((SELECT MAX(slot_index) + 1 FROM inventory_items \
                      WHERE character_id = $2 AND equipped_slot IS NULL), 0), $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(Uuid::new_v4())
    .bind(character_id)
    .bind(&item.base_id)
    .bind(item.rarity)
    .bind(item.ilvl)
    .bind(sqlx::types::Json(affixes_json))
    .bind(item.equipped_slot)
    .bind(item.anchored)
    .bind(item.provenance.as_deref())
    .bind(item.unique_id.as_deref())
    .bind(item.unique_pick)
    .bind(item.rift_touched.as_ref().map(|(id, _, _)| id.as_str()))
    .bind(item.rift_touched.as_ref().map(|(_, v, _)| *v))
    .bind(item.rift_touched.as_ref().map(|(_, _, d)| *d))
    .execute(pool)
    .await?;
    Ok(())
}

/// Replace every `inventory_items` row owned by `character_id`
/// with `items` in a single transaction. Used by the equipment
/// subsystem on every equip / unequip so the persisted snapshot
/// stays consistent with the in-memory bag + equipped set
/// without threading per-item UUIDs through the runtime `Item`.
async fn reset_character_inventory(
    pool: &PgPool,
    character_id: Uuid,
    items: &[PersistedItem],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM inventory_items WHERE character_id = $1")
        .bind(character_id)
        .execute(&mut *tx)
        .await?;
    for item in items {
        let affixes_json: Vec<AffixJson> = item
            .affixes
            .iter()
            .map(|(id, v)| AffixJson {
                id: id.clone(),
                v: *v,
            })
            .collect();
        sqlx::query(
            "INSERT INTO inventory_items \
             (id, character_id, base_id, rarity, ilvl, affixes, equipped_slot, slot_index, anchored, provenance, unique_id, unique_pick, rift_touched_id, rift_touched_value, rift_touched_depth) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(Uuid::new_v4())
        .bind(character_id)
        .bind(&item.base_id)
        .bind(item.rarity)
        .bind(item.ilvl)
        .bind(sqlx::types::Json(affixes_json))
        .bind(item.equipped_slot)
        .bind(item.slot_index)
        .bind(item.anchored)
        .bind(item.provenance.as_deref())
        .bind(item.unique_id.as_deref())
        .bind(item.unique_pick)
        .bind(item.rift_touched.as_ref().map(|(id, _, _)| id.as_str()))
        .bind(item.rift_touched.as_ref().map(|(_, v, _)| *v))
        .bind(item.rift_touched.as_ref().map(|(_, _, d)| *d))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Read every `stash_items` row belonging to `character_id`,
/// oldest-first so the order in which the player deposited is
/// preserved across sessions, plus the per-tab metadata rows
/// from `stash_tabs`. Tabs come back in `tab_index` order;
/// callers should treat the list as the dense `[0..n)` set of
/// tabs the character owns. If the table has no rows the
/// caller is expected to seed a default tab 0 client-side.
async fn load_stash(
    pool: &PgPool,
    character_id: Uuid,
) -> Result<(Vec<PersistedStashTab>, Vec<PersistedItem>), PersistenceError> {
    let tab_rows: Vec<(i16, String, i32)> = sqlx::query_as(
        "SELECT tab_index, name, color \
         FROM stash_tabs \
         WHERE character_id = $1 \
         ORDER BY tab_index",
    )
    .bind(character_id)
    .fetch_all(pool)
    .await?;
    let tabs: Vec<PersistedStashTab> = tab_rows
        .into_iter()
        .map(|(tab_index, name, color)| PersistedStashTab {
            tab_index,
            name,
            color,
        })
        .collect();

    let rows: Vec<(
        String,
        i16,
        i32,
        sqlx::types::Json<Vec<AffixJson>>,
        i32,
        bool,
        i16,
        Option<Vec<Uuid>>,
        Option<String>,
        Option<i16>,
        Option<String>,
        Option<f32>,
        Option<i16>,
    )> = sqlx::query_as(
        "SELECT base_id, rarity, ilvl, affixes, slot_index, anchored, tab_index, provenance, unique_id, unique_pick, \
                rift_touched_id, rift_touched_value, rift_touched_depth \
             FROM stash_items \
             WHERE character_id = $1 \
             ORDER BY tab_index, slot_index, acquired_at, id",
    )
    .bind(character_id)
    .fetch_all(pool)
    .await?;
    let items = rows
        .into_iter()
        .map(
            |(
                base_id,
                rarity,
                ilvl,
                affixes,
                slot_index,
                anchored,
                tab_index,
                provenance,
                unique_id,
                unique_pick,
                rt_id,
                rt_value,
                rt_depth,
            )| {
                PersistedItem {
                    base_id,
                    rarity,
                    ilvl,
                    affixes: affixes.0.into_iter().map(|a| (a.id, a.v)).collect(),
                    equipped_slot: None,
                    slot_index,
                    anchored,
                    tab_index,
                    provenance,
                    unique_id,
                    unique_pick,
                    rift_touched: rift_touched_from_columns(rt_id, rt_value, rt_depth),
                }
            },
        )
        .collect();
    Ok((tabs, items))
}

/// Replace every `stash_items` + `stash_tabs` row owned by
/// `character_id` with the given snapshot in a single
/// transaction. Mirrors [`reset_character_inventory`] in shape;
/// also rewrites tab metadata so name / color / tab count edits
/// are durable.
async fn reset_character_stash(
    pool: &PgPool,
    character_id: Uuid,
    tabs: &[PersistedStashTab],
    items: &[PersistedItem],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM stash_items WHERE character_id = $1")
        .bind(character_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM stash_tabs WHERE character_id = $1")
        .bind(character_id)
        .execute(&mut *tx)
        .await?;
    for tab in tabs {
        sqlx::query(
            "INSERT INTO stash_tabs (character_id, tab_index, name, color) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(character_id)
        .bind(tab.tab_index)
        .bind(&tab.name)
        .bind(tab.color)
        .execute(&mut *tx)
        .await?;
    }
    for item in items {
        let affixes_json: Vec<AffixJson> = item
            .affixes
            .iter()
            .map(|(id, v)| AffixJson {
                id: id.clone(),
                v: *v,
            })
            .collect();
        sqlx::query(
            "INSERT INTO stash_items \
             (id, character_id, base_id, rarity, ilvl, affixes, slot_index, anchored, tab_index, provenance, unique_id, unique_pick, rift_touched_id, rift_touched_value, rift_touched_depth) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(Uuid::new_v4())
        .bind(character_id)
        .bind(&item.base_id)
        .bind(item.rarity)
        .bind(item.ilvl)
        .bind(sqlx::types::Json(affixes_json))
        .bind(item.slot_index)
        .bind(item.anchored)
        .bind(item.tab_index)
        .bind(item.provenance.as_deref())
        .bind(item.unique_id.as_deref())
        .bind(item.unique_pick)
        .bind(item.rift_touched.as_ref().map(|(id, _, _)| id.as_str()))
        .bind(item.rift_touched.as_ref().map(|(_, v, _)| *v))
        .bind(item.rift_touched.as_ref().map(|(_, _, d)| *d))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
