use stremio_core::{
    Model,
    models::{
        addon_details::AddonDetails,
        calendar::Calendar,
        catalog_with_filters::CatalogWithFilters,
        catalogs_with_extra::CatalogsWithExtra,
        continue_watching_preview::ContinueWatchingPreview,
        ctx::Ctx,
        data_export::DataExport,
        installed_addons_with_filters::InstalledAddonsWithFilters,
        library_with_filters::{ContinueWatchingFilter, LibraryWithFilters, NotRemovedFilter},
        link::Link,
        local_search::LocalSearch,
        meta_details::MetaDetails,
        player::Player,
        streaming_server::StreamingServer,
    },
    types::{addon::Descriptor, api::LinkAuthKey, resource::MetaItemPreview},
};

use crate::IconData;
use core_env::DesktopEnv;

#[derive(Model, Clone)]
#[model(DesktopEnv)]
pub struct AppModel {
    pub ctx: Ctx,
    pub auth_link: Link<LinkAuthKey>,
    pub data_export: DataExport,
    pub continue_watching_preview: ContinueWatchingPreview,
    pub board: CatalogsWithExtra,
    pub discover: CatalogWithFilters<MetaItemPreview>,
    pub library: LibraryWithFilters<NotRemovedFilter>,
    pub continue_watching: LibraryWithFilters<ContinueWatchingFilter>,
    pub search: CatalogsWithExtra,
    pub local_search: LocalSearch,
    pub calendar: Calendar,
    pub meta_details: MetaDetails,
    pub player: Player,
    pub remote_addons: CatalogWithFilters<Descriptor>,
    pub installed_addons: InstalledAddonsWithFilters,
    pub addon_details: AddonDetails,
    pub streaming_server: StreamingServer,
}

pub fn get_icon_data(pack: iconflow::Pack, name: &str) -> IconData {
    if let Ok(icon) = iconflow::try_icon(
        pack,
        name,
        iconflow::Style::Regular,
        iconflow::Size::Regular,
    ) {
        let mut glyph_str = String::new();
        if let Some(c) = char::from_u32(icon.codepoint) {
            glyph_str.push(c);
        }
        IconData {
            glyph: glyph_str.into(),
            font_family: icon.family.into(),
        }
    } else {
        IconData {
            glyph: "".into(),
            font_family: "".into(),
        }
    }
}

pub fn format_rate(bytes_per_second: f64) -> String {
    const KIB: f64 = 1_024.0;
    const MIB: f64 = KIB * 1_024.0;
    if bytes_per_second >= MIB {
        format!("{:.1} MiB/s", bytes_per_second / MIB)
    } else {
        format!("{:.0} KiB/s", bytes_per_second.max(0.0) / KIB)
    }
}
