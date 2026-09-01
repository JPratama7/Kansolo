use serde::{Deserialize, Serialize};

/// Error codes used across all Tauri commands and the CLI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AcpErrorCode {
    Internal,
    NotFound,
    Locked,
    Validation,
    Conflict,
    /// Delete blocked because the agent still has run rows. The agent
    /// name is included in `message`; callers can also pass
    /// `delete_runs=true` to cascade.
    AgentHasRuns,
}

/// Typed error returned by all Tauri commands and CLI operations.
/// Serializes to `{ code: "internal" | "notFound" | ... , message: "..." }`
/// so the TS side can `switch(e.code)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpError {
    pub code: AcpErrorCode,
    pub message: String,
}

impl AcpError {
    pub fn internal(msg: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::Internal,
            message: msg.to_string(),
        }
    }
    pub fn not_found(msg: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::NotFound,
            message: msg.to_string(),
        }
    }
    pub fn locked(msg: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::Locked,
            message: msg.to_string(),
        }
    }
    pub fn validation(msg: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::Validation,
            message: msg.to_string(),
        }
    }
    pub fn conflict(msg: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::Conflict,
            message: msg.to_string(),
        }
    }
    /// Delete blocked by existing runs. `name` is the agent name; the
    /// caller should suggest `delete_runs=true` to cascade.
    pub fn agent_has_runs(name: impl ToString) -> Self {
        Self {
            code: AcpErrorCode::AgentHasRuns,
            message: format!(
                "Cannot delete agent '{}': agent_runs still exist. Pass delete_runs=true to cascade.",
                name.to_string()
            ),
        }
    }
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)
    }
}

impl std::error::Error for AcpError {}

/// Convert `Result<T, String>` errors from DB and utility calls into
/// `Result<T, AcpError>` so callers can use `?` uniformly.
impl From<String> for AcpError {
    fn from(s: String) -> Self {
        AcpError::internal(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let err = AcpError::locked("card is busy");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"code\":\"locked\""));
        assert!(json.contains("\"message\":\"card is busy\""));
        let back: AcpError = serde_json::from_str(&json).unwrap();
        assert_eq!(back.code, AcpErrorCode::Locked);
        assert_eq!(back.message, "card is busy");
    }

    #[test]
    fn all_codes_serialize_camel_case() {
        for (code, expected) in [
            (AcpErrorCode::Internal, "internal"),
            (AcpErrorCode::NotFound, "notFound"),
            (AcpErrorCode::Locked, "locked"),
            (AcpErrorCode::Validation, "validation"),
            (AcpErrorCode::Conflict, "conflict"),
            (AcpErrorCode::AgentHasRuns, "agentHasRuns"),
        ] {
            let err = AcpError {
                code: code.clone(),
                message: "x".into(),
            };
            let json = serde_json::to_string(&err).unwrap();
            assert!(
                json.contains(&format!("\"code\":\"{expected}\"")),
                "got: {json}"
            );
        }
    }
}
