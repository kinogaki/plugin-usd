//! The format's own model: composition (`compose`), the scene (`scene`),
//! materials (`shade`), instancing (`instancing`), packages and memory
//! input (`package`), and mutation fuzzing of text, crates and packages
//! (`fuzz`).
mod common;
mod compose;
mod fuzz;
mod instancing;
mod package;
mod scene;
mod shade;
