//! Explicit, run-scoped tool decisions shared by every frontend.
use anyhow::{Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Allow,
    #[default]
    Ask,
    Deny,
}

/// Missing entries keep the safe default: reads allowed, mutations ask.
/// An approval authorizes one invocation only and never changes OS boundaries.
#[derive(Clone, Debug, Serialize)]
pub struct ToolPermissions {
    pub read: Permission,
    pub search: Permission,
    pub write: Permission,
    pub edit: Permission,
    pub bash: Permission,
    pub rules: Vec<PermissionRule>,
    #[serde(skip)]
    pub source: String,
}

impl Default for ToolPermissions {
    fn default() -> Self {
        Self {
            read: Permission::Allow,
            search: Permission::Allow,
            write: Permission::Ask,
            edit: Permission::Ask,
            bash: Permission::Ask,
            rules: Vec::new(),
            source: "default".into(),
        }
    }
}

/// Literal workspace path (trailing slash means directory subtree), or an
/// exact complete shell command. Rules are trusted host configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRule {
    pub tool: String,
    pub decision: Permission,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawPermissions {
    read: Permission,
    search: Permission,
    write: Permission,
    edit: Permission,
    bash: Permission,
    rules: Vec<PermissionRule>,
}
impl Default for RawPermissions {
    fn default() -> Self {
        let p = ToolPermissions::default();
        Self {
            read: p.read,
            search: p.search,
            write: p.write,
            edit: p.edit,
            bash: p.bash,
            rules: p.rules,
        }
    }
}
impl<'de> Deserialize<'de> for ToolPermissions {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let p = RawPermissions::deserialize(deserializer)?;
        let p = Self {
            read: p.read,
            search: p.search,
            write: p.write,
            edit: p.edit,
            bash: p.bash,
            rules: p.rules,
            source: "default".into(),
        };
        p.validate().map_err(serde::de::Error::custom)?;
        Ok(p)
    }
}

fn stricter(a: Permission, b: Permission) -> Permission {
    match (a, b) {
        (Permission::Deny, _) | (_, Permission::Deny) => Permission::Deny,
        (Permission::Ask, _) | (_, Permission::Ask) => Permission::Ask,
        _ => Permission::Allow,
    }
}

impl ToolPermissions {
    pub fn validate(&self) -> Result<()> {
        if cfg!(not(target_os = "linux")) && self.rules.iter().any(|r| r.path.is_some()) {
            bail!("path permission scopes require Linux descriptor confinement");
        }
        for rule in &self.rules {
            match (rule.tool.as_str(), &rule.path, &rule.command) {
                ("read" | "write" | "edit", Some(path), None) => {
                    let value = path.strip_suffix('/').unwrap_or(path);
                    if value.is_empty()
                        || value.contains(['\\', ':', '*', '?', '[', ']'])
                        || value
                            .split('/')
                            .any(|p| p.is_empty() || p == "." || p == "..")
                    {
                        bail!(
                            "permission rule path must be a literal workspace-relative file or directory: {path}"
                        );
                    }
                }
                ("bash", None, Some(command)) if !command.trim().is_empty() => {}
                _ => bail!(
                    "permission rule requires read/write/edit + path, or bash + exact command"
                ),
            }
        }
        Ok(())
    }

    pub fn scoped_decision(
        &self,
        tool: &str,
        target: Option<&str>,
        command: Option<&str>,
    ) -> (Permission, Vec<usize>) {
        let mut matched = Vec::new();
        let mut decision = None;
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.tool != tool {
                continue;
            }
            let matches = match (&rule.path, &rule.command) {
                (Some(path), None) => target.is_some_and(|target| {
                    if let Some(dir) = path.strip_suffix('/') {
                        target == dir || target.starts_with(path)
                    } else {
                        target == path
                    }
                }),
                (None, Some(expected)) => command == Some(expected.as_str()),
                _ => false,
            };
            if matches {
                matched.push(index);
                decision =
                    Some(decision.map_or(rule.decision, |prior| stricter(prior, rule.decision)));
            }
        }
        (decision.unwrap_or_else(|| self.decision(tool)), matched)
    }

    pub(crate) fn path_decision(
        &self,
        root: &std::path::Path,
        tool: &str,
        requested: &str,
        resolved: &std::path::Path,
    ) -> Result<(Permission, Vec<usize>)> {
        use std::path::{Component, PathBuf};
        let mut lexical = PathBuf::new();
        for part in root.join(requested).components() {
            match part {
                Component::ParentDir => {
                    lexical.pop();
                }
                Component::CurDir => {}
                p => lexical.push(p.as_os_str()),
            }
        }
        let lexical = lexical
            .strip_prefix(root)?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-UTF-8 permission target"))?;
        let canonical = resolved
            .strip_prefix(root)?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-UTF-8 permission target"))?;
        let (a, mut rules) = self.scoped_decision(tool, Some(lexical), None);
        let (b, other) = self.scoped_decision(tool, Some(canonical), None);
        rules.extend(other);
        rules.sort_unstable();
        rules.dedup();
        Ok((stricter(a, b), rules))
    }

    pub fn with_mutations(permission: Permission) -> Self {
        let mut policy = Self::default();
        policy.set_mutations(permission);
        policy
    }

    pub fn set_mutations(&mut self, permission: Permission) {
        self.write = permission;
        self.edit = permission;
        self.bash = permission;
    }

    /// Explicit allow changes fallback only; ask/deny also constrain scoped
    /// mutations. A CLI flag cannot remove a configured deny rule.
    pub fn apply_approval(&mut self, permission: Permission) {
        self.set_mutations(permission);
        if permission != Permission::Allow {
            for rule in &mut self.rules {
                if matches!(rule.tool.as_str(), "write" | "edit" | "bash") {
                    rule.decision = stricter(permission, rule.decision);
                }
            }
        }
    }

    pub fn decision(&self, tool: &str) -> Permission {
        match tool {
            "read" => self.read,
            "search" => self.search,
            "write" => self.write,
            "edit" => self.edit,
            "bash" => self.bash,
            _ => Permission::Deny,
        }
    }

    pub fn from_env() -> Result<Self> {
        let mut policy = Self::default();
        if let Some(value) = std::env::var_os("HYPER_APPROVAL") {
            let Some(permission) = value.to_str().and_then(|v| match v {
                "allow" => Some(Permission::Allow),
                "ask" => Some(Permission::Ask),
                "deny" => Some(Permission::Deny),
                _ => None,
            }) else {
                bail!("invalid HYPER_APPROVAL; expected allow, ask or deny");
            };
            policy.set_mutations(permission);
            policy.source = "HYPER_APPROVAL".into();
        }
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_config_is_explicit_and_defaults_to_ask() {
        let policy: ToolPermissions =
            serde_json::from_str(r#"{"write":"allow","bash":"deny"}"#).unwrap();
        assert_eq!(policy.write, Permission::Allow);
        assert_eq!(policy.edit, Permission::Ask);
        assert_eq!(policy.read, Permission::Allow);
        assert_eq!(policy.bash, Permission::Deny);
        assert!(serde_json::from_str::<ToolPermissions>(r#"{"wrtie":"allow"}"#).is_err());
        assert!(serde_json::from_str::<ToolPermissions>(r#"{"source":"trusted"}"#).is_err());
        assert!(serde_json::from_str::<ToolPermissions>(r#"{"bash":"always"}"#).is_err());
        assert_eq!(policy.decision("unknown"), Permission::Deny);
    }
    #[test]
    fn invalid_scopes_are_rejected_before_use() {
        for rule in [
            r#"{"tool":"write","path":"../out","decision":"allow"}"#,
            r#"{"tool":"read","path":"/tmp/out","decision":"allow"}"#,
            r#"{"tool":"read","path":"src/*","decision":"allow"}"#,
            r#"{"tool":"write","path":"src//a","decision":"allow"}"#,
            r#"{"tool":"write","path":"src/./a","decision":"allow"}"#,
            r#"{"tool":"write","path":"","decision":"allow"}"#,
            r#"{"tool":"search","path":"src/","decision":"allow"}"#,
            r#"{"tool":"bash","command":" ","decision":"allow"}"#,
            r#"{"tool":"bash","command":"echo ok","path":"src/","decision":"allow"}"#,
            r#"{"tool":"unknown","command":"echo ok","decision":"allow"}"#,
            r#"{"tool":"bash","command":"echo ok","decision":"allow","extra":true}"#,
        ] {
            assert!(
                serde_json::from_str::<ToolPermissions>(&format!(r#"{{"rules":[{rule}]}}"#))
                    .is_err(),
                "{rule}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn overlapping_scopes_are_literal_and_most_restrictive() {
        let p: ToolPermissions = serde_json::from_str(
            r#"{"write":"deny","bash":"deny","rules":[
            {"tool":"write","path":"src/","decision":"allow"},
            {"tool":"write","path":"src/private/","decision":"ask"},
            {"tool":"write","path":"src/private/key","decision":"deny"},
            {"tool":"bash","command":"echo OK","decision":"allow"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(
            p.scoped_decision("write", Some("src/a"), None),
            (Permission::Allow, vec![0])
        );
        assert_eq!(
            p.scoped_decision("write", Some("src/private/a"), None),
            (Permission::Ask, vec![0, 1])
        );
        assert_eq!(
            p.scoped_decision("write", Some("src/private/key"), None),
            (Permission::Deny, vec![0, 1, 2])
        );
        for path in ["src2/a", "Src/a", "x/src/a"] {
            assert_eq!(
                p.scoped_decision("write", Some(path), None).0,
                Permission::Deny
            );
        }
        assert_eq!(
            p.scoped_decision("bash", None, Some("echo OK")).0,
            Permission::Allow
        );
        for command in [
            "echo OK; touch x",
            "echo OK && touch x",
            "echo OK ",
            "echo ok",
        ] {
            assert_eq!(
                p.scoped_decision("bash", None, Some(command)).0,
                Permission::Deny
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn approval_flags_keep_denies_and_can_constrain_scoped_allows() {
        let p: ToolPermissions = serde_json::from_str(
            r#"{"rules":[
            {"tool":"write","path":"src/","decision":"allow"},
            {"tool":"write","path":"src/key","decision":"deny"}
        ]}"#,
        )
        .unwrap();
        for (flag, expected) in [
            (Permission::Allow, Permission::Allow),
            (Permission::Ask, Permission::Ask),
            (Permission::Deny, Permission::Deny),
        ] {
            let mut p = p.clone();
            p.apply_approval(flag);
            assert_eq!(p.scoped_decision("write", Some("src/a"), None).0, expected);
            assert_eq!(
                p.scoped_decision("write", Some("src/key"), None).0,
                Permission::Deny
            );
        }
    }
}
