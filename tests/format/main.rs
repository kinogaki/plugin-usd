//! The format through the SDK's contract: files read into the standard
//! values (`import`), materials in every render context written and read
//! back (`materials`), Material Libraries as Scopes where they are
//! (`libraries`), geometry with all it holds (`geometry`), items written and read back (`export`), instances
//! as USD's instancing (`instances`), the budgets (`measure`), and the
//! hostile files once found (`hostile`, made by `crafted`).
mod crafted;
mod export;
mod geometry;
mod hostile;
mod import;
mod instances;
mod libraries;
mod lights;
mod materials;
mod materialx;
mod measure;
mod portals;
mod primvars;
mod subdivision;
mod support;
