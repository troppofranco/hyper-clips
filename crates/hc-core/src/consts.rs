//! Single place for project-wide constants (updater repo, file names, pipe name).

pub const APP_NAME: &str = "HyperClips";

/// GitHub repository used by the updater (Phase 5). Change it only here.
pub const REPO_OWNER: &str = "troppofranco";
pub const REPO_NAME: &str = "hyper-clips";

/// File name of the UI executable, expected next to the service executable.
pub const UI_EXE: &str = "hyperclips-ui.exe";

/// Builds the per-user named pipe path used for service <-> UI/CLI IPC.
/// The user name is sanitized so it can never alter the pipe namespace.
pub fn pipe_path(user: &str) -> String {
    let clean: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = if clean.is_empty() {
        "user".to_owned()
    } else {
        clean
    };
    format!(r"\\.\pipe\{APP_NAME}-{clean}")
}

/// Current user name from the environment (`USERNAME` on Windows, `USER` elsewhere).
pub fn current_user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_path_is_sanitized() {
        assert_eq!(pipe_path("mario"), r"\\.\pipe\HyperClips-mario");
        assert_eq!(
            pipe_path(r"..\evil name"),
            r"\\.\pipe\HyperClips-___evil_name"
        );
        assert_eq!(pipe_path(""), r"\\.\pipe\HyperClips-user");
    }
}
