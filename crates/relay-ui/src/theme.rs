//! Theme setup shared by every Relay window.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window, px};
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
