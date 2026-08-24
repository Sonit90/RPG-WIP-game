//! glTF 2.0 / GLB import: an artist's export, turned into the runtime representation.
//!
//! ```text
//!  fire_mage.glb ──► gltf::Gltf ──► ModelAsset ──► validate_model ──► the same
//!                    (parse)        (translate)     (prove)            renderer path
//!                                                                      the procedural
//!                                                                      model takes
//! ```
//!
//! # Why glTF and not a custom format
//!
//! Every DCC tool exports it, it is a documented standard, its coordinate system is already
//! this engine's — right-handed, Y up, -Z forward, counter-clockwise front faces — and its
//! material model is metallic-roughness, which is what [`crate::MaterialAsset`] already is.
//! Inventing a format here would buy a slightly faster load and cost a converter that has to
//! be written, maintained and taught to every artist.
//!
//! # What is deliberately not supported
//!
//! * **Cubic spline interpolation.** Rejected rather than approximated: silently resampling
//!   a spline as linear changes timing the animator authored.
//! * **More than four influences per vertex.** glTF allows `JOINTS_1`; the runtime vertex has
//!   four slots. Rejected rather than truncated, because dropping the fifth influence
//!   unnormalizes the weights and deflates the mesh.
//!
//! Several skins *are* supported, and become one skeleton: an export that skins the body and
//! the clothing separately describes one hierarchy twice, and the runtime poses one skeleton
//! per character. See [`import_skeleton`].
//! * **Morph targets, cameras, lights, extensions.** Ignored — they are not this engine's
//!   concern yet, and ignoring them lets a file that contains them still deliver its meshes.
//! * **External `.bin` and image files.** Only GLB binary chunks and `data:` URIs are read,
//!   because the runtime has no filesystem asset resolver and inventing one is a separate
//!   decision. A `.gltf` with sidecar files is the one case that reports
//!   [`AssetError::Unsupported`] with a precise reason.
//! * **Levels of detail.** A glTF file describes one. Extra levels are added by the caller
//!   through [`crate::ModelAsset::with_lod`], from a second file or a decimator.

use engine_core::math::{Mat4, Quat, Vec2, Vec3, Vec4};
use engine_core::transform::Transform;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::animation::{AnimationClip, BoneTrack, Key};
use crate::error::AssetError;
use crate::material::MaterialAsset;
use crate::mesh::{DEFAULT_VERTEX_COLOR, Geometry, MeshVertex, SkinVertex, SubMesh, UNSET_TANGENT};
use crate::model::ModelAsset;
use crate::skeleton::{Bone, Skeleton};
use crate::texture::{TextureAsset, TextureFormat};

/// Reads a `.glb` or self-contained `.gltf` into a model called `name`.
///
/// The result is **not** validated — call [`crate::validate_model`] on it. Keeping the two
/// separate means an importer bug and a content bug produce different errors.
///
/// # Errors
///
/// * [`AssetError::Malformed`] if the bytes are not a glTF document, or a required accessor
///   is missing.
/// * [`AssetError::Unsupported`] for a feature listed in this module's documentation.
/// * [`AssetError::Invalid`] if the document is self-inconsistent — a skin whose joints are
///   not a connected hierarchy, a texture that will not decode.
pub fn import_glb(name: &str, bytes: &[u8]) -> Result<ModelAsset, AssetError> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|error| AssetError::Malformed {
        model: name.to_owned(),
        reason: error.to_string(),
    })?;
    let buffers = resolve_buffers(name, &gltf)?;

    let textures = import_textures(name, &gltf, &buffers)?;
    let materials = import_materials(&gltf);
    let skin = import_skeleton(name, &gltf, &buffers)?;
    let submeshes = import_submeshes(name, &gltf, &buffers, skin.as_ref())?;

    if submeshes.is_empty() {
        return Err(AssetError::invalid(
            name,
            "meshes",
            "the document contains no drawable primitives",
        ));
    }

    let model = ModelAsset::rigid(name, submeshes, materials, textures);
    Ok(match skin {
        Some(skin) => {
            let clips = import_clips(name, &gltf, &buffers, &skin)?;
            model.with_animation(skin.skeleton, clips)
        }
        None => model,
    })
}

/// One node of a layout export: what it is called, and where it stands.
///
/// The whole of what a layout `.glb` contributes. No geometry, no material and no texture is
/// read out of one — see [`import_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    /// The node's name, exactly as the file spells it: `"pillar.007"`, `"spawn_player"`.
    pub name: String,
    /// Where it stands in the document's space, with every ancestor's transform applied.
    pub transform: Transform,
}

/// Reads a `.glb` as a *layout*: node names and world transforms, and nothing else.
///
/// # Why a layout is not a model
///
/// A level built in Blender out of this game's own assets should not arrive as one enormous
/// mesh. Baking it would throw away the three things the runtime already does well: one
/// upload per distinct asset however many times it is placed, a level of detail chosen per
/// object, and a cull per object. So the exporter's job is to say *where things are*, and the
/// engine keeps saying what they look like — the file's meshes are never read, and a
/// half-gigabyte export costs only its JSON chunk.
///
/// Every named node is returned, including empties and parents. Deciding which names mean
/// something is the caller's job, because this crate does not know what an environment asset
/// is called — see `disk::load_layout`.
///
/// # Errors
///
/// [`AssetError::Malformed`] if the bytes are not a glTF document.
pub fn import_layout(name: &str, bytes: &[u8]) -> Result<Vec<LayoutNode>, AssetError> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|error| AssetError::Malformed {
        model: name.to_owned(),
        reason: error.to_string(),
    })?;
    let parent_of = node_parents(&gltf);
    let worlds = node_worlds(&gltf, &parent_of);

    Ok(gltf
        .nodes()
        .filter_map(|node| {
            let name = node.name()?;
            Some(LayoutNode {
                name: name.to_owned(),
                transform: decompose(worlds[node.index()]),
            })
        })
        .collect())
}

/// A skeleton, plus the mappings the rest of the import needs.
struct ImportedSkin {
    skeleton: Skeleton,
    /// glTF node index of each bone, by bone index.
    joint_nodes: Vec<usize>,
    /// The bone each glTF skin's joints refer to, by skin index and then by the skin's own
    /// joint index — which is exactly what a primitive's `JOINTS_0` stores.
    ///
    /// One skeleton can be split across several skins: an exporter that gives the body and
    /// the clothing a skin each writes two joint lists over one hierarchy, and vertex 7 of
    /// the clothing means "joint 3 *of that skin*". Without this table those indices would
    /// be read against the merged bone list and the garment would be sewn to the wrong bones.
    skin_bones: Vec<Vec<u16>>,
}

impl ImportedSkin {
    /// The bone a glTF node corresponds to, if it is a joint.
    fn bone_of_node(&self, node: usize) -> Option<u16> {
        self.joint_nodes
            .iter()
            .position(|joint| *joint == node)
            .map(|index| index as u16)
    }
}

/// Every buffer's bytes, indexed as glTF indexes them.
fn resolve_buffers(name: &str, gltf: &gltf::Gltf) -> Result<Vec<Vec<u8>>, AssetError> {
    let mut buffers = Vec::new();
    for buffer in gltf.buffers() {
        let bytes = match buffer.source() {
            gltf::buffer::Source::Bin => match gltf.blob.as_ref() {
                Some(blob) => blob.clone(),
                None => {
                    return Err(AssetError::invalid(
                        name,
                        format!("buffer {}", buffer.index()),
                        "names the GLB binary chunk, and the file has none",
                    ));
                }
            },
            gltf::buffer::Source::Uri(uri) => decode_data_uri(name, uri)?,
        };
        buffers.push(bytes);
    }
    Ok(buffers)
}

/// Decodes a `data:...;base64,...` URI.
///
/// Anything else is a sidecar file, and this crate has no filesystem resolver.
fn decode_data_uri(model: &str, uri: &str) -> Result<Vec<u8>, AssetError> {
    let Some(payload) = uri.strip_prefix("data:") else {
        return Err(AssetError::Unsupported {
            model: model.to_owned(),
            feature: format!("external resource '{uri}' (embed it, or export GLB)"),
        });
    };
    let Some((_, encoded)) = payload.split_once(";base64,") else {
        return Err(AssetError::Unsupported {
            model: model.to_owned(),
            feature: "a data URI that is not base64".to_owned(),
        });
    };
    decode_base64(model, encoded)
}

/// A base64 decoder, so the crate does not gain a dependency for one function.
fn decode_base64(model: &str, text: &str) -> Result<Vec<u8>, AssetError> {
    /// Maps a base64 character to its six bits, or `None` for padding and whitespace.
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let Some(value) = sextet(byte) else {
            return Err(AssetError::Malformed {
                model: model.to_owned(),
                reason: format!("'{}' is not base64", byte as char),
            });
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulator >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

/// The bytes a buffer view covers.
fn view_bytes<'a>(
    model: &str,
    buffers: &'a [Vec<u8>],
    view: &gltf::buffer::View<'_>,
) -> Result<&'a [u8], AssetError> {
    let Some(buffer) = buffers.get(view.buffer().index()) else {
        return Err(AssetError::invalid(
            model,
            format!("buffer view {}", view.index()),
            "names a buffer that does not exist",
        ));
    };
    let start = view.offset();
    let end = start + view.length();
    buffer.get(start..end).ok_or_else(|| {
        AssetError::invalid(
            model,
            format!("buffer view {}", view.index()),
            format!("covers {start}..{end} of a {}-byte buffer", buffer.len()),
        )
    })
}

/// Decodes every image the document embeds.
fn import_textures(
    model: &str,
    gltf: &gltf::Gltf,
    buffers: &[Vec<u8>],
) -> Result<Vec<TextureAsset>, AssetError> {
    // Which images are sampled as colour. glTF says nothing on the image itself, so it is
    // read off the materials that reference it: base colour and emissive are sRGB, normal
    // and metallic-roughness are data. Getting this wrong is the classic washed-out import.
    let mut srgb = vec![false; gltf.images().len()];
    for material in gltf.materials() {
        let pbr = material.pbr_metallic_roughness();
        if let Some(info) = pbr.base_color_texture()
            && let Some(slot) = srgb.get_mut(info.texture().source().index())
        {
            *slot = true;
        }
        if let Some(info) = material.emissive_texture()
            && let Some(slot) = srgb.get_mut(info.texture().source().index())
        {
            *slot = true;
        }
    }

    let mut textures = Vec::new();
    for image in gltf.images() {
        let index = image.index();
        let name = image.name().unwrap_or("image").to_owned();
        let (bytes, mime) = match image.source() {
            gltf::image::Source::View { view, mime_type } => (
                view_bytes(model, buffers, &view)?.to_vec(),
                mime_type.to_owned(),
            ),
            gltf::image::Source::Uri { uri, mime_type } => (
                decode_data_uri(model, uri)?,
                mime_type.unwrap_or("image/png").to_owned(),
            ),
        };
        let format = if srgb.get(index).copied().unwrap_or(false) {
            TextureFormat::Rgba8Srgb
        } else {
            TextureFormat::Rgba8Unorm
        };
        // glTF allows exactly two image containers, and an exporter picks between them per
        // image: PNG where alpha matters, JPEG for everything else. Both decode to the same
        // RGBA8 the runtime holds, so the container stops mattering here.
        let texture = if mime.contains("png") {
            decode_png(model, &name, format, &bytes)?
        } else if mime.contains("jpeg") || mime.contains("jpg") {
            decode_jpeg(model, &name, format, &bytes)?
        } else {
            return Err(AssetError::Unsupported {
                model: model.to_owned(),
                feature: format!("image {index} in {mime}; only PNG and JPEG are decoded"),
            });
        };
        textures.push(texture);
    }
    Ok(textures)
}

/// Decodes a PNG into straight RGBA8.
fn decode_png(
    model: &str,
    name: &str,
    format: TextureFormat,
    bytes: &[u8],
) -> Result<TextureAsset, AssetError> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| AssetError::Malformed {
        model: model.to_owned(),
        reason: format!("image '{name}': {error}"),
    })?;
    let mut raw = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut raw)
        .map_err(|error| AssetError::Malformed {
            model: model.to_owned(),
            reason: format!("image '{name}': {error}"),
        })?;
    raw.truncate(info.buffer_size());

    // Expand to RGBA, because the runtime format is one thing and the upload path should not
    // learn about channel counts.
    let texels = match info.color_type {
        png::ColorType::Rgba => raw,
        png::ColorType::Rgb => raw
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        png::ColorType::Grayscale => raw
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => raw
            .chunks_exact(2)
            .flat_map(|pixel| [pixel[0], pixel[0], pixel[0], pixel[1]])
            .collect(),
        other => {
            return Err(AssetError::Unsupported {
                model: model.to_owned(),
                feature: format!("image '{name}' with colour type {other:?}"),
            });
        }
    };
    TextureAsset::new(name, info.width, info.height, format, texels)
}

/// Decodes a JPEG into straight RGBA8.
///
/// JPEG has no alpha, so the fourth channel is opaque. A greyscale file decodes as one
/// channel whatever output colourspace is asked for, which is why the colourspace is read
/// back off the decoder rather than assumed.
fn decode_jpeg(
    model: &str,
    name: &str,
    format: TextureFormat,
    bytes: &[u8],
) -> Result<TextureAsset, AssetError> {
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(bytes, options);
    let raw = decoder.decode().map_err(|error| AssetError::Malformed {
        model: model.to_owned(),
        reason: format!("image '{name}': {error:?}"),
    })?;
    let info = decoder.info().ok_or_else(|| AssetError::Malformed {
        model: model.to_owned(),
        reason: format!("image '{name}': no header after a successful decode"),
    })?;
    let colorspace = decoder.get_output_colorspace();

    let texels = match colorspace.as_ref().map(ColorSpace::num_components) {
        Some(3) => raw
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect(),
        Some(1) => raw
            .iter()
            .flat_map(|value| [*value, *value, *value, 255])
            .collect(),
        Some(4) => raw,
        _ => {
            return Err(AssetError::Unsupported {
                model: model.to_owned(),
                feature: format!("image '{name}' with colourspace {colorspace:?}"),
            });
        }
    };
    TextureAsset::new(
        name,
        u32::from(info.width),
        u32::from(info.height),
        format,
        texels,
    )
}

/// Translates every material, slot for slot.
fn import_materials(gltf: &gltf::Gltf) -> Vec<MaterialAsset> {
    let mut materials: Vec<MaterialAsset> = gltf
        .materials()
        .map(|material| {
            let pbr = material.pbr_metallic_roughness();
            let color = pbr.base_color_factor();
            let emissive = material.emissive_factor();
            MaterialAsset {
                name: material.name().unwrap_or("material").to_owned(),
                base_color: Vec4::new(color[0], color[1], color[2], color[3]),
                roughness: pbr.roughness_factor().clamp(0.0, 1.0),
                metallic: pbr.metallic_factor().clamp(0.0, 1.0),
                emissive: Vec3::new(emissive[0], emissive[1], emissive[2]),
                base_color_texture: pbr
                    .base_color_texture()
                    .map(|info| info.texture().source().index()),
                normal_texture: material
                    .normal_texture()
                    .map(|info| info.texture().source().index()),
                // glTF's `normalTexture.scale`, and its default of 1 when the file omits it
                // — which is also what a material with no normal map at all must carry, so
                // that turning the map on later does not also change its strength.
                //
                // Not clamped above: an artist who exported a scale of 2 meant it, and the
                // shader renormalizes afterwards so the frame stays valid. Negative is
                // refused rather than honoured — it would invert the map's X and Y, which
                // glTF has no way to mean.
                normal_scale: material
                    .normal_texture()
                    .map_or(1.0, |info| info.scale())
                    .max(0.0),
                // glTF's `occlusionTexture.strength`, clamped to its stated [0, 1] range.
                occlusion_strength: material
                    .occlusion_texture()
                    .map_or(1.0, |info| info.strength())
                    .clamp(0.0, 1.0),
                orm_texture: pbr
                    .metallic_roughness_texture()
                    .map(|info| info.texture().source().index()),
                emissive_texture: material
                    .emissive_texture()
                    .map(|info| info.texture().source().index()),
                // glTF's alpha mode, mapped onto the only kind of transparency this
                // renderer has. `Mask` is a cutout and carries its own threshold; `Blend`
                // becomes one too, at glTF's default threshold, because a leaf card with a
                // hard edge is a much closer answer than an opaque rectangle and there is no
                // sorted transparent pass to give it instead. `Opaque` performs no test.
                alpha_cutoff: match material.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => 0.0,
                    gltf::material::AlphaMode::Mask => {
                        material.alpha_cutoff().unwrap_or(0.5).clamp(0.0, 1.0)
                    }
                    gltf::material::AlphaMode::Blend => 0.5,
                },
            }
        })
        .collect();
    // A primitive with no material uses glTF's default, which this represents as one extra
    // slot at the end rather than as an `Option` every consumer would have to handle.
    materials.push(MaterialAsset::default());
    materials
}

/// Builds one skeleton from every skin in the document.
///
/// Returns `None` for a document with no skin, which is a perfectly good rigid prop.
///
/// # Why the bones come from the node hierarchy rather than from the joint's own transform
///
/// A bone's rest transform here is the joint's transform *relative to its nearest joint
/// ancestor*, computed from world matrices. Two things in a real export make the simpler copy
/// wrong: an exporter routinely leaves un-skinned pivots between two joints
/// (`Hip -> BoneRoot -> Pelvis`), and the whole rig usually hangs under a conversion node
/// carrying the unit and axis change — a hundredth-scale, quarter-turn affair. Copying the
/// node's own transform silently drops both, which is a character a hundred times too big
/// lying on its side.
///
/// # Why several skins become one skeleton
///
/// The runtime poses one skeleton per character and the shader reads one bone palette. An
/// export that skins the body and the clothing separately describes one hierarchy twice, so
/// the joints are merged by node and each skin keeps its own index table in
/// [`ImportedSkin::skin_bones`].
fn import_skeleton(
    model: &str,
    gltf: &gltf::Gltf,
    buffers: &[Vec<u8>],
) -> Result<Option<ImportedSkin>, AssetError> {
    if gltf.skins().len() == 0 {
        return Ok(None);
    }

    let parent_of = node_parents(gltf);
    let worlds = node_worlds(gltf, &parent_of);

    // Every joint any skin names, once, ordered so a parent always precedes its children —
    // which is what `Skeleton::new` requires and what makes its one-pass composition valid.
    let mut joint_nodes: Vec<usize> = Vec::new();
    for skin in gltf.skins() {
        for joint in skin.joints() {
            if !joint_nodes.contains(&joint.index()) {
                joint_nodes.push(joint.index());
            }
        }
    }
    joint_nodes.sort_by_key(|node| (ancestor_depth(*node, &parent_of), *node));

    // The file's own inverse binds, by node. Derived only where the file states none: an
    // exporter is entitled to make them differ from the rest pose, and assuming otherwise
    // turns the character inside out.
    let mut inverse_binds: Vec<Option<Mat4>> = vec![None; gltf.nodes().len()];
    for skin in gltf.skins() {
        let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
        let reader = skin.reader(|buffer| buffers.get(buffer.index()).map(|bytes| &bytes[..]));
        // The spec's default is the identity for every joint, which means the bind pose is
        // the model's own space.
        let Some(matrices) = reader.read_inverse_bind_matrices() else {
            continue;
        };
        let matrices: Vec<Mat4> = matrices.map(columns_to_mat4).collect();
        if matrices.len() != joints.len() {
            return Err(AssetError::invalid(
                model,
                format!("skin {}", skin.index()),
                format!(
                    "{} inverse bind matrices for {} joints",
                    matrices.len(),
                    joints.len()
                ),
            ));
        }
        for (node, matrix) in joints.into_iter().zip(matrices) {
            inverse_binds[node] = Some(matrix);
        }
    }

    let bone_of_node = |node: usize| -> Option<u16> {
        joint_nodes
            .iter()
            .position(|joint| *joint == node)
            .map(|index| index as u16)
    };

    let names: Vec<Option<String>> = gltf
        .nodes()
        .map(|node| node.name().map(str::to_owned))
        .collect();

    let mut bones = Vec::with_capacity(joint_nodes.len());
    for node in joint_nodes.iter().copied() {
        // Nearest rather than immediate ancestor: an un-skinned pivot in the middle of a
        // chain is not a break in the skeleton, and treating it as one detaches half the
        // character. Its transform is not lost — it is inside the world matrices below.
        let mut ancestor = parent_of[node];
        let mut parent = None;
        while let Some(candidate) = ancestor {
            if let Some(bone) = bone_of_node(candidate) {
                parent = Some(bone);
                break;
            }
            ancestor = parent_of[candidate];
        }

        let world = worlds[node];
        let rest = match parent {
            Some(parent) => match worlds[joint_nodes[parent as usize]].inverse() {
                Some(inverse) => inverse * world,
                // A zero scale somewhere above this joint. The bone is placed in its
                // parent's space unchanged rather than filled with NaNs.
                None => world,
            },
            None => world,
        };

        bones.push(Bone {
            name: names[node].clone().unwrap_or_else(|| format!("bone{node}")),
            parent,
            rest: decompose(rest),
            inverse_bind: match inverse_binds[node] {
                Some(matrix) => matrix,
                None => world.inverse().unwrap_or(Mat4::IDENTITY),
            },
        });
    }

    let mut skin_bones = Vec::with_capacity(gltf.skins().len());
    for skin in gltf.skins() {
        let mut table = Vec::new();
        for joint in skin.joints() {
            let Some(bone) = bone_of_node(joint.index()) else {
                return Err(AssetError::invalid(
                    model,
                    format!("skin {}", skin.index()),
                    format!("joint node {} is not in the merged skeleton", joint.index()),
                ));
            };
            table.push(bone);
        }
        skin_bones.push(table);
    }

    Ok(Some(ImportedSkin {
        skeleton: Skeleton::new(bones)?,
        joint_nodes,
        skin_bones,
    }))
}

/// Each node's parent, by node index. glTF stores children, not parents.
fn node_parents(gltf: &gltf::Gltf) -> Vec<Option<usize>> {
    let mut parent_of = vec![None; gltf.nodes().len()];
    for node in gltf.nodes() {
        for child in node.children() {
            parent_of[child.index()] = Some(node.index());
        }
    }
    parent_of
}

/// How many ancestors a node has. The sort key that puts parents before children.
fn ancestor_depth(node: usize, parent_of: &[Option<usize>]) -> usize {
    let mut depth = 0;
    let mut current = parent_of[node];
    // Bounded by the node count: a malformed file containing a cycle leaves this loop
    // rather than hanging the import.
    while let Some(parent) = current {
        depth += 1;
        if depth > parent_of.len() {
            break;
        }
        current = parent_of[parent];
    }
    depth
}

/// Each node's transform relative to the document root, by node index.
fn node_worlds(gltf: &gltf::Gltf, parent_of: &[Option<usize>]) -> Vec<Mat4> {
    let locals: Vec<Mat4> = gltf
        .nodes()
        .map(|node| columns_to_mat4(node.transform().matrix()))
        .collect();
    let mut worlds: Vec<Option<Mat4>> = vec![None; locals.len()];
    for index in 0..locals.len() {
        // Up to the nearest ancestor already resolved, then back down: every node is
        // composed exactly once however deep the hierarchy is, and nothing recurses.
        let mut chain = Vec::new();
        let mut current = Some(index);
        while let Some(node) = current {
            if worlds[node].is_some() || chain.len() > locals.len() {
                break;
            }
            chain.push(node);
            current = parent_of[node];
        }
        let mut accumulated = current
            .and_then(|node| worlds[node])
            .unwrap_or(Mat4::IDENTITY);
        for node in chain.into_iter().rev() {
            accumulated = accumulated * locals[node];
            worlds[node] = Some(accumulated);
        }
    }
    worlds
        .into_iter()
        .map(|world| world.unwrap_or(Mat4::IDENTITY))
        .collect()
}

/// Splits an affine matrix into the translation, rotation and scale a [`Transform`] holds.
///
/// Shear is not representable and is dropped: a skewed bone is not something a rig should
/// contain, and the alternative is carrying a matrix per bone through the whole animation
/// system to preserve something no exporter intends.
fn decompose(matrix: Mat4) -> Transform {
    let values = matrix.to_cols_array();
    let axis = |column: usize| {
        Vec3::new(
            values[column * 4],
            values[column * 4 + 1],
            values[column * 4 + 2],
        )
    };
    let (x, y, z) = (axis(0), axis(1), axis(2));
    let mut scale = Vec3::new(x.length(), y.length(), z.length());
    // A negative determinant is a mirrored basis, which no quaternion expresses. The flip
    // goes on X, the convention every other engine uses, so a mirrored rig at least mirrors
    // consistently instead of rotating unpredictably.
    if matrix.determinant() < 0.0 {
        scale.x = -scale.x;
    }
    let unit = |vector: Vec3, length: f32| {
        if length.abs() > 1e-8 {
            vector * (1.0 / length)
        } else {
            Vec3::ZERO
        }
    };
    let rotation = quat_from_basis(unit(x, scale.x), unit(y, scale.y), unit(z, scale.z));
    Transform::new(
        Vec3::new(values[12], values[13], values[14]),
        rotation,
        scale,
    )
}

/// The rotation an orthonormal basis describes, as its three column vectors.
fn quat_from_basis(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    let trace = x.x + y.y + z.z;
    // Four branches rather than one formula: the short form divides by a component that can
    // be zero, and picking the largest keeps the division well conditioned.
    let quat = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat::new((y.z - z.y) / s, (z.x - x.z) / s, (x.y - y.x) / s, 0.25 * s)
    } else if x.x > y.y && x.x > z.z {
        let s = (1.0 + x.x - y.y - z.z).sqrt() * 2.0;
        Quat::new(0.25 * s, (y.x + x.y) / s, (z.x + x.z) / s, (y.z - z.y) / s)
    } else if y.y > z.z {
        let s = (1.0 + y.y - x.x - z.z).sqrt() * 2.0;
        Quat::new((y.x + x.y) / s, 0.25 * s, (z.y + y.z) / s, (z.x - x.z) / s)
    } else {
        let s = (1.0 + z.z - x.x - y.y).sqrt() * 2.0;
        Quat::new((z.x + x.z) / s, (z.y + y.z) / s, 0.25 * s, (x.y - y.x) / s)
    };
    quat.normalize()
}

/// glTF's column-major `[[f32; 4]; 4]` into a [`Mat4`].
fn columns_to_mat4(columns: [[f32; 4]; 4]) -> Mat4 {
    let column = |values: [f32; 4]| Vec4::new(values[0], values[1], values[2], values[3]);
    Mat4::from_columns(
        column(columns[0]),
        column(columns[1]),
        column(columns[2]),
        column(columns[3]),
    )
}

/// Every drawable primitive in the document, as submeshes.
fn import_submeshes(
    model: &str,
    gltf: &gltf::Gltf,
    buffers: &[Vec<u8>],
    skin: Option<&ImportedSkin>,
) -> Result<Vec<SubMesh>, AssetError> {
    let default_material = gltf.materials().len();
    let mut submeshes = Vec::new();

    for node in gltf.nodes() {
        let Some(mesh) = node.mesh() else {
            continue;
        };
        // A skinned primitive's vertices are already in the skin's space and the node
        // transform must be ignored — that is the spec, and applying it anyway is how an
        // imported character ends up doubled in size.
        let node_matrix = if node.skin().is_some() {
            None
        } else {
            Some(columns_to_mat4(node.transform().matrix()))
        };

        for (index, primitive) in mesh.primitives().enumerate() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                return Err(AssetError::Unsupported {
                    model: model.to_owned(),
                    feature: format!("primitive mode {:?}", primitive.mode()),
                });
            }
            let name = format!("{}#{index}", mesh.name().unwrap_or("mesh"));
            let reader =
                primitive.reader(|buffer| buffers.get(buffer.index()).map(|bytes| &bytes[..]));

            let Some(positions) = reader.read_positions() else {
                return Err(AssetError::Malformed {
                    model: model.to_owned(),
                    reason: format!("primitive '{name}' has no POSITION accessor"),
                });
            };
            let positions: Vec<Vec3> = positions
                .map(|value| Vec3::new(value[0], value[1], value[2]))
                .collect();

            let normals: Vec<Vec3> = match reader.read_normals() {
                Some(normals) => normals
                    .map(|value| Vec3::new(value[0], value[1], value[2]))
                    .collect(),
                // Flat +Y rather than a refusal: a normal-less mesh is unusual but drawable,
                // and it is obvious on screen that it needs re-exporting.
                None => vec![Vec3::Y; positions.len()],
            };
            let uvs: Vec<Vec2> = match reader.read_tex_coords(0) {
                Some(coords) => coords
                    .into_f32()
                    .map(|value| Vec2::new(value[0], value[1]))
                    .collect(),
                None => vec![Vec2::ZERO; positions.len()],
            };

            // TANGENT is optional in glTF and most exporters only write it when a normal map
            // is assigned. What arrives here is one of three things: a full set, nothing at
            // all, or a set with holes in it where the exporter could not solve a vertex.
            //
            // The rule, and it matches this crate's validation philosophy of *repair what is
            // unambiguous and refuse what is not*: a tangent set that is short, or that
            // contains a non-finite or zero-length entry, is not trusted at all — the whole
            // primitive falls back to generation. It is not refused, because a mesh with bad
            // tangents is still a perfectly good mesh whose tangents this crate can compute;
            // refusing it would reject assets over a derived attribute. It is not partially
            // repaired either, because mixing exporter tangents with generated ones across
            // one surface produces a seam that is far harder to diagnose than either.
            let source_tangents: Option<Vec<Vec4>> = reader.read_tangents().and_then(|tangents| {
                let values: Vec<Vec4> = tangents
                    .map(|value| Vec4::new(value[0], value[1], value[2], value[3]))
                    .collect();
                if values.len() != positions.len() {
                    log::warn!(
                        "{model}: primitive '{name}' has {} tangents for {} vertices; \
                         generating instead",
                        values.len(),
                        positions.len()
                    );
                    return None;
                }
                let usable = values.iter().all(|tangent| {
                    tangent.x.is_finite()
                        && tangent.y.is_finite()
                        && tangent.z.is_finite()
                        && tangent.w.is_finite()
                        && tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z
                            > 1e-12
                        && (tangent.w.abs() - 1.0).abs() < 1e-3
                });
                if !usable {
                    log::warn!(
                        "{model}: primitive '{name}' has malformed TANGENT data; generating \
                         instead"
                    );
                    return None;
                }
                Some(values)
            });

            // COLOR_0 may be RGB or RGBA, and 8-bit, 16-bit or float. `into_rgba_u8` is the
            // glTF crate's own conversion and supplies an opaque alpha for the RGB forms,
            // which is exactly the engine's convention — so alpha survives when it was
            // authored and is 255 when it was not.
            let colors: Vec<[u8; 4]> = match reader.read_colors(0) {
                Some(colors) => {
                    let values: Vec<[u8; 4]> = colors.into_rgba_u8().collect();
                    if values.len() == positions.len() {
                        values
                    } else {
                        log::warn!(
                            "{model}: primitive '{name}' has {} vertex colours for {} \
                             vertices; using white",
                            values.len(),
                            positions.len()
                        );
                        vec![DEFAULT_VERTEX_COLOR; positions.len()]
                    }
                }
                None => vec![DEFAULT_VERTEX_COLOR; positions.len()],
            };

            let indices: Vec<u32> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };

            let material = primitive.material().index().unwrap_or(default_material);

            let geometry = match (
                skin,
                node.skin(),
                reader.read_joints(0),
                reader.read_weights(0),
            ) {
                (Some(skin), Some(node_skin), Some(joints), Some(weights)) => {
                    if primitive.get(&gltf::Semantic::Joints(1)).is_some() {
                        return Err(AssetError::Unsupported {
                            model: model.to_owned(),
                            feature: format!(
                                "primitive '{name}' with more than four influences per vertex"
                            ),
                        });
                    }
                    let joints: Vec<[u16; 4]> = joints.into_u16().collect();
                    let weights: Vec<[f32; 4]> = weights.into_f32().collect();
                    if joints.len() != positions.len() || weights.len() != positions.len() {
                        return Err(AssetError::invalid(
                            model,
                            &name,
                            "JOINTS_0 and WEIGHTS_0 must cover every vertex",
                        ));
                    }
                    // `JOINTS_0` indexes *this primitive's skin*, not the merged skeleton.
                    // Reading it as a bone index directly is how a garment ends up sewn to
                    // the wrong bones when an export carries more than one skin.
                    let Some(bones_of_joint) = skin.skin_bones.get(node_skin.index()) else {
                        return Err(AssetError::invalid(
                            model,
                            &name,
                            format!("names skin {}, which does not exist", node_skin.index()),
                        ));
                    };
                    let mut vertices = Vec::with_capacity(positions.len());
                    for at in 0..positions.len() {
                        let mut bones = [0u16; 4];
                        for (slot, joint) in joints[at].iter().enumerate() {
                            let Some(bone) = bones_of_joint.get(*joint as usize) else {
                                return Err(AssetError::invalid(
                                    model,
                                    &name,
                                    format!(
                                        "vertex {at} is bound to joint {joint}, and its skin \
                                         has {}",
                                        bones_of_joint.len()
                                    ),
                                ));
                            };
                            bones[slot] = *bone;
                        }
                        vertices.push(
                            SkinVertex {
                                position: positions[at],
                                normal: normals[at],
                                uv: uvs[at],
                                // `UNSET_TANGENT` when the source had none, which is what
                                // makes `SubMesh::new` generate a set for this primitive.
                                tangent: source_tangents
                                    .as_ref()
                                    .map_or(UNSET_TANGENT, |tangents| tangents[at]),
                                color: colors[at],
                                joints: bones,
                                weights: weights[at],
                            }
                            // Exporters routinely emit weights summing to 0.9999.
                            .normalized(),
                        );
                    }
                    Geometry::Skinned(vertices)
                }
                _ => {
                    let matrix = node_matrix.unwrap_or(Mat4::IDENTITY);
                    let vertices = (0..positions.len())
                        .map(|at| {
                            let vertex = MeshVertex::new(
                                matrix.transform_point(positions[at]),
                                matrix.transform_vector(normals[at]).normalize(),
                                uvs[at],
                            )
                            .with_color(colors[at]);
                            match source_tangents.as_ref() {
                                // The node transform applies to the tangent's *direction*
                                // exactly as it does to the normal. Handedness is a property
                                // of the UV layout, not of where the node sits, so W is
                                // carried across unchanged — a mirrored node transform would
                                // need it flipped, and this importer already refuses to bake
                                // one of those into static geometry elsewhere.
                                Some(tangents) => {
                                    let source = tangents[at];
                                    let direction = matrix
                                        .transform_vector(Vec3::new(source.x, source.y, source.z))
                                        .normalize();
                                    vertex.with_tangent(Vec4::new(
                                        direction.x,
                                        direction.y,
                                        direction.z,
                                        source.w,
                                    ))
                                }
                                None => vertex,
                            }
                        })
                        .collect();
                    Geometry::Static(vertices)
                }
            };

            submeshes.push(SubMesh::new(name, geometry, indices, material));
        }
    }
    Ok(submeshes)
}

/// Every animation, bound to the imported skeleton's bone indices.
fn import_clips(
    model: &str,
    gltf: &gltf::Gltf,
    buffers: &[Vec<u8>],
    skin: &ImportedSkin,
) -> Result<Vec<AnimationClip>, AssetError> {
    let mut clips = Vec::new();
    for (index, animation) in gltf.animations().enumerate() {
        let name = animation
            .name()
            .map_or_else(|| format!("clip{index}"), str::to_owned);
        let mut tracks: Vec<BoneTrack> = Vec::new();

        for channel in animation.channels() {
            if channel.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline {
                return Err(AssetError::Unsupported {
                    model: model.to_owned(),
                    feature: format!("cubic spline interpolation in '{name}'"),
                });
            }
            let Some(bone) = skin.bone_of_node(channel.target().node().index()) else {
                // A channel animating something that is not a joint — a prop, a camera.
                // Skipped rather than refused: it is not this skeleton's business.
                continue;
            };
            let reader =
                channel.reader(|buffer| buffers.get(buffer.index()).map(|bytes| &bytes[..]));
            let Some(times) = reader.read_inputs() else {
                return Err(AssetError::Malformed {
                    model: model.to_owned(),
                    reason: format!("a channel of '{name}' has no time accessor"),
                });
            };
            let times: Vec<f32> = times.collect();

            let track = match tracks.iter().position(|track| track.bone == bone) {
                Some(existing) => &mut tracks[existing],
                None => {
                    tracks.push(BoneTrack::new(bone));
                    let last = tracks.len() - 1;
                    &mut tracks[last]
                }
            };

            match reader.read_outputs() {
                Some(gltf::animation::util::ReadOutputs::Translations(values)) => {
                    track.translation = times
                        .iter()
                        .zip(values)
                        .map(|(time, value)| {
                            Key::new(*time, Vec3::new(value[0], value[1], value[2]))
                        })
                        .collect();
                }
                Some(gltf::animation::util::ReadOutputs::Rotations(values)) => {
                    track.rotation = times
                        .iter()
                        .zip(values.into_f32())
                        .map(|(time, value)| {
                            Key::new(
                                *time,
                                Quat::new(value[0], value[1], value[2], value[3]).normalize(),
                            )
                        })
                        .collect();
                }
                Some(gltf::animation::util::ReadOutputs::Scales(values)) => {
                    track.scale = times
                        .iter()
                        .zip(values)
                        .map(|(time, value)| {
                            Key::new(*time, Vec3::new(value[0], value[1], value[2]))
                        })
                        .collect();
                }
                // Morph target weights: ignored, like the morph targets themselves.
                Some(gltf::animation::util::ReadOutputs::MorphTargetWeights(_)) | None => {}
            }
        }

        let tracks: Vec<BoneTrack> = tracks
            .into_iter()
            .filter(|track| !track.is_empty())
            .map(BoneTrack::sorted)
            .collect();
        // glTF says nothing about looping, so every imported clip loops and the character
        // definition's slot decides otherwise. `AnimationSlot::loops` is where that lives.
        clips.push(AnimationClip::new(name, true, tracks));
    }
    Ok(clips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_that_are_not_gltf_are_reported_as_malformed_with_the_model_named() {
        let Err(error) = import_glb("fire_mage", b"this is not a GLB") else {
            panic!("arbitrary bytes are not a model");
        };
        assert!(error.to_string().contains("fire_mage"), "{error}");
        assert!(matches!(error, AssetError::Malformed { .. }), "{error:?}");
    }

    #[test]
    fn an_empty_input_is_reported_rather_than_producing_an_empty_model() {
        assert!(import_glb("empty", &[]).is_err());
    }

    #[test]
    fn base64_round_trips_the_bytes_a_data_uri_carries() {
        // The decoder is hand-written to avoid a dependency, so it gets its own test.
        let Ok(decoded) = decode_base64("test", "SGVsbG8sIHdvcmxkIQ==") else {
            panic!("valid base64 must decode");
        };
        assert_eq!(decoded, b"Hello, world!");

        let Ok(unpadded) = decode_base64("test", "TWE=") else {
            panic!("padded base64 must decode");
        };
        assert_eq!(unpadded, b"Ma");
    }

    #[test]
    fn a_character_outside_the_alphabet_is_reported() {
        assert!(decode_base64("test", "not base64 !!").is_err());
    }

    #[test]
    fn a_sidecar_file_is_refused_with_an_instruction_rather_than_a_silent_hole() {
        let Err(error) = decode_data_uri("mage", "buffers/mage.bin") else {
            panic!("this crate has no filesystem resolver");
        };
        let text = error.to_string();
        assert!(text.contains("GLB"), "{text}");
    }

    /// A GLB carrying `json` and no binary chunk.
    fn glb(json: &str) -> Vec<u8> {
        let mut chunk = json.as_bytes().to_vec();
        while chunk.len() % 4 != 0 {
            chunk.push(b' ');
        }
        let mut bytes = Vec::with_capacity(chunk.len() + 20);
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(20 + chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"JSON");
        bytes.extend_from_slice(&chunk);
        bytes
    }

    #[test]
    fn every_material_texture_slot_gltf_defines_reaches_the_asset() {
        // The regression this exists for: the importer used to read the base colour map and
        // drop `normalTexture` and `metallicRoughnessTexture` on the floor, and there was no
        // emissive slot at all. A model exported with four maps then rendered with one, with
        // nothing anywhere saying three had been discarded.
        //
        // Four distinct image indices, so a slot that reads the wrong one cannot pass by
        // coincidence. The images are never decoded — this exercises `import_materials`,
        // which reads indices — so their URIs are placeholders.
        let document = r#"{
            "asset": { "version": "2.0" },
            "images": [
                { "uri": "base.png" },
                { "uri": "normal.png" },
                { "uri": "orm.png" },
                { "uri": "emissive.png" }
            ],
            "textures": [
                { "source": 0 }, { "source": 1 }, { "source": 2 }, { "source": 3 }
            ],
            "materials": [{
                "name": "four_maps",
                "pbrMetallicRoughness": {
                    "baseColorTexture": { "index": 0 },
                    "metallicRoughnessTexture": { "index": 2 },
                    "roughnessFactor": 0.25,
                    "metallicFactor": 0.75
                },
                "normalTexture": { "index": 1 },
                "emissiveTexture": { "index": 3 },
                "emissiveFactor": [1.0, 0.5, 0.25]
            }]
        }"#;
        let Ok(gltf) = gltf::Gltf::from_slice(&glb(document)) else {
            panic!("the document is valid glTF");
        };
        let materials = import_materials(&gltf);

        let material = &materials[0];
        assert_eq!(material.name, "four_maps");
        assert_eq!(
            material.base_color_texture,
            Some(0),
            "base colour is image 0"
        );
        assert_eq!(material.normal_texture, Some(1), "normal is image 1");
        assert_eq!(material.orm_texture, Some(2), "ORM is image 2");
        assert_eq!(material.emissive_texture, Some(3), "emissive is image 3");

        // The constants come across too. They are not replaced by the maps — glTF's
        // convention is that the metallic-roughness texture *scales* these factors, which is
        // why the renderer's neutral ORM fallback is white rather than a baked default.
        assert_eq!(material.roughness, 0.25);
        assert_eq!(material.metallic, 0.75);
        assert_eq!(material.emissive.x, 1.0);

        // And every slot is reported for validation, so none of them can point past the end
        // of the texture array unnoticed.
        let named: Vec<_> = material.texture_indices().collect();
        assert_eq!(
            named,
            vec![
                ("base_color", 0),
                ("normal", 1),
                ("orm", 2),
                ("emissive", 3),
            ],
            "the slots are reported in the renderer's descriptor order"
        );
    }

    #[test]
    fn the_two_texture_parameters_are_imported_and_default_to_one_when_absent() {
        // glTF puts `scale` on the normal texture reference and `strength` on the occlusion
        // one, and both default to 1 - "apply the map as authored". A material that imported
        // them as zero would silently switch off a map it does have, and the symptom would be
        // flat lighting on exactly the assets that bothered to author a normal map.
        let document = r#"{
            "asset": { "version": "2.0" },
            "images": [{ "uri": "normal.png" }, { "uri": "occlusion.png" }],
            "textures": [{ "source": 0 }, { "source": 1 }],
            "materials": [
                {
                    "name": "explicit",
                    "normalTexture": { "index": 0, "scale": 2.5 },
                    "occlusionTexture": { "index": 1, "strength": 0.25 }
                },
                {
                    "name": "defaulted",
                    "normalTexture": { "index": 0 },
                    "occlusionTexture": { "index": 1 }
                },
                { "name": "no_maps_at_all" }
            ]
        }"#;
        let Ok(gltf) = gltf::Gltf::from_slice(&glb(document)) else {
            panic!("the document is valid glTF");
        };
        let materials = import_materials(&gltf);

        assert_eq!(
            materials[0].normal_scale, 2.5,
            "an explicit scale is carried"
        );
        assert_eq!(
            materials[0].occlusion_strength, 0.25,
            "an explicit strength is carried"
        );

        assert_eq!(
            materials[1].normal_scale, 1.0,
            "a normal texture with no scale means glTF's default of 1"
        );
        assert_eq!(
            materials[1].occlusion_strength, 1.0,
            "an occlusion texture with no strength means glTF's default of 1"
        );

        // And a material with neither texture still carries 1, not 0: the values must not
        // become "off" merely because the maps they scale are absent, or adding a map later
        // would arrive with its strength already turned down.
        assert_eq!(materials[2].normal_scale, 1.0);
        assert_eq!(materials[2].occlusion_strength, 1.0);
    }

    #[test]
    fn out_of_range_texture_parameters_are_brought_back_into_range() {
        // glTF states occlusion strength is in [0, 1] and puts no ceiling on the normal
        // scale. A negative scale would invert the map's X and Y, which glTF has no way to
        // mean, so it is clamped away rather than honoured; a large positive one is an artist
        // exaggerating a map and is kept.
        let document = r#"{
            "asset": { "version": "2.0" },
            "images": [{ "uri": "normal.png" }],
            "textures": [{ "source": 0 }],
            "materials": [{
                "normalTexture": { "index": 0, "scale": -3.0 },
                "occlusionTexture": { "index": 0, "strength": 4.0 }
            }]
        }"#;
        let Ok(gltf) = gltf::Gltf::from_slice(&glb(document)) else {
            panic!("the document is valid glTF");
        };
        let materials = import_materials(&gltf);
        assert_eq!(
            materials[0].normal_scale, 0.0,
            "a negative scale is refused"
        );
        assert_eq!(
            materials[0].occlusion_strength, 1.0,
            "a strength above one is clamped to glTF's range"
        );
    }

    #[test]
    fn an_image_a_material_samples_as_colour_is_marked_srgb_and_data_maps_are_not() {
        // The classification the renderer's view format is built from. Base colour and
        // emissive are colour and are linearized on read; normal and ORM are data and must
        // not be. Getting it backwards is the classic washed-out or muddy import, and it has
        // no other symptom.
        let document = r#"{
            "asset": { "version": "2.0" },
            "images": [
                { "uri": "base.png" },
                { "uri": "normal.png" },
                { "uri": "orm.png" },
                { "uri": "emissive.png" }
            ],
            "textures": [
                { "source": 0 }, { "source": 1 }, { "source": 2 }, { "source": 3 }
            ],
            "materials": [{
                "pbrMetallicRoughness": {
                    "baseColorTexture": { "index": 0 },
                    "metallicRoughnessTexture": { "index": 2 }
                },
                "normalTexture": { "index": 1 },
                "emissiveTexture": { "index": 3 }
            }]
        }"#;
        let Ok(gltf) = gltf::Gltf::from_slice(&glb(document)) else {
            panic!("the document is valid glTF");
        };

        // The same walk `import_textures` performs, isolated from the image decoding it
        // otherwise does — the decode needs real PNG bytes and the classification does not.
        let mut srgb = vec![false; gltf.images().len()];
        for material in gltf.materials() {
            let pbr = material.pbr_metallic_roughness();
            if let Some(info) = pbr.base_color_texture()
                && let Some(slot) = srgb.get_mut(info.texture().source().index())
            {
                *slot = true;
            }
            if let Some(info) = material.emissive_texture()
                && let Some(slot) = srgb.get_mut(info.texture().source().index())
            {
                *slot = true;
            }
        }
        assert_eq!(
            srgb,
            vec![true, false, false, true],
            "base colour and emissive are sRGB; normal and ORM are data"
        );
    }

    #[test]
    fn a_layout_reports_every_named_node_where_its_ancestors_put_it() {
        // A parent that scales by two and turns a quarter turn about +Y, and a child ten
        // metres down +X. The child's world position is therefore twenty metres down −Z.
        // Reading the child's own transform instead would put it in the wrong room, which is
        // the whole reason a layout is composed rather than copied.
        let document = r#"{
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "scene": 0,
            "nodes": [
                {
                    "name": "Room",
                    "children": [1, 2],
                    "scale": [2.0, 2.0, 2.0],
                    "rotation": [0.0, 0.70710678, 0.0, 0.70710678]
                },
                {"name": "pillar.001", "translation": [10.0, 0.0, 0.0]},
                {"name": "spawn_player", "translation": [0.0, 0.0, 1.0]}
            ]
        }"#;
        let Ok(nodes) = import_layout("layout", &glb(document)) else {
            panic!("a layout document parses");
        };
        assert_eq!(nodes.len(), 3, "every named node is reported");

        let Some(pillar) = nodes.iter().find(|node| node.name == "pillar.001") else {
            panic!("the pillar is in the layout");
        };
        let position = pillar.transform.translation;
        assert!(
            (position - Vec3::new(0.0, 0.0, -20.0)).length() < 1e-3,
            "the parent's scale and rotation were dropped: {position:?}"
        );
        assert!(
            (pillar.transform.scale - Vec3::splat(2.0)).length() < 1e-3,
            "the parent's scale is the pillar's size"
        );
    }

    #[test]
    fn a_layout_reads_no_geometry_so_a_document_with_none_is_still_a_layout() {
        let document = r#"{
            "asset": {"version": "2.0"},
            "nodes": [{"name": "wall_segment", "translation": [1.0, 2.0, 3.0]}]
        }"#;
        let Ok(nodes) = import_layout("layout", &glb(document)) else {
            panic!("meshes are not required");
        };
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].transform.translation, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn a_gltf_column_array_becomes_the_matrix_it_describes() {
        // glTF is column-major and so is Mat4, so this must be a copy and not a transpose.
        // A transpose "for safety" would make every imported skeleton wrong.
        let columns = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [4.0, 5.0, 6.0, 1.0],
        ];
        let matrix = columns_to_mat4(columns);
        assert_eq!(matrix.transform_point(Vec3::ZERO), Vec3::new(4.0, 5.0, 6.0));
    }

    #[test]
    fn a_png_decodes_to_straight_rgba_whatever_channels_it_had() {
        // A 1x1 greyscale PNG, which exercises the expansion path.
        let png = build_grey_png();
        let Ok(texture) = decode_png("test", "grey", TextureFormat::Rgba8Unorm, &png) else {
            panic!("a valid PNG must decode");
        };
        assert_eq!((texture.width, texture.height), (1, 1));
        assert_eq!(texture.texels.len(), 4);
        assert_eq!(texture.texels[3], 255, "alpha must be filled in");
        assert_eq!(texture.texels[0], texture.texels[1]);
    }

    #[test]
    fn bytes_that_are_not_a_png_are_reported_with_the_image_named() {
        let Err(error) = decode_png("test", "broken", TextureFormat::Rgba8Unorm, b"not a png")
        else {
            panic!("arbitrary bytes are not an image");
        };
        assert!(error.to_string().contains("broken"), "{error}");
    }

    /// A minimal 1×1 8-bit greyscale PNG, encoded here so the test needs no fixture file.
    fn build_grey_png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 1, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let Ok(mut writer) = encoder.write_header() else {
                panic!("the header must write");
            };
            let Ok(()) = writer.write_image_data(&[128]) else {
                panic!("one texel must write");
            };
        }
        out
    }
}
