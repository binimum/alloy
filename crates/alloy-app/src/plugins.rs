use crate::audio::{SampleBlockProcessor, SampleBlockProcessorFactory};
use alloy_core::{
    ABI_VERSION, ModuleCategory, ModuleManifest, NativeAudioBuffer, NativeCreateFn,
    NativeDestroyFn, NativePluginDescriptorV1, NativeProcessFn,
};
use libloading::Library;
use serde::Deserialize;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredModuleKind {
    Manifest,
    Native,
}

#[derive(Debug, Clone)]
pub struct DiscoveredModule {
    pub manifest: ModuleManifest,
    pub path: PathBuf,
    pub kind: DiscoveredModuleKind,
}

pub struct PluginHost {
    modules: Vec<DiscoveredModule>,
    native_audio_processors: Vec<NativeAudioProcessorFactory>,
}

impl PluginHost {
    #[must_use]
    pub fn load(modules_dir: &Path) -> Self {
        let mut host = Self {
            modules: Vec::new(),
            native_audio_processors: Vec::new(),
        };
        host.discover(modules_dir);
        host
    }

    #[must_use]
    pub fn modules(&self) -> &[DiscoveredModule] {
        &self.modules
    }

    pub fn audio_processor_factory(
        &self,
        id: &str,
    ) -> Option<Box<dyn SampleBlockProcessorFactory>> {
        self.native_audio_processors
            .iter()
            .find(|factory| factory.manifest.id == id)
            .cloned()
            .map(|factory| Box::new(factory) as Box<dyn SampleBlockProcessorFactory>)
    }

    fn discover(&mut self, modules_dir: &Path) {
        let Ok(entries) = std::fs::read_dir(modules_dir) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                self.discover(&path);
                continue;
            }

            if path.extension().and_then(|extension| extension.to_str()) == Some("toml") {
                if let Ok(module) = load_manifest_module(&path) {
                    self.modules.push(module);
                }
                continue;
            }

            if is_native_library(&path) {
                if let Ok((module, audio_processor)) = load_native_module(&path) {
                    self.modules.push(module);
                    if let Some(audio_processor) = audio_processor {
                        self.native_audio_processors.push(audio_processor);
                    }
                }
            }
        }

        self.modules
            .sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    }
}

fn load_manifest_module(path: &Path) -> anyhow::Result<DiscoveredModule> {
    let raw = std::fs::read_to_string(path)?;
    let file: ManifestModuleFile = toml::from_str(&raw)?;
    Ok(DiscoveredModule {
        manifest: ModuleManifest {
            id: file.id,
            name: file.name,
            version: file.version,
            category: file.category,
            description: file.description,
            authors: file.authors,
            enabled_by_default: file.enabled_by_default,
            homepage: file.homepage,
            capabilities: file.capabilities,
        },
        path: path.to_path_buf(),
        kind: DiscoveredModuleKind::Manifest,
    })
}

fn load_native_module(
    path: &Path,
) -> anyhow::Result<(DiscoveredModule, Option<NativeAudioProcessorFactory>)> {
    let library = Arc::new(unsafe { Library::new(path)? });
    let (manifest, audio_processor) = {
        let entry = unsafe {
            library.get::<unsafe extern "C" fn() -> *const NativePluginDescriptorV1>(
                b"alloy_plugin_entry",
            )?
        };
        let descriptor = unsafe { entry() };
        if descriptor.is_null() {
            anyhow::bail!("native Alloy module returned a null descriptor");
        }

        let descriptor = unsafe { &*descriptor };
        if descriptor.abi_version != ABI_VERSION {
            anyhow::bail!("unsupported Alloy native ABI {}", descriptor.abi_version);
        }
        if descriptor.manifest_json.is_null() {
            anyhow::bail!("native Alloy module did not expose manifest JSON");
        }

        let raw_manifest = unsafe { CStr::from_ptr(descriptor.manifest_json) }
            .to_string_lossy()
            .into_owned();
        let manifest = serde_json::from_str::<ModuleManifest>(&raw_manifest)?;
        let audio_processor = match (
            descriptor.audio_processor.create,
            descriptor.audio_processor.destroy,
            descriptor.audio_processor.process,
        ) {
            (Some(create), Some(destroy), Some(process)) => Some(NativeAudioProcessorFactory {
                manifest: manifest.clone(),
                library: Arc::clone(&library),
                create,
                destroy,
                process,
            }),
            _ => None,
        };

        (manifest, audio_processor)
    };

    Ok((
        DiscoveredModule {
            manifest,
            path: path.to_path_buf(),
            kind: DiscoveredModuleKind::Native,
        },
        audio_processor,
    ))
}

#[derive(Clone)]
pub struct NativeAudioProcessorFactory {
    manifest: ModuleManifest,
    library: Arc<Library>,
    create: NativeCreateFn,
    destroy: NativeDestroyFn,
    process: NativeProcessFn,
}

impl SampleBlockProcessorFactory for NativeAudioProcessorFactory {
    fn create(
        self: Box<Self>,
        _channels: u16,
        _sample_rate: u32,
    ) -> anyhow::Result<Box<dyn SampleBlockProcessor>> {
        let instance = unsafe { (self.create)() };
        if instance.is_null() {
            anyhow::bail!("{} returned a null processor instance", self.manifest.name);
        }

        Ok(Box::new(NativeAudioProcessorInstance {
            _library: Arc::clone(&self.library),
            instance,
            destroy: self.destroy,
            process: self.process,
        }))
    }
}

pub struct NativeAudioProcessorInstance {
    _library: Arc<Library>,
    instance: *mut std::ffi::c_void,
    destroy: NativeDestroyFn,
    process: NativeProcessFn,
}

unsafe impl Send for NativeAudioProcessorInstance {}

impl SampleBlockProcessor for NativeAudioProcessorInstance {
    fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32) {
        if self.instance.is_null() || samples.is_empty() {
            return;
        }

        let mut buffer = NativeAudioBuffer {
            samples: samples.as_mut_ptr(),
            sample_count: samples.len(),
            channels,
            sample_rate,
        };

        unsafe {
            (self.process)(self.instance, &mut buffer);
        }
    }
}

impl Drop for NativeAudioProcessorInstance {
    fn drop(&mut self) {
        if !self.instance.is_null() {
            unsafe {
                (self.destroy)(self.instance);
            }
        }
    }
}

fn is_native_library(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };

    matches!(extension, "dll" | "dylib" | "so")
}

#[derive(Debug, Deserialize)]
struct ManifestModuleFile {
    id: String,
    name: String,
    version: String,
    category: ModuleCategory,
    description: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    enabled_by_default: bool,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
}
