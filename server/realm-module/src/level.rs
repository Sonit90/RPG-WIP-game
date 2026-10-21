//! The catalog and each dungeon's level, parsed once per module instance and kept.
//!
//! A level is a 370 KB JSON document that takes milliseconds to parse, and every fight is
//! built from it. So it is parsed on first use and cached for the life of the wasm instance.
//! It is a pure function of compiled-in data, so a fresh instance parsing it again changes
//! nothing — the cache affects cost, never results.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use engine_environment::EnvironmentScene;
use engine_realm::RealmError;
use engine_realm::catalog::{Catalog, DungeonDefinition, embedded_scene};
use spacetimedb::ReducerContext;

use crate::schema::*;

/// A dungeon's compiled data.
pub struct Dungeon {
    /// Its catalog row.
    pub definition: DungeonDefinition,
    /// Its level.
    pub scene: EnvironmentScene,
}

thread_local! {
    static CATALOG: RefCell<Option<Rc<Catalog>>> = const { RefCell::new(None) };
    static DUNGEONS: RefCell<BTreeMap<String, Rc<Dungeon>>> = const { RefCell::new(BTreeMap::new()) };
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

/// A dungeon's compiled data, building it on first use. `cm:` ids are community maps, built
/// from the `community_map` table through the same mapcheck the API ran (a version never
/// changes, so the cache is as good for them as for the official levels).
pub fn dungeon(ctx: &ReducerContext, id: &str) -> Result<Rc<Dungeon>, RealmError> {
    if let Some(found) = DUNGEONS.with(|cell| cell.borrow().get(id).cloned()) {
        return Ok(found);
    }
    if engine_realm::community::is_community(id) {
        return community(ctx, id);
    }
    let catalog = catalog()?;
    let definition = catalog
        .dungeon(id)
        .cloned()
        .ok_or_else(|| RealmError::dungeon("That dungeon does not exist."))?;
    let text = embedded_scene(&definition.scene).ok_or_else(|| {
        log::error!(
            "event=scene_missing dungeon_id={id} scene={}",
            definition.scene
        );
        RealmError::dungeon("That dungeon is not available on this realm.")
    })?;
    let scene =
        engine_environment::scene_file::parse(text, &definition.scene).map_err(|error| {
            log::error!("event=scene_invalid dungeon_id={id} error={error}");
            RealmError::dungeon("That dungeon is not available on this realm.")
        })?;
    let built = Rc::new(Dungeon { definition, scene });
    DUNGEONS.with(|cell| {
        cell.borrow_mut().insert(id.to_owned(), Rc::clone(&built));
    });
    log::info!("event=dungeon_compiled dungeon_id={id}");
    Ok(built)
}

/// A community map built from its stored text. Refused (and logged) if the text no longer
/// validates: the realm never runs a map it has not checked.
fn community(ctx: &ReducerContext, id: &str) -> Result<Rc<Dungeon>, RealmError> {
    let row = ctx
        .db
        .community_map()
        .id()
        .find(id.to_owned())
        .ok_or_else(|| RealmError::dungeon("That community map is not available on this realm."))?;
    let catalog = catalog()?;
    let template = catalog
        .dungeon(engine_realm::community::TEMPLATE_DUNGEON)
        .ok_or_else(|| RealmError::protocol("The realm's catalog has no template dungeon."))?;
    let prepared = engine_realm::community::prepare(&row.data, id, template).map_err(|report| {
        log::error!(
            "event=community_invalid dungeon_id={id} errors={}",
            report.errors.len()
        );
        RealmError::dungeon("That community map is not available on this realm.")
    })?;
    let built = Rc::new(Dungeon {
        definition: prepared.definition,
        scene: prepared.scene,
    });
    DUNGEONS.with(|cell| {
        cell.borrow_mut().insert(id.to_owned(), Rc::clone(&built));
    });
    log::info!("event=community_compiled dungeon_id={id}");
    Ok(built)
}

/// A dungeon's catalog row, official or community.
pub fn definition(ctx: &ReducerContext, id: &str) -> Result<DungeonDefinition, RealmError> {
    if engine_realm::community::is_community(id) {
        return dungeon(ctx, id).map(|built| built.definition.clone());
    }
    catalog()?
        .dungeon(id)
        .cloned()
        .ok_or_else(|| RealmError::matchmaking("That dungeon does not exist."))
}
