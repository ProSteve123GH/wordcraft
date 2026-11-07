//! GPU backend and adapter policy for the Android shell.
//!
//! The interface is drawn by wgpu, but Android devices differ widely: almost every device from the
//! last decade exposes Vulkan, some only expose OpenGL ES through `libEGL`, and emulators may offer
//! a software rasteriser. This module asks wgpu for adapters on **both** backends, prefers a
//! hardware Vulkan adapter, falls back to hardware OpenGL ES, and never silently runs on a software
//! rasteriser (a diagnostic environment variable is required for that).
//!
//! The selection itself is plain data (`AdapterFacts` + [`choose_adapter`]) so it is unit-tested on
//! the host, where the Android dependencies and wgpu are not built. Only the small `android` module
//! touches wgpu.
//!
//! Environment variables (mainly for emulators, CI and `adb shell` diagnostics):
//!
//! - `WORDFCRAFT_GPU=vulkan|gles|auto` forces one backend. `WGPU_BACKEND` is honoured too, with
//!   the same names wgpu accepts; anything other than Vulkan/OpenGL ES is ignored on Android.
//! - `WORDFCRAFT_ALLOW_SOFTWARE_GPU=1` (or `WORDFCRAFT_GPU=software`) permits a software adapter
//!   when no hardware adapter can present to the Activity. This is a diagnostic fallback, never the
//!   default: it is slow and the reason is logged loudly.

#![cfg_attr(not(target_os = "android"), allow(dead_code))]

/// The backend kinds WordCraft cares about, independent of wgpu's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackendKind {
    Vulkan,
    Gl,
    Metal,
    Dx12,
    BrowserWebGpu,
    Noop,
}

/// The physical device classes wgpu reports, independent of wgpu's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceKind {
    DiscreteGpu,
    IntegratedGpu,
    VirtualGpu,
    Cpu,
    Other,
}

/// What the adapter selector needs to know about one candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AdapterFacts {
    pub(crate) name: String,
    pub(crate) backend: BackendKind,
    pub(crate) device: DeviceKind,
}

impl BackendKind {
    /// Higher is preferred. Vulkan comes first by design; OpenGL ES is the fallback for devices
    /// without a usable Vulkan driver. The other backends cannot occur on Android today, but the
    /// ranking keeps the function honest on the host and in tests.
    fn score(self) -> u32 {
        match self {
            Self::Vulkan => 5,
            Self::Metal | Self::Dx12 => 4,
            Self::Gl => 3,
            Self::BrowserWebGpu => 2,
            Self::Noop => 0,
        }
    }
}

impl DeviceKind {
    fn is_software(self) -> bool {
        matches!(self, Self::Cpu)
    }

    /// A discrete GPU is preferred over an integrated one; a virtualised GPU is still hardware.
    /// `Other` is kept above `Cpu`: GL drivers sometimes report an unknown type, and rejecting
    /// them would disable OpenGL ES on working devices.
    fn score(self) -> u32 {
        match self {
            Self::DiscreteGpu => 4,
            Self::IntegratedGpu => 3,
            Self::VirtualGpu => 2,
            Self::Other => 1,
            Self::Cpu => 0,
        }
    }
}

/// Picks the best adapter from `facts`, returning its index.
///
/// Hardware always wins over software. Among hardware, a higher backend score wins (Vulkan before
/// OpenGL ES), then a higher device score. Ties keep the first candidate, which is wgpu's own
/// enumeration order (and therefore stable for a given device). A software adapter is only
/// returned when `allow_software` is true and no hardware adapter exists.
pub(crate) fn choose_adapter(facts: &[AdapterFacts], allow_software: bool) -> Result<usize, String> {
    if facts.is_empty() {
        return Err("wgpu found no graphics adapters for Vulkan or OpenGL ES".to_owned());
    }

    let mut best_hardware: Option<(usize, u32)> = None;
    let mut best_any: Option<(usize, u32)> = None;
    for (index, fact) in facts.iter().enumerate() {
        let score = fact.backend.score() * 10 + fact.device.score();
        if best_any.is_none_or(|(_, best)| score > best) {
            best_any = Some((index, score));
        }
        if !fact.device.is_software() && best_hardware.is_none_or(|(_, best)| score > best) {
            best_hardware = Some((index, score));
        }
    }

    if let Some((index, _)) = best_hardware {
        return Ok(index);
    }
    if allow_software && let Some((index, _)) = best_any {
        return Ok(index);
    }
    Err(format!(
        "no hardware graphics adapter (only software adapters: {}); set WORDFCRAFT_ALLOW_SOFTWARE_GPU=1 for a diagnostic software fallback",
        describe(facts)
    ))
}

/// A one-line description of all candidates, for the failure log and the unit tests.
pub(crate) fn describe(facts: &[AdapterFacts]) -> String {
    facts.iter().map(|fact| format!("{} ({:?}, {:?})", fact.name, fact.backend, fact.device)).collect::<Vec<_>>().join(", ")
}

/// Parses a backend name. `None` means "no override" (including `auto` and junk, which is logged).
pub(crate) fn parse_backend(value: &str) -> Option<BackendKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => Some(BackendKind::Vulkan),
        "gl" | "gles" | "opengl" | "opengl es" | "opengles" => Some(BackendKind::Gl),
        _ => None,
    }
}

/// True when the value clearly means "yes".
pub(crate) fn env_truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// A requested backend from the environment, if any.
pub(crate) fn backend_override() -> Option<BackendKind> {
    if let Ok(value) = std::env::var("WORDFCRAFT_GPU") {
        if let Some(kind) = parse_backend(&value) {
            return Some(kind);
        }
        let value = value.trim();
        if !value.eq_ignore_ascii_case("auto") && !value.eq_ignore_ascii_case("software") && !value.is_empty() {
            log::warn!("ignoring WORDFCRAFT_GPU={value:?}; expected vulkan, gles, auto or software");
        }
    }
    // wgpu's own variable. It accepts lists; `preferred_backends` filters them with `from_env`.
    None
}

/// Whether a software adapter may be used as a last resort.
pub(crate) fn software_allowed() -> bool {
    if let Ok(value) = std::env::var("WORDFCRAFT_ALLOW_SOFTWARE_GPU")
        && env_truthy(&value)
    {
        return true;
    }
    std::env::var("WORDFCRAFT_GPU").is_ok_and(|value| value.trim().eq_ignore_ascii_case("software"))
}

/// The wgpu backends to ask for, before adapter selection. Only Vulkan and OpenGL ES can exist on
/// Android; `WGPU_BACKEND` may narrow the list for diagnostics, but an empty/unknown selection
/// falls back to both rather than leaving the app with no renderer at all.
#[cfg(target_os = "android")]
fn preferred_backends() -> eframe::wgpu::Backends {
    use eframe::wgpu;
    let allowed = wgpu::Backends::VULKAN | wgpu::Backends::GL;
    if let Some(kind) = backend_override() {
        return match kind {
            BackendKind::Vulkan => wgpu::Backends::VULKAN,
            BackendKind::Gl => wgpu::Backends::GL,
            _ => allowed,
        };
    }
    let requested = wgpu::Backends::from_env().unwrap_or(allowed) & allowed;
    if requested.is_empty() {
        log::warn!("WGPU_BACKEND selected no Android backend; using Vulkan and OpenGL ES");
        allowed
    } else {
        requested
    }
}

#[cfg(target_os = "android")]
mod android {
    use std::sync::Arc;

    use eframe::egui_wgpu::WgpuSetupCreateNew;
    use eframe::wgpu;

    use super::{AdapterFacts, BackendKind, DeviceKind, choose_adapter, software_allowed};

    /// Applies the GPU policy to eframe's options. Called before the event loop starts.
    pub(crate) fn configure(options: &mut eframe::NativeOptions) {
        options.renderer = eframe::Renderer::Wgpu;
        options.wgpu_options.wgpu_setup = setup().into();
    }

    fn setup() -> WgpuSetupCreateNew {
        let backends = super::preferred_backends();
        let allow_software = software_allowed();
        log::info!("wgpu: requesting {backends:?}; software adapters allowed: {allow_software}");

        let mut setup = WgpuSetupCreateNew::without_display_handle();
        setup.instance_descriptor.backends = backends;
        setup.power_preference = wgpu::PowerPreference::HighPerformance;

        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| select(adapters, surface, allow_software)));

        // egui's default asks for an 8k texture even on the GL backend. Some GLES drivers report a
        // smaller limit, and a device request above the adapter's limit fails at startup. Asking
        // for the smaller of 8k and what the adapter actually supports keeps both backends usable
        // without weakening the Vulkan path on tablets that do support 8k.
        setup.device_descriptor = Arc::new(|adapter| {
            let backend = adapter.get_info().backend;
            let base = if backend == wgpu::Backend::Gl { wgpu::Limits::downlevel_webgl2_defaults() } else { wgpu::Limits::default() };
            let max_texture_dimension_2d = 8192.min(adapter.limits().max_texture_dimension_2d).max(1);
            wgpu::DeviceDescriptor {
                label: Some("WordCraft Android wgpu device"),
                required_limits: wgpu::Limits { max_texture_dimension_2d, ..base },
                ..Default::default()
            }
        });

        setup
    }

    fn select(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>, allow_software: bool) -> Result<wgpu::Adapter, String> {
        let mut facts = Vec::with_capacity(adapters.len());
        let mut usable = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            let info = adapter.get_info();
            let fact = facts_of(&info);
            let presents = surface.is_none_or(|surface| adapter.is_surface_supported(surface));
            log::info!(
                "wgpu adapter candidate: {} (backend {:?}, device {:?}, driver {:?}){}",
                fact.name,
                fact.backend,
                fact.device,
                info.driver,
                if presents { "" } else { " — cannot present to the Activity surface" }
            );
            if presents {
                facts.push(fact);
                usable.push(adapter.clone());
            }
        }
        if usable.is_empty() {
            return Err(format!("wgpu found {} adapter(s), but none can present to the Activity surface", adapters.len()));
        }

        let index = choose_adapter(&facts, allow_software)?;
        let chosen = usable.get(index).cloned().ok_or_else(|| "adapter selection returned an index outside the candidate list".to_owned())?;
        let info = chosen.get_info();
        log::info!("wgpu selected: {} (backend {:?}, device {:?}, driver {:?})", info.name, info.backend, info.device_type, info.driver);
        Ok(chosen)
    }

    fn facts_of(info: &wgpu::AdapterInfo) -> AdapterFacts {
        AdapterFacts {
            name: info.name.clone(),
            backend: match info.backend {
                wgpu::Backend::Vulkan => BackendKind::Vulkan,
                wgpu::Backend::Gl => BackendKind::Gl,
                wgpu::Backend::Metal => BackendKind::Metal,
                wgpu::Backend::Dx12 => BackendKind::Dx12,
                wgpu::Backend::BrowserWebGpu => BackendKind::BrowserWebGpu,
                wgpu::Backend::Noop => BackendKind::Noop,
            },
            device: match info.device_type {
                wgpu::DeviceType::DiscreteGpu => DeviceKind::DiscreteGpu,
                wgpu::DeviceType::IntegratedGpu => DeviceKind::IntegratedGpu,
                wgpu::DeviceType::VirtualGpu => DeviceKind::VirtualGpu,
                wgpu::DeviceType::Cpu => DeviceKind::Cpu,
                wgpu::DeviceType::Other => DeviceKind::Other,
            },
        }
    }

    /// Logs the renderer eframe actually created. The selector log is the decision; this confirms
    /// the adapter and render target the first frame will use, including the software warning.
    pub(crate) fn log_ready(cc: &eframe::CreationContext<'_>) {
        let Some(render) = &cc.wgpu_render_state else {
            log::warn!("wgpu render state is missing; the interface will not be drawn");
            return;
        };
        let info = render.adapter.get_info();
        log::info!(
            "wgpu ready: {} (backend {:?}, device {:?}, driver {:?}), target {:?}, {} adapter(s) enumerated",
            info.name,
            info.backend,
            info.device_type,
            info.driver,
            render.target_format,
            render.available_adapters.len()
        );
        if info.device_type == wgpu::DeviceType::Cpu {
            log::warn!(
                "software rasteriser in use: this is the diagnostic fallback and will be slow; no hardware Vulkan/OpenGL ES adapter could present"
            );
        }
        if render.available_adapters.len() > 1 {
            let facts = render.available_adapters.iter().map(|adapter| facts_of(&adapter.get_info())).collect::<Vec<_>>();
            log::info!("wgpu available adapters: {}", super::describe(&facts));
        }
    }
}

#[cfg(target_os = "android")]
pub(crate) use android::{configure, log_ready};

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(name: &str, backend: BackendKind, device: DeviceKind) -> AdapterFacts {
        AdapterFacts { name: name.to_owned(), backend, device }
    }

    #[test]
    fn empty_adapter_list_is_an_error() {
        assert!(choose_adapter(&[], false).is_err());
        assert!(choose_adapter(&[], true).is_err());
    }

    #[test]
    fn hardware_vulkan_wins_over_hardware_opengl() {
        let facts = [
            fact("Adreno (GL)", BackendKind::Gl, DeviceKind::IntegratedGpu),
            fact("Adreno (Vulkan)", BackendKind::Vulkan, DeviceKind::IntegratedGpu),
        ];
        assert_eq!(choose_adapter(&facts, false), Ok(1));
    }

    #[test]
    fn opengl_is_the_fallback_when_vulkan_is_absent() {
        let facts = [fact("Mali (GL)", BackendKind::Gl, DeviceKind::IntegratedGpu), fact("SwiftShader", BackendKind::Gl, DeviceKind::Cpu)];
        assert_eq!(choose_adapter(&facts, false), Ok(0));
    }

    #[test]
    fn hardware_always_beats_software() {
        let facts = [fact("SwiftShader Vulkan", BackendKind::Vulkan, DeviceKind::Cpu), fact("Mali GL", BackendKind::Gl, DeviceKind::IntegratedGpu)];
        assert_eq!(choose_adapter(&facts, false), Ok(1));
    }

    #[test]
    fn a_software_only_device_is_refused_unless_allowed() {
        let facts = [fact("SwiftShader Vulkan", BackendKind::Vulkan, DeviceKind::Cpu), fact("llvmpipe", BackendKind::Gl, DeviceKind::Cpu)];
        assert!(choose_adapter(&facts, false).is_err());
        assert_eq!(choose_adapter(&facts, true), Ok(0));
    }

    #[test]
    fn unknown_device_types_are_not_treated_as_software() {
        let facts = [fact("Vendor GL", BackendKind::Gl, DeviceKind::Other)];
        assert_eq!(choose_adapter(&facts, false), Ok(0));
    }

    #[test]
    fn ties_keep_wgpu_enumeration_order() {
        let facts = [fact("first", BackendKind::Vulkan, DeviceKind::IntegratedGpu), fact("second", BackendKind::Vulkan, DeviceKind::IntegratedGpu)];
        assert_eq!(choose_adapter(&facts, false), Ok(0));
    }

    #[test]
    fn backend_names_are_parsed_case_insensitively() {
        assert_eq!(parse_backend("Vulkan"), Some(BackendKind::Vulkan));
        assert_eq!(parse_backend(" vk "), Some(BackendKind::Vulkan));
        assert_eq!(parse_backend("gles"), Some(BackendKind::Gl));
        assert_eq!(parse_backend("OpenGL"), Some(BackendKind::Gl));
        assert_eq!(parse_backend("auto"), None);
        assert_eq!(parse_backend("dx12"), None);
        assert_eq!(parse_backend(""), None);
    }

    #[test]
    fn truthy_values_are_recognised() {
        for value in ["1", "true", "YES", " On "] {
            assert!(env_truthy(value), "{value}");
        }
        for value in ["0", "false", "no", "off", "", "maybe"] {
            assert!(!env_truthy(value), "{value}");
        }
    }

    #[test]
    fn descriptions_include_every_candidate() {
        let facts = [fact("A", BackendKind::Vulkan, DeviceKind::IntegratedGpu), fact("B", BackendKind::Gl, DeviceKind::Cpu)];
        let description = describe(&facts);
        assert!(description.contains('A'));
        assert!(description.contains('B'));
        assert!(description.contains("Vulkan"));
        assert!(description.contains("Gl"));
    }
}
