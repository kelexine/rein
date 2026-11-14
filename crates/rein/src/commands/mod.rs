// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Built-in slash command and ~/.rein / .rein SKILL.md registry exports

/// Filesystem loader for `~/.rein/skills/` and `<workspace>/.rein/skills/`.
pub mod loader;
/// Built-in slash command registry, prefix matcher, and skill activation prompts.
pub mod registry;

pub use loader::load_slash_registry;
pub use registry::{
    BuiltinAction, CommandSource, SlashCommand, SlashKind, SlashRegistry, SlashResolution,
};
