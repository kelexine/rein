// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Public exports for the flag/option/argument-aware Shell/Bash effect classifier

/// Ordered system effect levels, target scopes, and classification results.
pub mod effect;
/// Shell quote/escape/ANSI-C lexer, brace expander, and substitution scanner.
pub mod lexer;
/// Compound shell parser handling redirections, variables, control flow, and flags.
pub mod parser;
/// Command dispatcher, env-var guard, wrapper recursion, and core file rules.
pub mod rules;
/// Path target scope analyzer (`workspace` vs `outside_workspace` vs `protected`).
pub mod scope;
/// High-level ShellClassifier orchestrating pipeline and stage analysis.
pub mod shell;
/// Tool-specific flag, option, and embedded script analyzers.
pub mod tools;

pub use effect::{EffectLevel, ShellClassification, TargetScope};
pub use parser::{ParsedCommand, Redirection, ShellPipeline};
pub use rules::{classify_parsed_command, has_dangerous_env_var};
pub use scope::{classify_args_scope, classify_path_token};
pub use shell::ShellClassifier;
