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
pub fn install_typography(cx: &mut App) {
    Theme::update(cx, |theme| {
        // Both families ship with most Linux desktops or are installed with the
        // app's recommended font package. GPUI falls back safely if they are
        // missing.
        theme.font_family = "Fira Sans".into();
        theme.font_size = px(15.);
        theme.mono_font_family = "DejaVu Sans Mono".into();
        theme.mono_font_size = px(13.);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
    });
}
