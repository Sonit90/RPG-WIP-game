//! Shader libraries: one `MTLLibrary` per `shaders/metal/*.metal` file, loaded on first use.
//!
//! `build.rs` hands over two lists. `METAL_LIBRARIES` holds precompiled `.metallib` bytes when
//! the build machine had Apple's Metal toolchain, and a library is then created from them with
//! no compilation at all. `METAL_SOURCES` always holds the expanded source, which is compiled
//! here when no `.metallib` exists. Loading is lazy so a feature that is never drawn (the
//! shadow and contact passes, until they exist) costs nothing.

use std::collections::HashMap;
use std::ptr::NonNull;

use dispatch2::DispatchData;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLCompileOptions, MTLDataType, MTLDevice, MTLFunction, MTLFunctionConstantValues,
    MTLLanguageVersion, MTLLibrary,
};

use super::{Device, api_error, describe};
use crate::error::RendererError;

include!(concat!(env!("OUT_DIR"), "/metal_sources.rs"));

/// A shader function.
pub(crate) type Function = Retained<ProtocolObject<dyn MTLFunction>>;

type Library = Retained<ProtocolObject<dyn MTLLibrary>>;

/// The shader libraries loaded so far.
pub(crate) struct Libraries {
    device: Device,
    loaded: HashMap<&'static str, Library>,
}

impl Libraries {
    /// An empty set; nothing is loaded until a function is asked for.
    pub(crate) fn new(device: &Device) -> Self {
        Self {
            device: device.clone(),
            loaded: HashMap::new(),
        }
    }

    /// The function `entry` of the shader file `file` (its stem, e.g. `"mesh"`).
    ///
    /// # Errors
    ///
    /// [`RendererError::ShaderCompilation`] if the file does not compile or is unknown, or
    /// [`RendererError::Api`] if the library has no such function.
    pub(crate) fn function(
        &mut self,
        file: &'static str,
        entry: &str,
    ) -> Result<Function, RendererError> {
        let library = self.library(file)?;
        library
            .newFunctionWithName(&NSString::from_str(entry))
            .ok_or_else(|| missing(file, entry))
    }

    /// The fragment function `entry` of `file`, specialized by the three mesh function
    /// constants (`common.h`: 0 `MATERIAL_PBR`, 1 `MATERIAL_NORMAL_MAP`, 2 `ENABLE_SHADOWS`).
    /// All three must be supplied, whether or not the variant uses them.
    ///
    /// # Errors
    ///
    /// As [`Libraries::function`].
    pub(crate) fn mesh_fragment(
        &mut self,
        file: &'static str,
        entry: &str,
        pbr: bool,
        normal_map: bool,
        shadows: bool,
    ) -> Result<Function, RendererError> {
        let constants = MTLFunctionConstantValues::new();
        for (index, value) in [pbr, normal_map, shadows].into_iter().enumerate() {
            // SAFETY: the pointer addresses a live `bool`, which is what `MTLDataType::Bool`
            // reads; Metal copies the value before the call returns.
            unsafe {
                constants.setConstantValue_type_atIndex(
                    NonNull::from(&value).cast::<std::ffi::c_void>(),
                    MTLDataType::Bool,
                    index,
                );
            }
        }
        let library = self.library(file)?;
        library
            .newFunctionWithName_constantValues_error(&NSString::from_str(entry), &constants)
            .map_err(|error| RendererError::ShaderCompilation {
                name: format!("{file}.metal: {entry}"),
                messages: describe(&error),
            })
    }

    /// The library for `file`, loading it on the first request.
    fn library(&mut self, file: &'static str) -> Result<&Library, RendererError> {
        if !self.loaded.contains_key(file) {
            let library = self.load(file)?;
            self.loaded.insert(file, library);
        }
        self.loaded.get(file).ok_or_else(|| missing(file, ""))
    }

    /// Creates the library, from precompiled bytes if the build had them.
    fn load(&self, file: &'static str) -> Result<Library, RendererError> {
        if let Some((_, bytes)) = METAL_LIBRARIES.iter().find(|(name, _)| *name == file) {
            let data = DispatchData::from_static_bytes(bytes);
            return self
                .device
                .newLibraryWithData_error(&data)
                .map_err(|error| RendererError::ShaderCompilation {
                    name: format!("{file}.metallib"),
                    messages: describe(&error),
                });
        }
        let Some((_, source)) = METAL_SOURCES.iter().find(|(name, _)| *name == file) else {
            return Err(RendererError::ShaderCompilation {
                name: format!("{file}.metal"),
                messages: "the shader file is not part of this build".to_owned(),
            });
        };
        log::info!("compiling {file}.metal from source (no precompiled library in this build)");
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(MTLLanguageVersion::Version3_0);
        // Fast math off, matching build.rs's `-fno-fast-math`: the depth prepass and the
        // camera pass compute clip position in different functions, and primed draws test
        // `LessEqual` against the prepass's depth. Fast math lets the compiler reassociate the
        // two differently, which would leave holes in primed surfaces.
        #[allow(
            deprecated,
            reason = "mathMode replaces it, but only on macOS 15 and newer"
        )]
        options.setFastMathEnabled(false);
        self.device
            .newLibraryWithSource_options_error(&NSString::from_str(source), Some(&options))
            .map_err(|error| RendererError::ShaderCompilation {
                name: format!("{file}.metal"),
                messages: describe(&error),
            })
    }
}

/// The error for a function the library does not define.
fn missing(file: &str, entry: &str) -> RendererError {
    api_error(
        "MTLLibrary::newFunctionWithName",
        format!("{file}.metal has no function named '{entry}'"),
    )
}
