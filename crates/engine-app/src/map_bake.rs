//! The dungeon map's bake: the level, drawn from straight above by the real renderer, styled
//! into a map and kept on disk.
//!
//! ```text
//!  EnvironmentScene ─► frame_for ─► texture size ─► tiles of the back buffer
//!         │                                             │  Renderer::capture_world
//!         │        a RenderWorld of the static level ───┘  (orthographic, top-down)
//!         ▼                                             ▼
//!   walkable_mask ───────────────────────────────► style_map ─► <level>.map.png
//!                                                             └► <level>.map.cfg
//! ```
//!
//! # When it runs
//!
//! Once, when a level loads — and only if the picture on disk is missing or describes other
//! geometry ([`engine_environment::map::MapDocument::matches`]). A loaded level whose walls
//! have not moved reuses its picture: a PNG decode and one texture upload, no rendering. The
//! editor's *Bake map* command and `--bake-map` force it. Nothing here runs per frame.
//!
//! # What it draws
//!
//! A world of its own holding the static level only — every placement at its most detailed
//! level, the ground and the water — so nothing that moves (the player, the monsters, a
//! debug marker) is ever baked into the picture. It shares the live renderer's meshes,
//! materials and textures, so the map is the level as the game draws it, not a second
//! description of it.
//!
//! # Tiles
//!
//! The capture is the back buffer, so a map larger than the window is drawn a window-sized
//! tile at a time, each tile an orthographic camera shifted by exactly its own width. The
//! texel grid is shared by every tile, so the seams line up to the texel.

use std::path::{Path, PathBuf};
use std::time::Instant;

use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_environment::map::{self, MapDocument, MapFrame};
use engine_presentation::EnvironmentVisuals;
use engine_render_data::{Camera, Projection, RenderEntity, RenderEntityDesc, RenderWorld};
use engine_renderer::Renderer;

/// What the ground beyond the level is cleared to before the capture.
const CLEAR: [f32; 4] = [0.05, 0.04, 0.03, 1.0];

/// A map picture, ready to upload.
#[derive(Debug, Clone)]
pub struct MapPicture {
    /// The world rectangle it shows.
    pub frame: MapFrame,
    /// Texels across.
    pub width: u32,
    /// Texels down.
    pub height: u32,
    /// Straight RGBA8, row zero north.
    pub texels: Vec<u8>,
    /// Milliseconds it took to produce: the whole bake, or the load from disk.
    pub milliseconds: f32,
    /// Whether it was baked just now rather than read from disk.
    pub baked: bool,
    /// How many captures the bake took.
    pub tiles: u32,
}

/// The `assets/` directory a level's generated map belongs in.
///
/// The level file's own asset root when it is `…/assets/scenes/<level>.json`, so a map is
/// kept beside the level it was baked from; otherwise the first asset root that exists.
#[must_use]
pub fn map_root(scene_file: Option<&Path>) -> PathBuf {
    if let Some(root) = scene_file
        .and_then(Path::parent)
        .filter(|parent| parent.file_name().is_some_and(|name| name == "scenes"))
        .and_then(Path::parent)
    {
        return root.to_path_buf();
    }
    engine_assets::disk::asset_roots()
        .into_iter()
        .find(|root| root.is_dir())
        .unwrap_or_else(|| PathBuf::from("assets"))
}

/// The picture on disk for `scene`, if there is one and it describes this geometry.
#[must_use]
pub fn load_cached(scene: &EnvironmentScene, root: &Path) -> Option<MapPicture> {
    let started = Instant::now();
    let (png, json) = map::map_paths(root, &scene.name);
    let document = MapDocument::load(&json)?;
    if !document.matches(scene) {
        log::info!(
            "map '{}': the baked picture is stale; baking again",
            scene.name
        );
        return None;
    }
    let texture = match engine_assets::map_image::load_png(&png) {
        Ok(texture) => texture,
        Err(error) => {
            log::warn!("map '{}': {error}; baking again", scene.name);
            return None;
        }
    };
    if (texture.width, texture.height) != (document.width, document.height) {
        log::warn!(
            "map '{}': the picture is not the size its document says; baking again",
            scene.name
        );
        return None;
    }
    Some(MapPicture {
        frame: document.frame(),
        width: texture.width,
        height: texture.height,
        texels: texture.texels,
        milliseconds: started.elapsed().as_secs_f32() * 1e3,
        baked: false,
        tiles: 0,
    })
}

/// Bakes `scene`'s map through `renderer` and writes it under `root`.
///
/// `visuals` are the uploaded library the level is placed from; `world` and `statics` are the
/// live world and the entities in it that belong to the level without being placements — the
/// ground and the water — copied into the bake's own world.
///
/// # Errors
///
/// A description of what failed: a window with no area, a capture the renderer refused, or
/// a styling error. Writing the files is not one — a map that could not be saved is still
/// shown, and the failure is logged.
pub fn bake(
    renderer: &mut Renderer,
    visuals: &EnvironmentVisuals,
    world: &RenderWorld,
    statics: &[RenderEntity],
    scene: &EnvironmentScene,
    root: &Path,
) -> Result<MapPicture, String> {
    let started = Instant::now();
    let frame = map::frame_for(scene);
    let (width, height) = frame.texture_size(scene.map.texels_per_metre);
    let (buffer_width, buffer_height) = renderer.size();
    if buffer_width == 0 || buffer_height == 0 {
        return Err("the window has no area to draw the map in".to_owned());
    }

    // The level and nothing else, at its most detailed.
    let mut level = RenderWorld::new();
    let mut placer = engine_presentation::EnvironmentPlacer::new(visuals.clone());
    placer.place(&mut level, scene, Vec3::ZERO, 0.0);
    for &entity in statics {
        copy_entity(world, entity, &mut level);
    }
    // A surveyor's light, not the level's: high and from the north-west, bright enough that a
    // crypt lit by the moon still shows its floor and the tops of its walls. The level's own
    // lights still add their pools on top — a brazier is on the map because it is in the room.
    let mut lighting = engine_presentation::scene_lighting(&scene.lighting);
    lighting.direction = Vec3::new(0.35, -1.0, 0.45).normalize();
    lighting.color = Vec3::new(1.0, 0.97, 0.9);
    lighting.intensity = 2.6;
    lighting.ambient_color = Vec3::new(0.85, 0.85, 0.9);
    lighting.ambient_intensity = 0.55;

    let (floor, ceiling) = scene.map.vertical_range(scene.ground_height);
    let metres_per_texel = frame.width() / width as f32;
    let tile_width = buffer_width.min(width);
    let tile_height = buffer_height.min(height);
    let tiles_x = width.div_ceil(tile_width);
    let tiles_y = height.div_ceil(tile_height);
    let mut capture = vec![0u8; width as usize * height as usize * 4];
    let mut tiles = 0;
    for tile_y in 0..tiles_y {
        for tile_x in 0..tiles_x {
            // The camera always sees a whole back buffer; the tile keeps the part of it that
            // lands inside the map.
            let span_x = buffer_width as f32 * metres_per_texel;
            let span_z = buffer_height as f32 * metres_per_texel;
            let x0 = frame.min_x + (tile_x * tile_width) as f32 * metres_per_texel;
            let z0 = frame.min_z + (tile_y * tile_height) as f32 * metres_per_texel;
            let center = Vec3::new(x0 + span_x * 0.5, 0.0, z0 + span_z * 0.5);
            let mut camera = Camera::new(Projection::Orthographic {
                half_height: span_z * 0.5,
                aspect: buffer_width as f32 / buffer_height as f32,
                near: 0.05,
                far: ceiling - floor + 0.05,
            })
            .map_err(|error| format!("the map camera: {error}"))?;
            // Straight down, with north (−Z) at the top of the picture and east (+X) right.
            camera
                .look_at(
                    Vec3::new(center.x, ceiling, center.z),
                    Vec3::new(center.x, floor, center.z),
                    Vec3::new(0.0, 0.0, -1.0),
                )
                .map_err(|error| format!("the map camera: {error}"))?;
            let image = renderer
                .capture_world(&level, &camera, lighting, CLEAR)
                .map_err(|error| format!("capturing tile {tile_x},{tile_y}: {error}"))?;
            tiles += 1;
            // Copy the tile's rows into the map, clipped to the map's edge.
            let left = (tile_x * tile_width) as usize;
            let top = (tile_y * tile_height) as usize;
            let copy_width = (width as usize - left).min(image.width as usize);
            let copy_height = (height as usize - top).min(image.height as usize);
            for row in 0..copy_height {
                let from = row * image.width as usize * 4;
                let to = ((top + row) * width as usize + left) * 4;
                capture[to..to + copy_width * 4]
                    .copy_from_slice(&image.pixels[from..from + copy_width * 4]);
            }
        }
    }

    // `MYGAME_MAP_CAPTURE=1` keeps the unstyled capture beside the map, for tuning the style.
    if std::env::var_os("MYGAME_MAP_CAPTURE").is_some() {
        let (png, _) = map::map_paths(root, &format!("{}.capture", scene.name));
        if let Err(error) = engine_assets::map_image::save_png(&png, width, height, &capture) {
            log::warn!(
                "map '{}': could not write the raw capture: {error}",
                scene.name
            );
        }
    }
    let mask = map::walkable_mask(scene, &frame, width, height);
    let texels = engine_assets::map_image::style_map(
        &capture,
        (width, height),
        &mask,
        &engine_assets::map_image::MapPalette {
            // Flagstones two metres across, whatever the density.
            tile_texels: (2.0 * width as f32 / frame.width()).max(4.0),
            ..engine_assets::map_image::MapPalette::default()
        },
    )
    .map_err(|error| format!("styling the map: {error}"))?;
    let milliseconds = started.elapsed().as_secs_f32() * 1e3;

    let (png, json) = map::map_paths(root, &scene.name);
    match engine_assets::map_image::save_png(&png, width, height, &texels) {
        Ok(()) => {
            let document = MapDocument::describe(scene, &frame, width, height, milliseconds);
            if let Err(error) = document.save(&json) {
                log::warn!(
                    "map '{}': could not write {}: {error}",
                    scene.name,
                    json.display()
                );
            }
        }
        Err(error) => log::warn!("map '{}': could not write the picture: {error}", scene.name),
    }
    log::info!(
        "map '{}': baked {width}x{height} ({:.1} m) in {milliseconds:.0} ms, {tiles} tile(s), into {}",
        scene.name,
        frame.width().max(frame.depth()),
        png.display()
    );
    Ok(MapPicture {
        frame,
        width,
        height,
        texels,
        milliseconds,
        baked: true,
        tiles,
    })
}

/// Copies one entity of `from` into `to`, if it is alive.
fn copy_entity(from: &RenderWorld, entity: RenderEntity, to: &mut RenderWorld) {
    if !from.contains(entity) {
        return;
    }
    let slot = entity.index();
    to.spawn(RenderEntityDesc {
        transform: from.transforms()[slot],
        mesh: from.meshes()[slot],
        material: from.materials()[slot],
        bounds: from.bounds()[slot],
        visible: true,
        style: from.styles()[slot],
        shadow: from.shadow_casting()[slot],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_under_assets_scenes_keeps_its_map_in_that_assets_folder() {
        let root = map_root(Some(Path::new("C:/game/assets/scenes/crypt.json")));
        assert_eq!(root, PathBuf::from("C:/game/assets"));
    }

    #[test]
    fn a_level_anywhere_else_uses_an_asset_root() {
        let root = map_root(Some(Path::new("C:/elsewhere/crypt.json")));
        assert_ne!(root, PathBuf::from("C:/elsewhere"));
        let _ = map_root(None);
    }
}
