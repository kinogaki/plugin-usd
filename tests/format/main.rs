//! The format through the SDK's contract: files read into the standard
//! values (`import`), materials in every render context written and read
//! back (`materials`), items written and read back (`export`), the
//! budgets (`measure`), and the hostile files once found (`hostile`, made
//! by `crafted`).
mod crafted;
mod export;
mod hostile;
mod import;
mod lights;
mod materials;
mod materialx;
mod measure;
mod primvars;
mod support;
