//! USD as a Studio plugin library (`formats/usd.dylib`).
use kinogaki_studio_api::export_package;

kinogaki_shared::link!();
export_package!(kinogaki_usd::package);
