//! Bundled developer fonts embedded in the binary.

use std::borrow::Cow;

/// A font option exposed in settings.
#[derive(Debug, Clone, Copy)]
pub struct BundledFont {
    pub family: &'static str,
    pub label: &'static str,
    pub mono: bool,
}

pub const UI_FONTS: &[BundledFont] = &[
    BundledFont {
        family: "Fira Sans",
        label: "Fira Sans (bundled)",
        mono: false,
    },
    BundledFont {
        family: "Inter",
        label: "Inter (system if installed)",
        mono: false,
    },
    BundledFont {
        family: "Noto Sans",
        label: "Noto Sans (system if installed)",
        mono: false,
    },
    BundledFont {
        family: "DejaVu Sans",
        label: "DejaVu Sans (system if installed)",
        mono: false,
    },
];

pub const MONO_FONTS: &[BundledFont] = &[
    BundledFont {
        family: "JetBrains Mono",
        label: "JetBrains Mono (bundled)",
        mono: true,
    },
    BundledFont {
        family: "Source Code Pro",
        label: "Source Code Pro (bundled)",
        mono: true,
    },
    BundledFont {
        family: "IBM Plex Mono",
        label: "IBM Plex Mono (bundled)",
        mono: true,
    },
    BundledFont {
        family: "DejaVu Sans Mono",
        label: "DejaVu Sans Mono (system if installed)",
        mono: true,
    },
    BundledFont {
        family: "Fira Code",
        label: "Fira Code (system if installed)",
        mono: true,
    },
];

static FIRA_SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/FiraSans-Regular.ttf");
static JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
static SOURCE_CODE_PRO_REGULAR: &[u8] = include_bytes!("../assets/fonts/SourceCodePro-Regular.ttf");
static IBM_PLEX_MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// Register bundled fonts with the GPUI text system.
pub fn register(cx: &gpui_kit::App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(FIRA_SANS_REGULAR),
        Cow::Borrowed(JETBRAINS_MONO_REGULAR),
        Cow::Borrowed(SOURCE_CODE_PRO_REGULAR),
        Cow::Borrowed(IBM_PLEX_MONO_REGULAR),
    ];
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!(%err, "failed to register bundled fonts");
    }
}

pub fn ui_font_labels() -> Vec<gpui_kit::SharedString> {
    UI_FONTS
        .iter()
        .map(|f| gpui_kit::SharedString::from(f.family))
        .collect()
}

pub fn mono_font_labels() -> Vec<gpui_kit::SharedString> {
    MONO_FONTS
        .iter()
        .map(|f| gpui_kit::SharedString::from(f.family))
        .collect()
}
