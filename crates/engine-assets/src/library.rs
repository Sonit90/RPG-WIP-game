//! Naming models, and holding them.
//!
//! The indirection that makes the phase's core requirement true: gameplay-adjacent code
//! names a model with a [`ModelId`], and what that resolves to — a procedural mesh today, an
//! artist's `.glb` tomorrow — is a decision made in one place at load.
//!
//! # Why this is a `Vec` and not a `HashMap`
//!
//! A scene registers a handful of models once and then addresses them by index every frame.
//! A map would be a hash per frame per character to avoid a linear scan that only ever runs
//! at load. So: names are looked up once through [`ModelLibrary::index_of`], and everything
//! after that is [`ModelIndex`].

use std::sync::Arc;

use crate::error::AssetError;
use crate::model::ModelAsset;

/// The name of a model, as configuration writes it.
///
/// Cheap to clone and compare — an `Arc<str>` rather than a `String` — because a character
/// definition holds one and gets cloned wherever definitions are collected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelId(Arc<str>);

impl ModelId {
    /// Names a model.
    #[must_use]
    pub fn new(name: impl AsRef<str>) -> Self {
        Self(Arc::from(name.as_ref()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for ModelId {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

/// Where a model sits in a [`ModelLibrary`].
///
/// A plain index, not a generational handle: models are registered during scene setup and
/// never removed, so there is no freed slot to mistake for a live one. If models ever become
/// unloadable this becomes a generational handle, and the type existing separately from
/// `usize` is what makes that a change to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelIndex(usize);

impl ModelIndex {
    /// The raw index.
    #[must_use]
    pub fn get(self) -> usize {
        self.0
    }
}

/// Every model a scene has loaded.
#[derive(Debug, Default, Clone)]
pub struct ModelLibrary {
    ids: Vec<ModelId>,
    models: Vec<Arc<ModelAsset>>,
}

impl ModelLibrary {
    /// An empty library.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `model` under `id`, replacing any model already registered under it.
    ///
    /// Replacement rather than rejection, because that is the operation a future hot-reload
    /// performs, and because a scene that registers the same id twice has a configuration
    /// bug that a silent first-wins would hide behind stale geometry.
    pub fn insert(&mut self, id: ModelId, model: ModelAsset) -> ModelIndex {
        let model = Arc::new(model);
        if let Some(existing) = self.ids.iter().position(|known| *known == id) {
            self.models[existing] = model;
            return ModelIndex(existing);
        }
        self.ids.push(id);
        self.models.push(model);
        ModelIndex(self.models.len() - 1)
    }

    /// Where `id` is, if it is registered.
    #[must_use]
    pub fn index_of(&self, id: &ModelId) -> Option<ModelIndex> {
        self.ids
            .iter()
            .position(|known| known == id)
            .map(ModelIndex)
    }

    /// The model at `index`.
    #[must_use]
    pub fn get(&self, index: ModelIndex) -> Option<&Arc<ModelAsset>> {
        self.models.get(index.0)
    }

    /// The model named `id`.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::NotFound`] rather than `None`, because the caller of this — a
    /// scene resolving a character definition — wants to report *which* name failed.
    pub fn require(&self, id: &ModelId) -> Result<(ModelIndex, &Arc<ModelAsset>), AssetError> {
        let Some(index) = self.index_of(id) else {
            return Err(AssetError::NotFound {
                model: id.to_string(),
            });
        };
        let Some(model) = self.models.get(index.0) else {
            return Err(AssetError::NotFound {
                model: id.to_string(),
            });
        };
        Ok((index, model))
    }

    /// How many models are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every registered model, with its name.
    pub fn iter(&self) -> impl Iterator<Item = (&ModelId, &Arc<ModelAsset>)> {
        self.ids.iter().zip(&self.models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::MaterialAsset;
    use crate::mesh::{Geometry, MeshVertex, SubMesh};
    use engine_core::math::{Vec2, Vec3};

    fn model(name: &str) -> ModelAsset {
        let vertices = vec![
            MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::X, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::Y, Vec3::Y, Vec2::ZERO),
        ];
        ModelAsset::rigid(
            name,
            vec![SubMesh::new(
                "tri",
                Geometry::Static(vertices),
                vec![0, 1, 2],
                0,
            )],
            vec![MaterialAsset::default()],
            vec![],
        )
    }

    #[test]
    fn a_registered_model_is_found_by_name_and_then_by_index() {
        let mut library = ModelLibrary::new();
        let index = library.insert(ModelId::new("fire_mage"), model("fire_mage"));
        assert_eq!(library.index_of(&ModelId::new("fire_mage")), Some(index));
        assert!(library.get(index).is_some());
        assert_eq!(library.len(), 1);
    }

    #[test]
    fn an_unknown_name_reports_itself() {
        let library = ModelLibrary::new();
        let Err(error) = library.require(&ModelId::new("wizard")) else {
            panic!("an empty library cannot resolve anything");
        };
        assert!(error.to_string().contains("wizard"), "{error}");
    }

    #[test]
    fn registering_a_name_twice_replaces_rather_than_growing() {
        // What a hot reload does, and what stops a configuration bug hiding behind stale
        // geometry.
        let mut library = ModelLibrary::new();
        let first = library.insert(ModelId::new("dummy"), model("v1"));
        let second = library.insert(ModelId::new("dummy"), model("v2"));
        assert_eq!(first, second);
        assert_eq!(library.len(), 1);
        let Some(current) = library.get(second) else {
            panic!("the slot must hold something");
        };
        assert_eq!(current.name, "v2");
    }

    #[test]
    fn ids_compare_by_text_not_by_allocation() {
        assert_eq!(ModelId::new("a"), ModelId::from("a"));
        assert_ne!(ModelId::new("a"), ModelId::new("b"));
        assert_eq!(ModelId::new("a").as_str(), "a");
    }
}
