//! Skills system - markdown skill discovery and loading.

mod loader;
mod types;

pub use loader::SkillsLoader;
pub use types::{
    EnvSpec, InstallOption, Skill, SkillInfo, SkillMetadata, SkillRequirements, ZeptoMetadata,
};
pub mod registry;
