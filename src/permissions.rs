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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPermissions {
    pub read: Permission,
    pub search: Permission,
    pub write: Permission,
    pub edit: Permission,
    pub bash: Permission,
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
            source: "default".into(),
        }
    }
}

impl ToolPermissions {
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
}
