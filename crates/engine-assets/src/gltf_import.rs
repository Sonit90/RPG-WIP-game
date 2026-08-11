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

use crate::animation::{AnimationClip, BoneTrack, Key};
use crate::error::AssetError;
use crate::material::MaterialAsset;
use crate::mesh::{Geometry, MeshVertex, SkinVertex, SubMesh};
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

/// A skin, plus the mapping the rest of the import needs.
struct ImportedSkin {
    skeleton: Skeleton,
    /// glTF node index of each joint, by bone index.
    joint_nodes: Vec<usize>,
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
        if !mime.contains("png") {
            return Err(AssetError::Unsupported {
                model: model.to_owned(),
                feature: format!("image {index} in {mime}; only PNG is decoded"),
            });
        }
        let format = if srgb.get(index).copied().unwrap_or(false) {
            TextureFormat::Rgba8Srgb
        } else {
            TextureFormat::Rgba8Unorm
        };
        textures.push(decode_png(model, &name, format, &bytes)?);
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
                orm_texture: pbr
                    .metallic_roughness_texture()
                    .map(|info| info.texture().source().index()),
            }
        })
        .collect();
    // A primitive with no material uses glTF's default, which this represents as one extra
    // slot at the end rather than as an `Option` every consumer would have to handle.
    materials.push(MaterialAsset::default());
    materials
}

/// Builds a skeleton from the document's first skin.
///
/// Returns `None` for a document with no skin, which is a perfectly good rigid prop.
fn import_skeleton(
    model: &str,
    gltf: &gltf::Gltf,
    buffers: &[Vec<u8>],
) -> Result<Option<ImportedSkin>, AssetError> {
    let Some(skin) = gltf.skins().next() else {
        return Ok(None);
    };
    if gltf.skins().len() > 1 {
        return Err(AssetError::Unsupported {
            model: model.to_owned(),
            feature: format!("{} skins; a character has one", gltf.skins().len()),
        });
    }

    let joints: Vec<gltf::Node<'_>> = skin.joints().collect();
    let joint_nodes: Vec<usize> = joints.iter().map(gltf::Node::index).collect();

    let reader = skin.reader(|buffer| buffers.get(buffer.index()).map(|bytes| &bytes[..]));
    let inverse_binds: Vec<Mat4> = match reader.read_inverse_bind_matrices() {
        Some(matrices) => matrices.map(columns_to_mat4).collect(),
        // The spec's default is the identity for every joint, which means the bind pose is
        // the model's own space.
        None => vec![Mat4::IDENTITY; joints.len()],
    };
    if inverse_binds.len() != joints.len() {
        return Err(AssetError::invalid(
            model,
            "skin",
            format!(
                "{} inverse bind matrices for {} joints",
                inverse_binds.len(),
                joints.len()
            ),
        ));
    }

    // Parent of each node, from the hierarchy. glTF stores children, not parents.
    let mut parent_of = vec![None; gltf.nodes().len()];
    for node in gltf.nodes() {
        for child in node.children() {
            parent_of[child.index()] = Some(node.index());
        }
    }

    // A joint's parent bone is its nearest ancestor that is also a joint. Nearest rather
    // than immediate, because exporters routinely leave an un-skinned pivot node in the
    // middle of a chain, and treating that as "no parent" detaches half the character.
    let mut bones = Vec::with_capacity(joints.len());
    for (index, node) in joints.iter().enumerate() {
        let mut ancestor = parent_of[node.index()];
        let mut parent = None;
        while let Some(candidate) = ancestor {
            if let Some(bone) = joint_nodes.iter().position(|joint| *joint == candidate) {
                parent = Some(bone as u16);
                break;
            }
            ancestor = parent_of[candidate];
        }
        if let Some(parent) = parent
            && parent as usize >= index
        {
            return Err(AssetError::invalid(
                model,
                format!("joint {index}"),
                "the skin lists a joint before its parent; re-export with a sorted hierarchy",
            ));
        }

        let (translation, rotation, scale) = node.transform().decomposed();
        bones.push(Bone {
            name: node.name().unwrap_or("bone").to_owned(),
            parent,
            rest: Transform::new(
                Vec3::new(translation[0], translation[1], translation[2]),
                Quat::new(rotation[0], rotation[1], rotation[2], rotation[3]),
                Vec3::new(scale[0], scale[1], scale[2]),
            ),
            // The file's own inverse binds, never derived from the rest pose: an exporter is
            // entitled to make them differ, and assuming otherwise turns the character
            // inside out.
            inverse_bind: inverse_binds[index],
        });
    }

    Ok(Some(ImportedSkin {
        skeleton: Skeleton::new(bones)?,
        joint_nodes,
    }))
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

            let indices: Vec<u32> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };

            let material = primitive.material().index().unwrap_or(default_material);

            let geometry = match (skin, reader.read_joints(0), reader.read_weights(0)) {
                (Some(skin), Some(joints), Some(weights)) => {
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
                    let bones = skin.skeleton.len();
                    let mut vertices = Vec::with_capacity(positions.len());
                    for at in 0..positions.len() {
                        if let Some(joint) =
                            joints[at].iter().find(|joint| **joint as usize >= bones)
                        {
                            return Err(AssetError::invalid(
                                model,
                                &name,
                                format!(
                                    "vertex {at} is bound to joint {joint}, and the skin has \
                                     {bones}"
                                ),
                            ));
                        }
                        vertices.push(
                            SkinVertex {
                                position: positions[at],
                                normal: normals[at],
                                uv: uvs[at],
                                joints: joints[at],
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
                            MeshVertex::new(
                                matrix.transform_point(positions[at]),
                                matrix.transform_vector(normals[at]).normalize(),
                                uvs[at],
                            )
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
