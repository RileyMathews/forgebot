use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Output;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tracing::{debug, warn};

use crate::session::env_loader_errors::{EnvLoaderError, Result};

/// Load environment variables based on the specified loader type.
///
/// # Arguments
/// * `loader_type` - The environment loader type: "none", "direnv", or "nix"
/// * `worktree_path` - The path to the worktree directory where the command should run
///
/// # Returns
/// A HashMap of environment variable names to values.
///
/// # Errors
/// Returns an error if:
/// - The loader type is invalid
/// - The command is not found on PATH
/// - The command exits non-zero
/// - The output cannot be parsed as JSON
/// - For "nix": the 60-second timeout is exceeded
pub async fn load_env(loader_type: &str, worktree_path: &Path) -> Result<HashMap<String, String>> {
    debug!(
        "Loading environment with loader '{}' in worktree: {}",
        loader_type,
        worktree_path.display()
    );

    match loader_type {
        "none" => Ok(load_env_none()),
        "direnv" => load_env_direnv(worktree_path).await,
        "nix" => load_env_nix(worktree_path).await,
        other => Err(EnvLoaderError::InvalidLoaderType(other.to_string())),
    }
}

/// Load environment using "none" strategy - returns empty map.
fn load_env_none() -> HashMap<String, String> {
    debug!("Using 'none' env loader - returning empty environment");
    HashMap::new()
}

/// Load environment using direnv.
///
/// Runs `direnv export json` in the worktree and parses the output.
async fn load_env_direnv(worktree_path: &Path) -> Result<HashMap<String, String>> {
    debug!("Running direnv export json in: {}", worktree_path.display());

    let output = run_command_with_timeout(
        Command::new("direnv")
            .arg("export")
            .arg("json")
            .current_dir(worktree_path),
        30, // 30 second timeout for direnv
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EnvLoaderError::CommandFailed {
            command: "direnv export json",
            exit_code: output.status.code(),
            stderr: stderr.to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_direnv_json(&stdout)
}

/// Parse direnv JSON output into environment variables.
///
/// Expected format: {"PATH": "...", "VAR": "value", ...}
pub fn parse_direnv_json(output: &str) -> Result<HashMap<String, String>> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        // Empty output is valid for direnv (no env changes)
        debug!("direnv output is empty, returning empty environment");
        return Ok(HashMap::new());
    }

    // Direnv outputs a flat object of string values
    let env_vars: HashMap<String, String> =
        serde_json::from_str(trimmed).map_err(|source| EnvLoaderError::ParseJson {
            loader: "direnv",
            output: trimmed.to_string(),
            source,
        })?;

    debug!(
        "Parsed {} environment variables from direnv output",
        env_vars.len()
    );
    Ok(env_vars)
}

/// Load environment using nix.
///
/// Runs `nix print-dev-env --json` in the worktree and extracts exported string variables.
async fn load_env_nix(worktree_path: &Path) -> Result<HashMap<String, String>> {
    debug!(
        "Running nix print-dev-env --json in: {}",
        worktree_path.display()
    );

    let output = run_command_with_timeout(
        Command::new("nix")
            .arg("print-dev-env")
            .arg("--json")
            .current_dir(worktree_path),
        60, // 60 second timeout for nix (per spec)
    )
    .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(EnvLoaderError::CommandFailed {
            command: "nix print-dev-env --json",
            exit_code: output.status.code(),
            stderr: stderr.to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_nix_json(&stdout)
}

/// Nix variable entry with type information.
#[derive(Debug, Deserialize)]
struct NixVariable {
    #[serde(rename = "type")]
    var_type: String,
    value: Option<serde_json::Value>,
}

/// Nix environment output structure.
#[derive(Debug, Deserialize)]
struct NixEnvOutput {
    variables: HashMap<String, NixVariable>,
}

/// Parse nix JSON output into environment variables.
///
/// Extracts only entries where `type == "exported"` and `value` is a plain string.
pub fn parse_nix_json(output: &str) -> Result<HashMap<String, String>> {
    let trimmed = output.trim();

    let nix_output: NixEnvOutput =
        serde_json::from_str(trimmed).map_err(|source| EnvLoaderError::ParseJson {
            loader: "nix",
            output: trimmed.to_string(),
            source,
        })?;

    let mut env_vars = HashMap::new();

    for (name, var) in &nix_output.variables {
        // Only extract exported variables
        if var.var_type != "exported" {
            debug!(
                "Skipping non-exported nix variable: {} (type: {})",
                name, var.var_type
            );
            continue;
        }

        // Only extract string values (not arrays or functions)
        match &var.value {
            Some(serde_json::Value::String(s)) => {
                env_vars.insert(name.clone(), s.clone());
            }
            Some(other) => {
                warn!(
                    "Skipping nix variable '{}' with non-string value: {:?}",
                    name, other
                );
            }
            None => {
                warn!("Skipping nix variable '{}' with no value", name);
            }
        }
    }

    debug!(
        "Parsed {} environment variables from nix output (out of {} total)",
        env_vars.len(),
        nix_output.variables.len()
    );
    Ok(env_vars)
}

/// Run a command with a timeout.
///
/// # Arguments
/// * `cmd` - The command to run (passed as mutable reference for configuration)
/// * `timeout_secs` - Maximum time to wait in seconds
///
/// # Returns
/// The command output on success, or an error if the command times out or fails.
async fn run_command_with_timeout(cmd: &mut Command, timeout_secs: u64) -> Result<Output> {
    let timeout_duration = Duration::from_secs(timeout_secs);

    let result = timeout(timeout_duration, cmd.output()).await;

    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(source)) => Err(EnvLoaderError::CommandExecution {
            command: "subprocess",
            source,
        }),
        Err(_) => Err(EnvLoaderError::Timeout(timeout_secs)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ============================================================================
    // parse_direnv_json tests
    // ============================================================================

    #[test]
    fn test_parse_direnv_json_empty() {
        let result = parse_direnv_json("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_direnv_json_whitespace_only() {
        let result = parse_direnv_json("   \n\t  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_direnv_json_simple_vars() {
        let json = r#"{"PATH": "/usr/local/bin:/usr/bin", "FOO": "bar"}"#;
        let result = parse_direnv_json(json).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(
            result.get("PATH"),
            Some(&"/usr/local/bin:/usr/bin".to_string())
        );
        assert_eq!(result.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_parse_direnv_json_special_chars() {
        let json = r#"{"VAR": "value with spaces", "QUOTE": "value \"quoted\" here"}"#;
        let result = parse_direnv_json(json).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result.get("VAR"), Some(&"value with spaces".to_string()));
        assert_eq!(
            result.get("QUOTE"),
            Some(&"value \"quoted\" here".to_string())
        );
    }

    #[test]
    fn test_parse_direnv_json_invalid_json() {
        let json = "not valid json";
        let result = parse_direnv_json(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse direnv JSON"));
    }

    #[test]
    fn test_parse_direnv_json_non_object() {
        // JSON array instead of object
        let json = r#"["PATH", "/usr/bin"]"#;
        let result = parse_direnv_json(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_direnv_json_non_string_values() {
        // JSON with non-string values should fail
        let json = r#"{"PATH": "/usr/bin", "NUM": 123}"#;
        let result = parse_direnv_json(json);
        assert!(result.is_err());
    }

    // ============================================================================
    // parse_nix_json tests
    // ============================================================================

    #[test]
    fn test_parse_nix_json_empty() {
        let json = r#"{"variables": {}}"#;
        let result = parse_nix_json(json).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_nix_json_exported_vars() {
        let json = r#"{
            "variables": {
                "PATH": {"type": "exported", "value": "/nix/store/.../bin:/usr/bin"},
                "PKG_CONFIG_PATH": {"type": "exported", "value": "/nix/store/.../lib/pkgconfig"},
                "EDITOR": {"type": "exported", "value": "vim"}
            }
        }"#;
        let result = parse_nix_json(json).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains_key("PATH"));
        assert!(result.contains_key("PKG_CONFIG_PATH"));
        assert!(result.contains_key("EDITOR"));
    }

    #[test]
    fn test_parse_nix_json_filters_non_exported() {
        let json = r#"{
            "variables": {
                "PATH": {"type": "exported", "value": "/usr/bin"},
                "buildInputs": {"type": "array", "value": ["/nix/store/abc"]},
                "mkDerivation": {"type": "function", "value": null},
                "nativeBuildInputs": {"type": "array", "value": []}
            }
        }"#;
        let result = parse_nix_json(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("PATH"), Some(&"/usr/bin".to_string()));
        assert!(!result.contains_key("buildInputs"));
        assert!(!result.contains_key("mkDerivation"));
        assert!(!result.contains_key("nativeBuildInputs"));
    }

    #[test]
    fn test_parse_nix_json_filters_non_string_values() {
        let json = r#"{
            "variables": {
                "PATH": {"type": "exported", "value": "/usr/bin"},
                "ARRAY_VAR": {"type": "exported", "value": ["a", "b"]},
                "NULL_VAR": {"type": "exported", "value": null},
                "NUMBER_VAR": {"type": "exported", "value": 42},
                "BOOL_VAR": {"type": "exported", "value": true}
            }
        }"#;
        let result = parse_nix_json(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result.get("PATH"), Some(&"/usr/bin".to_string()));
        assert!(!result.contains_key("ARRAY_VAR"));
        assert!(!result.contains_key("NULL_VAR"));
        assert!(!result.contains_key("NUMBER_VAR"));
        assert!(!result.contains_key("BOOL_VAR"));
    }

    #[test]
    fn test_parse_nix_json_missing_value() {
        let json = r#"{
            "variables": {
                "PATH": {"type": "exported", "value": "/usr/bin"},
                "MISSING": {"type": "exported"}
            }
        }"#;
        let result = parse_nix_json(json).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("PATH"));
        assert!(!result.contains_key("MISSING"));
    }

    #[test]
    fn test_parse_nix_json_invalid_json() {
        let json = "not valid json";
        let result = parse_nix_json(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("failed to parse nix JSON"));
    }

    #[test]
    fn test_parse_nix_json_missing_variables() {
        let json = r#"{"someOtherField": {}}"#;
        let result = parse_nix_json(json);
        assert!(result.is_err());
    }

    // ============================================================================
    // load_env_none tests
    // ============================================================================

    #[test]
    fn test_load_env_none() {
        let result = load_env_none();
        assert!(result.is_empty());
    }

    // ============================================================================
    // load_env dispatcher tests (async)
    // ============================================================================

    #[tokio::test]
    async fn test_load_env_dispatcher_none() {
        let worktree = PathBuf::from("/tmp/test");
        let result = load_env("none", &worktree).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_load_env_dispatcher_invalid_type() {
        let worktree = PathBuf::from("/tmp/test");
        let result = load_env("invalid", &worktree).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid env_loader type"));
    }

    // ============================================================================
    // run_command_with_timeout tests
    // ============================================================================

    #[tokio::test]
    async fn test_run_command_with_timeout_success() {
        // Use 'echo' which should always work
        let mut cmd = Command::new("echo");
        cmd.arg("hello world");

        let output = run_command_with_timeout(&mut cmd, 5).await.unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello world"));
    }

    #[tokio::test]
    async fn test_run_command_with_timeout_nonexistent_command() {
        let mut cmd = Command::new("this_command_definitely_does_not_exist_12345");

        let result = run_command_with_timeout(&mut cmd, 5).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_command_with_timeout_long_command() {
        // 'sleep 0.1' with 1 second timeout should succeed
        let mut cmd = Command::new("sleep");
        cmd.arg("0.1");

        let result = run_command_with_timeout(&mut cmd, 1).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_command_with_timeout_exceeded() {
        // 'sleep 5' with 0.1 second timeout should fail
        let mut cmd = Command::new("sleep");
        cmd.arg("5");

        let result = run_command_with_timeout(&mut cmd, 0).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timed out"));
    }

    // ============================================================================
    // Real-world-like JSON examples
    // ============================================================================

    #[test]
    fn test_parse_direnv_json_realistic() {
        // Simulated real direnv output
        let json = r#"{
            "PATH": "/home/user/project/.direnv/bin:/usr/local/bin:/usr/bin:/bin",
            "DIRENV_DIR": "-/home/user/project",
            "DIRENV_WATCHES": "eJx...base64...",
            "VIRTUAL_ENV": "/home/user/project/.venv",
            "PYTHONPATH": "/home/user/project/src"
        }"#;
        let result = parse_direnv_json(json).unwrap();
        assert_eq!(result.len(), 5);
        assert!(result.contains_key("VIRTUAL_ENV"));
        assert!(result.contains_key("PYTHONPATH"));
    }

    #[test]
    fn test_parse_nix_json_realistic() {
        // Simulated real nix print-dev-env output
        let json = r#"{
            "variables": {
                "PATH": {"type": "exported", "value": "/nix/store/abc123-bash/bin:/usr/bin"},
                "XDG_CONFIG_DIRS": {"type": "exported", "value": "/nix/store/xyz789/etc/xdg"},
                "buildInputs": {"type": "array", "value": ["/nix/store/pkg1", "/nix/store/pkg2"]},
                "builder": {"type": "function", "value": null},
                "name": {"type": "exported", "value": "my-env"},
                "system": {"type": "exported", "value": "x86_64-linux"},
                "nativeBuildInputs": {"type": "array", "value": ["/nix/store/tool1"]},
                "shellHook": {"type": "derivation", "value": "/nix/store/123-hook"},
                "passthru": {"type": "attribute set", "value": null}
            }
        }"#;
        let result = parse_nix_json(json).unwrap();
        // Only exported string variables
        assert_eq!(result.len(), 4);
        assert!(result.contains_key("PATH"));
        assert!(result.contains_key("XDG_CONFIG_DIRS"));
        assert!(result.contains_key("name"));
        assert!(result.contains_key("system"));
        // Non-exported types excluded
        assert!(!result.contains_key("buildInputs"));
        assert!(!result.contains_key("builder"));
        assert!(!result.contains_key("nativeBuildInputs"));
        assert!(!result.contains_key("shellHook"));
        assert!(!result.contains_key("passthru"));
    }
}
