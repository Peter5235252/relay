//! Theme setup shared by every Relay window.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window, px};
use relay_core::AppSettings;
use relay_core::Appearance;

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

/// Apply user-selected colors to the active theme.
pub fn apply_colors(settings: &AppSettings, cx: &mut App) {
    let parse = |hex: &str| -> Option<gpui_kit::Hsla> {
        let hex = hex.trim();
        if hex.is_empty() {
            return None;
        }
        let rgba = gpui_kit::Rgba::try_from(hex).ok()?;
        Some(rgba.into())
    };
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        if let Some(c) = parse(&settings.primary_color) {
            colors.primary = c;
            colors.button_primary = c;
            colors.sidebar_primary = c;
        }
        if let Some(c) = parse(&settings.accent_color) {
            colors.accent = c;
            colors.ring = c;
        }
        if let Some(c) = parse(&settings.background_color) {
            colors.background = c;
        }
        if let Some(c) = parse(&settings.foreground_color) {
            colors.foreground = c;
            colors.popover_foreground = c;
            colors.sidebar_foreground = c;
        }
        if let Some(c) = parse(&settings.secondary_color) {
            colors.secondary = c;
            colors.button_secondary = c;
            colors.list = c;
            colors.popover = c;
            colors.table = c;
        }
        if let Some(c) = parse(&settings.secondary_foreground_color) {
            colors.secondary_foreground = c;
            colors.button_secondary_foreground = c;
            colors.list_head = c;
            colors.table_head_foreground = c;
        }
        if let Some(c) = parse(&settings.muted_color) {
            colors.muted = c;
            colors.table_head = c;
            colors.tab_bar = c;
            colors.list_even = c;
            colors.table_even = c;
        }
        if let Some(c) = parse(&settings.muted_foreground_color) {
            colors.muted_foreground = c;
            colors.tab_foreground = c;
        }
        if let Some(c) = parse(&settings.border_color) {
            colors.border = c;
            colors.sidebar_border = c;
            colors.title_bar_border = c;
            colors.list_active_border = c;
            colors.table_row_border = c;
        }
        if let Some(c) = parse(&settings.input_color) {
            colors.input = c;
        }
        if let Some(c) = parse(&settings.sidebar_color) {
            colors.sidebar = c;
            colors.title_bar = c;
        }
        if let Some(c) = parse(&settings.sidebar_foreground_color) {
            colors.sidebar_foreground = c;
        }
        if let Some(c) = parse(&settings.sidebar_accent_color) {
            colors.sidebar_accent = c;
            colors.list_hover = c;
            colors.list_active = c;
            colors.tab_active = c;
            colors.table_hover = c;
            colors.table_active = c;
        }
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
