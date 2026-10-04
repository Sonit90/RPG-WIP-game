//! The catalog and each dungeon's collision, built once per module instance and kept.
//!
//! A level is a 370 KB JSON document that takes milliseconds to parse and build, and every
//! movement message needs its collision. So both are built on first use and cached for the
//! life of the wasm instance. They are pure functions of compiled-in data, so a fresh instance
//! rebuilding them changes nothing — the cache affects cost, never results.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use engine_collision::world::QueryBuffers;
use engine_environment::EnvironmentScene;
use engine_realm::RealmError;
use engine_realm::catalog::{Catalog, DungeonDefinition, embedded_scene};
use engine_realm::movement::{Barrier, BarrierKind, LevelGeometry, barriers_of};

/// A dungeon's compiled data.
pub struct Dungeon {
    /// Its catalog row.
    pub definition: DungeonDefinition,
    /// Its level.
    pub scene: EnvironmentScene,
    /// Collision with the start barrier standing (countdown).
    pub closed: LevelGeometry,
    /// Collision with the start barrier lifted (the run).
    pub open: LevelGeometry,
}

thread_local! {
    static CATALOG: RefCell<Option<Rc<Catalog>>> = const { RefCell::new(None) };
    static DUNGEONS: RefCell<BTreeMap<String, Rc<Dungeon>>> = const { RefCell::new(BTreeMap::new()) };
    static BUFFERS: RefCell<QueryBuffers> = RefCell::new(QueryBuffers::new());
}

/// The catalog this module was built with.
pub fn catalog() -> Result<Rc<Catalog>, RealmError> {
    CATALOG.with(|cell| {
        if let Some(catalog) = cell.borrow().as_ref() {
            return Ok(Rc::clone(catalog));
        }
        let catalog = Rc::new(Catalog::builtin().map_err(|error| {
            log::error!("event=catalog_invalid error={error}");
            RealmError::protocol("The realm's catalog is invalid.")
        })?);
        *cell.borrow_mut() = Some(Rc::clone(&catalog));
        Ok(catalog)
    })
}

/// A dungeon's compiled data, building it on first use.
pub fn dungeon(id: &str) -> Result<Rc<Dungeon>, RealmError> {
    if let Some(found) = DUNGEONS.with(|cell| cell.borrow().get(id).cloned()) {
        return Ok(found);
    }
    let catalog = catalog()?;
    let definition = catalog
        .dungeon(id)
        .cloned()
        .ok_or_else(|| RealmError::dungeon("That dungeon does not exist."))?;
    let text = embedded_scene(&definition.scene).ok_or_else(|| {
        log::error!("event=scene_missing dungeon_id={id} scene={}", definition.scene);
        RealmError::dungeon("That dungeon is not available on this realm.")
    })?;
    let scene = engine_environment::scene_file::parse(text, &definition.scene).map_err(|error| {
        log::error!("event=scene_invalid dungeon_id={id} error={error}");
        RealmError::dungeon("That dungeon is not available on this realm.")
    })?;
    let barriers = barriers_of(&scene, &definition);
    // The boss gate stays shut online until server-side combat can open it (Phase 9).
    let without_start: Vec<Barrier> = barriers
        .iter()
        .copied()
        .filter(|barrier| barrier.kind != BarrierKind::Start)
        .collect();
    let closed = LevelGeometry::build(&scene, &barriers);
    let open = LevelGeometry::build(&scene, &without_start);
    let built = Rc::new(Dungeon {
        definition,
        scene,
        closed,
        open,
    });
    DUNGEONS.with(|cell| {
        cell.borrow_mut().insert(id.to_owned(), Rc::clone(&built));
    });
    log::info!("event=dungeon_compiled dungeon_id={id}");
    Ok(built)
}

/// Runs `body` with the shared collision query buffers.
pub fn with_buffers<T>(body: impl FnOnce(&mut QueryBuffers) -> T) -> T {
    BUFFERS.with(|cell| body(&mut cell.borrow_mut()))
}
