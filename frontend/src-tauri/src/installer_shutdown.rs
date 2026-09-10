use std::path::{Path, PathBuf};

pub(crate) const INSTALLER_QUIT_ARG: &str = "--installer-quit";
const INSTALLER_TARGET_ARG: &str = "--installer-target";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallerQuitDecision {
    NotRequested,
    RefuseUntrustedTarget,
    RefuseActiveRecording,
    Exit,
}

fn comparable_path(path: &Path) -> Option<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        return Some(path);
    }
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    std::env::current_dir().ok().map(|cwd| cwd.join(path))
}

fn same_executable(left: &Path, right: &Path) -> bool {
    let (Some(left), Some(right)) = (comparable_path(left), comparable_path(right)) else {
        return false;
    };

    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn installer_target(args: &[String]) -> Option<&Path> {
    args.windows(2)
        .find(|pair| pair[0] == INSTALLER_TARGET_ARG)
        .map(|pair| Path::new(&pair[1]))
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix("--installer-target=").map(Path::new))
        })
}

pub(crate) fn installer_quit_decision(
    args: &[String],
    current_exe: &Path,
    recording: bool,
) -> InstallerQuitDecision {
    if !args.iter().any(|arg| arg == INSTALLER_QUIT_ARG) {
        return InstallerQuitDecision::NotRequested;
    }

    // tauri-plugin-single-instance scopes by bundle identity, not by the path
    // of the launched executable. Require both independent pieces of caller
    // evidence so one installation can never shut down another one.
    let Some(invoked_exe) = args.first().map(Path::new) else {
        return InstallerQuitDecision::RefuseUntrustedTarget;
    };
    let Some(target) = installer_target(args) else {
        return InstallerQuitDecision::RefuseUntrustedTarget;
    };
    if !same_executable(invoked_exe, current_exe) || !same_executable(target, current_exe) {
        return InstallerQuitDecision::RefuseUntrustedTarget;
    }

    if recording {
        InstallerQuitDecision::RefuseActiveRecording
    } else {
        InstallerQuitDecision::Exit
    }
}

#[cfg(test)]
mod tests {
    use super::{installer_quit_decision, InstallerQuitDecision};

    fn request_args(target: &std::path::Path) -> Vec<String> {
        vec![
            target.display().to_string(),
            "--installer-quit".into(),
            "--installer-target".into(),
            target.display().to_string(),
        ]
    }

    #[test]
    fn exact_target_exits_only_when_not_recording() {
        let current = std::env::current_exe().unwrap();
        let args = request_args(&current);

        assert_eq!(
            installer_quit_decision(&args, &current, false),
            InstallerQuitDecision::Exit
        );
        assert_eq!(
            installer_quit_decision(&args, &current, true),
            InstallerQuitDecision::RefuseActiveRecording
        );
    }

    #[test]
    fn same_bundle_id_cannot_close_a_different_install_path() {
        let current = std::env::current_exe().unwrap();
        let other = current.with_file_name("other-install.exe");
        let args = request_args(&other);

        assert_eq!(
            installer_quit_decision(&args, &current, false),
            InstallerQuitDecision::RefuseUntrustedTarget
        );
    }

    #[test]
    fn missing_target_proof_is_not_an_installer_quit_request() {
        let current = std::env::current_exe().unwrap();
        let args = vec![current.display().to_string(), "--installer-quit".into()];

        assert_eq!(
            installer_quit_decision(&args, &current, false),
            InstallerQuitDecision::RefuseUntrustedTarget
        );
        assert_eq!(
            installer_quit_decision(&[current.display().to_string()], &current, false),
            InstallerQuitDecision::NotRequested
        );
    }
}
