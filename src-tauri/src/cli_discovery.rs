use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn first_valid_candidate<T, E>(
    candidates: impl IntoIterator<Item = PathBuf>,
    mut inspect: impl FnMut(&Path) -> Result<T, E>,
) -> Result<Option<T>, E> {
    let mut last_error = None;
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        match inspect(&path) {
            Ok(value) => return Ok(Some(value)),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) => Err(error),
        None => Ok(None),
    }
}

pub fn first_valid_candidate_across_sources<T, E>(
    primary_candidates: impl IntoIterator<Item = PathBuf>,
    secondary_candidates: impl FnOnce() -> Result<Vec<PathBuf>, E>,
    mut inspect: impl FnMut(&Path) -> Result<T, E>,
) -> Result<Option<T>, E> {
    let mut seen = HashSet::new();
    let primary_candidates = primary_candidates
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect::<Vec<_>>();
    let primary_result = first_valid_candidate(primary_candidates, &mut inspect);
    if matches!(primary_result, Ok(Some(_))) {
        return primary_result;
    }

    let secondary_candidates = match secondary_candidates() {
        Ok(candidates) => candidates
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .collect::<Vec<_>>(),
        Err(_) => return primary_result,
    };
    match first_valid_candidate(secondary_candidates, inspect) {
        Ok(None) => primary_result,
        result => result,
    }
}

pub fn version_command(path: &Path) -> io::Result<Command> {
    let mut command = Command::new(path);
    if let Some(parent) = path.parent() {
        let mut directories = vec![parent.to_path_buf()];
        if let Some(current) = env::var_os("PATH") {
            directories.extend(env::split_paths(&current));
        }
        let path = env::join_paths(directories).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("build CLI inspection PATH: {error}"),
            )
        })?;
        command.env("PATH", path);
    }
    Ok(command)
}

pub fn node_cli_candidates_from_home(home: impl AsRef<Path>, executable: &str) -> Vec<PathBuf> {
    let home = home.as_ref();
    let mut candidates = vec![
        home.join(".local/bin").join(executable),
        home.join(".npm-global/bin").join(executable),
        home.join("Library/pnpm").join(executable),
        home.join(".bun/bin").join(executable),
        home.join(".volta/bin").join(executable),
        home.join(".asdf/shims").join(executable),
        home.join(".local/share/mise/shims").join(executable),
    ];
    if let Ok(entries) = fs::read_dir(home.join(".nvm/versions/node")) {
        let mut nvm = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("bin").join(executable))
            .collect::<Vec<_>>();
        nvm.sort();
        nvm.reverse();
        candidates.extend(nvm);
    }
    candidates
}

pub fn login_shell_candidates(executable: &str) -> io::Result<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        let _ = executable;
        return Ok(Vec::new());
    }
    #[cfg(not(windows))]
    {
        let shell = env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        login_shell_candidates_with(shell, executable)
    }
}

pub fn login_shell_candidates_with(
    shell: impl AsRef<Path>,
    executable: &str,
) -> io::Result<Vec<PathBuf>> {
    if executable.is_empty()
        || !executable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CLI discovery executable name is invalid",
        ));
    }
    let command = format!("which -a {executable}");
    let mut child = Command::new(shell.as_ref())
        .args(["-lic", &command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + SHELL_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            let mut seen = HashSet::new();
            let paths = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(str::trim)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute() && path.is_file())
                .filter(|path| seen.insert(path.clone()))
                .collect();
            return Ok(paths);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "login shell CLI discovery timed out",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}
