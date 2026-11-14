//! Theme setup shared by every Relay window.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, Window, px};
use relay_core::{AppSettings, Appearance, ColorPreset};

/// Apply the user's appearance preference, following the desktop for
/// [`Appearance::System`].
pub fn apply_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    let mode = match appearance {
        Appearance::System => ThemeMode::from(window.appearance()),
        Appearance::Light => ThemeMode::Light,
        Appearance::Dark => ThemeMode::Dark,
    };
    Theme::change(mode, Some(window), cx);
}

fn hex(h: &str) -> Option<Hsla> {
    Rgba::try_from(h).ok().map(Into::into)
}

fn with_alpha(mut color: Hsla, a: f32) -> Hsla {
    color.a = a;
    color
}

/// Apply the selected curated color preset. Presets only adjust accent/brand
/// colors so surfaces and text contrast remain correct in both light and dark
/// modes.
pub fn apply_colors(settings: &AppSettings, cx: &mut App) {
    let preset = settings.color_preset;
    if preset == ColorPreset::Default {
        return;
    }
    Theme::update(cx, |theme| {
        let is_dark = theme.mode.is_dark();
        let (primary_hex, accent_hex) = match (preset, is_dark) {
            (ColorPreset::Default, _) => ("", ""),
            (ColorPreset::Ocean, false) => ("#2563EB", "#0284C7"),
            (ColorPreset::Ocean, true) => ("#3B82F6", "#38BDF8"),
            (ColorPreset::Forest, false) => ("#059669", "#0D9488"),
            (ColorPreset::Forest, true) => ("#34D399", "#2DD4BF"),
            (ColorPreset::Rose, false) => ("#E11D48", "#DB2777"),
            (ColorPreset::Rose, true) => ("#FB7185", "#F472B6"),
            (ColorPreset::Amber, false) => ("#D97706", "#EA580C"),
            (ColorPreset::Amber, true) => ("#FBBF24", "#FB923C"),
            (ColorPreset::Violet, false) => ("#7C3AED", "#9333EA"),
            (ColorPreset::Violet, true) => ("#A78BFA", "#C084FC"),
            (ColorPreset::Graphite, false) => ("#475569", "#334155"),
            (ColorPreset::Graphite, true) => ("#94A3B8", "#CBD5E1"),
        };
        let Some(primary) = hex(primary_hex) else {
            return;
        };
        let accent = hex(accent_hex).unwrap_or(primary);
        let colors = &mut theme.colors;
        colors.primary = primary;
        colors.button_primary = primary;
        colors.sidebar_primary = primary;
        colors.ring = accent;
        colors.link = accent;
        colors.link_hover = primary;
        let soft_a = if is_dark { 0.22 } else { 0.14 };
        let active_a = if is_dark { 0.30 } else { 0.20 };
        let soft = with_alpha(accent, soft_a);
        let active = with_alpha(accent, active_a);
        colors.sidebar_accent = active;
        colors.list_hover = soft;
        colors.list_active = active;
        colors.tab_active = active;
        colors.table_hover = soft;
        colors.table_active = active;
        colors.selection = with_alpha(accent, if is_dark { 0.35 } else { 0.25 });
    });
}

/// Install Relay's typography once, after the kit theme exists.
pub fn install_typography(cx: &mut App, settings: &relay_core::AppSettings) {
    Theme::update(cx, |theme| {
        theme.font_family = settings.font_family.clone().into();
        theme.font_size = px(settings.font_size);
        theme.mono_font_family = settings.mono_font_family.clone().into();
        theme.mono_font_size = px((settings.font_size - 2.0).max(10.0));
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
    });
}

/// Apply appearance mode then reapply all user customizations.
pub fn apply_theme(settings: &AppSettings, window: &mut Window, cx: &mut App) {
    apply_appearance(settings.appearance, window, cx);
    install_typography(cx, settings);
    apply_colors(settings, cx);
}
