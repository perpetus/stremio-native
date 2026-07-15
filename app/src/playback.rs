use std::{
    collections::HashMap,
    sync::{
        RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use stremio_core::{
    models::{common::Loadable, meta_details::MetaDetails, player::Selected},
    types::addon::{Descriptor, ResourcePath},
};

/// Presentation data for a stream whose full core selection remains in Rust.
#[derive(Clone, Debug)]
pub struct StreamSelectionView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider: String,
}

#[derive(Clone)]
struct RegisteredSelection {
    selected: Selected,
    stream_name: String,
}

/// Keeps full core stream selections out of the Slint presentation model.
#[derive(Default)]
pub struct PlaybackSelections {
    next_id: AtomicU64,
    entries: RwLock<HashMap<String, RegisteredSelection>>,
    trailer_id: RwLock<Option<String>>,
}

impl PlaybackSelections {
    /// Atomically replaces visible stream selections and returns their UI views.
    pub fn rebuild(&self, details: &MetaDetails, addons: &[Descriptor]) -> Vec<StreamSelectionView> {
        let meta_request = details
            .selected
            .as_ref()
            .and_then(|selected| {
                details
                    .meta_items
                    .iter()
                    .find(|resource| resource.request.path.eq_no_extra(&selected.meta_path))
            })
            .map(|resource| resource.request.clone());

        let mut next_entries = HashMap::new();
        let mut views = Vec::new();

        let trailer_id = details
            .meta_items
            .iter()
            .find_map(|resource| {
                let Loadable::Ready(meta) = resource.content.as_ref()? else {
                    return None;
                };
                meta.preview.trailer_streams.first().cloned()
            })
            .map(|stream| {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
                next_entries.insert(
                    id.clone(),
                    RegisteredSelection {
                        selected: Selected {
                            stream,
                            stream_request: None,
                            meta_request: meta_request.clone(),
                            subtitles_path: None,
                        },
                        stream_name: "Trailer".to_owned(),
                    },
                );
                id
            });

        for resource in &details.streams {
            let Some(Loadable::Ready(streams)) = &resource.content else {
                continue;
            };

            for stream in streams {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
                let name = stream.name.clone().unwrap_or_else(|| "Stream".to_owned());
                let description = stream.description.clone().unwrap_or_default();
                let subtitles_path = ResourcePath {
                    resource: "subtitles".to_owned(),
                    r#type: resource.request.path.r#type.clone(),
                    id: resource.request.path.id.clone(),
                    extra: Vec::new(),
                };
                let selected = Selected {
                    stream: stream.clone(),
                    stream_request: Some(resource.request.clone()),
                    meta_request: meta_request.clone(),
                    subtitles_path: Some(subtitles_path),
                };

                next_entries.insert(
                    id.clone(),
                    RegisteredSelection {
                        selected,
                        stream_name: if description.is_empty() {
                            name.clone()
                        } else {
                            description.clone()
                        },
                    },
                );
                views.push(StreamSelectionView {
                    id,
                    name,
                    description,
                    provider: addons
                        .iter()
                        .find(|addon| addon.transport_url == resource.request.base)
                        .map(|addon| addon.manifest.name.clone())
                        .unwrap_or_else(|| {
                            resource
                                .request
                                .base
                                .host_str()
                                .unwrap_or("Addon")
                                .to_owned()
                        }),
                });
            }
        }

        match self.entries.write() {
            Ok(mut entries) => *entries = next_entries,
            Err(poisoned) => *poisoned.into_inner() = next_entries,
        }
        match self.trailer_id.write() {
            Ok(mut current) => *current = trailer_id,
            Err(poisoned) => *poisoned.into_inner() = trailer_id,
        }
        views
    }

    pub fn trailer_id(&self) -> Option<String> {
        match self.trailer_id.read() {
            Ok(id) => id.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Resolves an opaque UI ID back to the full core selection and label.
    pub fn resolve(&self, id: &str) -> Option<(Selected, String)> {
        let entries = match self.entries.read() {
            Ok(entries) => entries,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries
            .get(id)
            .map(|entry| (entry.selected.clone(), entry.stream_name.clone()))
    }
}
