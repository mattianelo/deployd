use std::sync::OnceLock;

use regex::Regex;

#[derive(Debug, Clone)]
pub enum Rule {
    /// Match a path prefix via regex; replace the matched portion.
    Prefix { pattern: Regex, replacement: String },
}

impl Rule {
    /// Apply this rule to a relative path.
    /// Returns `Some(mapped_path)` if the rule matched, `None` otherwise.
    pub fn apply(&self, rel_path: &str) -> Option<String> {
        match self {
            Rule::Prefix {
                pattern,
                replacement,
            } => {
                if pattern.is_match(rel_path) {
                    Some(pattern.replace(rel_path, replacement.as_str()).to_string())
                } else {
                    None
                }
            }
        }
    }
}

/// Apply the first matching rule, or return the path unmodified.
pub fn apply_rules(rules: &[Rule], rel_path: &str) -> String {
    for rule in rules {
        if let Some(mapped) = rule.apply(rel_path) {
            return mapped;
        }
    }
    rel_path.to_string()
}

// Compiled once; Regex::clone() is O(1) (internally Arc-wrapped).
static DATA_PREFIX_RE: OnceLock<Regex> = OnceLock::new();

fn data_prefix_re() -> &'static Regex {
    DATA_PREFIX_RE.get_or_init(|| Regex::new(r"(?i)^data/").expect("invalid data prefix regex"))
}

pub fn rules_for_game(game_id: &str) -> Vec<Rule> {
    match game_id {
        // Strip redundant Data/ prefix. Everything else stays as-is since
        // we deploy relative to the game's Data directory already.
        "skyrimse" | "skyrimse-steam" | "skyrimvr" | "fallout4" | "fallout4-steam"
        | "falloutnv" | "falloutnv-steam" | "starfield" => {
            vec![Rule::Prefix {
                pattern: data_prefix_re().clone(),
                replacement: String::new(),
            }]
        }
        _ => vec![],
    }
}
