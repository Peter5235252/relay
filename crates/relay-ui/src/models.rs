//! Unified model picker data.
//!
//! The picker is one searchable, grouped list: the three cloud providers plus
//! any configured local Ollama or OpenAI-compatible models. Selecting an item
//! stores the Relay slug, which is also what conversations persist.

use gpui_kit::component::{
    searchable_list::SearchableListItem,
    select::{SearchableVec, SelectGroup, SelectState},
};
use gpui_kit::{App, AppContext as _, Entity, SharedString, Window};
use relay_core::{AppSettings, ModelSelection, Provider, find_model, models};

/// One selectable model.
#[derive(Clone)]
pub struct ModelChoice {
    /// Relay slug persisted with the conversation.
    pub slug: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub tagline: String,
    /// Provider display name.
    pub provider: String,
}

impl SearchableListItem for ModelChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn value(&self) -> &String {
        &self.slug
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.name.to_lowercase().contains(&query)
            || self.tagline.to_lowercase().contains(&query)
            || self.provider.to_lowercase().contains(&query)
            || self.slug.to_lowercase().contains(&query)
    }
}

/// The picker delegate type used throughout the app.
pub type ModelSelect = Entity<SelectState<SearchableVec<SelectGroup<ModelChoice>>>>;

/// Resolve a stored slug to a concrete model. Unknown cloud slugs fall back to
/// the default model rather than breaking the chat.
pub fn resolve(slug: &str) -> ModelSelection {
    if let Some(spec) = find_model(slug) {
        return ModelSelection::from_spec(spec);
    }
    if let Some(local) = ModelSelection::local_from_slug(slug) {
        return local;
    }
    find_model(relay_core::DEFAULT_MODEL_SLUG)
        .map(ModelSelection::from_spec)
        .unwrap_or_else(|| ModelSelection::ollama("llama3.2"))
}


fn installed_ollama_models() -> Vec<String> {
    let out = std::process::Command::new("ollama")
        .arg("list")
        .output();
    let Ok(out) = out else { return Vec::new(); };
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .map(|s| s.to_owned())
        .collect()
}

/// Build grouped picker items for the configured settings.
pub fn groups(settings: &AppSettings) -> Vec<SelectGroup<ModelChoice>> {
    let mut result: Vec<SelectGroup<ModelChoice>> = Vec::new();

    for provider in Provider::CLOUD {
        let items: Vec<ModelChoice> = models()
            .iter()
            .filter(|model| model.provider == provider)
            .map(|model| ModelChoice {
                slug: model.slug.to_owned(),
                name: model.name.to_owned(),
                tagline: model.tagline.to_owned(),
                provider: provider.label().to_owned(),
            })
            .collect();
        if !items.is_empty() {
            result.push(SelectGroup::new(provider.label()).items(items));
        }
    }

    let mut ready = Vec::new();
    // Bundled Nemotron variants - always available, auto-prepared on demand.
    for variant in crate::bundled::BundledVariant::all() {
        ready.push(ModelChoice {
            slug: format!("local/{}", variant.id()),
            name: variant.display_name().to_owned(),
            tagline: variant.tagline().to_owned(),
            provider: "Bundled".to_owned(),
        });
    }
    if !settings.ollama_model.trim().is_empty() {
        let model = ModelSelection::ollama(&settings.ollama_model);
        ready.push(ModelChoice {
            slug: model.slug.clone(),
            name: format!("{} · Ollama", model.name),
            tagline: "Running locally through Ollama.".to_owned(),
            provider: Provider::Ollama.label().to_owned(),
        });
    }
    let local_model = settings.local_model.trim();
    if !local_model.is_empty()
        && crate::bundled::BundledVariant::from_id(local_model).is_none()
        && crate::bundled::BundledVariant::from_model_slug(&format!("local/{local_model}"))
            .is_none()
    {
        let model = ModelSelection::openai_compatible(local_model);
        ready.push(ModelChoice {
            slug: model.slug.clone(),
            name: format!("{} · Local endpoint", model.name),
            tagline: "Served by an OpenAI-compatible endpoint.".to_owned(),
            provider: Provider::OpenAiCompatible.label().to_owned(),
        });
    }

    // Curated open-weight models. Installed ones merge into ready.
    let installed = installed_ollama_models();
    let curated: Vec<(&str, &str, &str)> = vec![
        ("deepseek-v4.1:flash", "DeepSeek V4.1 Flash", "MIT"),
        ("reflection-beam", "Reflection Beam", "Apache 2.0"),
        ("glm-5.3", "GLM-5.3", "GLM-5.3 (MIT-like, $10B MaaS review clause)"),
        ("glm-5.3:flash", "GLM-5.3 Flash", "MIT"),
        ("mistral-small-4", "Mistral Small 4", "Apache 2.0"),
    ];
    let mut installable = Vec::new();
    for (tag, name, license) in curated {
        let choice = ModelChoice {
            slug: format!("ollama/{}", tag),
            name: name.to_owned(),
            tagline: format!("Local, private. License: {}.", license),
            provider: "Local (Ollama)".to_owned(),
        };
        if installed.iter().any(|m| m == tag) {
            ready.push(choice);
        } else {
            installable.push(choice);
        }
    }

    if !ready.is_empty() {
        result.push(SelectGroup::new("Ready to use (local)").items(ready));
    }
        if !installable.is_empty() {
            result.push(SelectGroup::new("Install (needs download)").items(installable));
        }

    result
}

/// Build a fresh model select with a committed value.
pub fn model_select(
    settings: &AppSettings,
    slug: &str,
    window: &mut Window,
    cx: &mut App,
) -> ModelSelect {
    let items = SearchableVec::new(groups(settings));
    let slug = slug.to_owned();
    cx.new(|cx| {
        let mut state = SelectState::new(items, None, window, cx).searchable(true);
        state.set_selected_value(&slug, window, cx);
        state
    })
}

/// Build an ungrouped string select, committed to one of `values`.
pub fn string_select(
    values: &[SharedString],
    selected: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SelectState<SearchableVec<SharedString>>> {
    let items = SearchableVec::new(values.to_vec());
    let selected = SharedString::from(selected);
    cx.new(|cx| {
        let mut state = SelectState::new(items, None, window, cx);
        state.set_selected_value(&selected, window, cx);
        state
    })
}
