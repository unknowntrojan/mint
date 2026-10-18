pub mod asset_registry;
pub mod paths;
use std::io::{Read, Seek};

use unreal_asset::{exports::ExportBaseTrait as _, types::PackageIndex, Asset};

pub mod splice;

pub fn get_root_export<R: Read + Seek>(asset: &Asset<R>) -> Option<PackageIndex> {
    for (i, e) in asset.asset_data.exports.iter().enumerate() {
        let base = e.get_base_export();
        if base.outer_index.index == 0 {
            return Some(PackageIndex::from_export(i as i32).unwrap());
        }
    }
    None
}
