//! Non-interactive, fast-forward-only synchronization of an existing checkout.
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const SYNC_TIMEOUT: Duration = Duration::from_secs(180);
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct GitSyncResult {
    pub repo_root: PathBuf,
    pub before_commit: String,
    pub after_commit: String,
    pub updated: bool,
}

/// Synchronize the current branch without stashing, rebasing or resetting files.
/// All child processes share one deadline, and diagnostics never expose remote URLs.
pub fn pull_repository(path: &Path, cancel: &AtomicBool) -> Result<GitSyncResult, String> {
    pull_with_timeout(path, cancel, SYNC_TIMEOUT)
}

fn pull_with_timeout(
    path: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<GitSyncResult, String> {
    if !path.is_dir() {
        return Err("Git 拉取失败：仓库目录不存在或无法访问。".into());
    }
    let deadline = Instant::now() + timeout;
    let root = git(path, &["rev-parse", "--show-toplevel"], cancel, deadline)?;
    if !root.status.success() {
        return Err("Git 拉取失败：所选目录不属于可用的 Git 工作区。".into());
    }
    let repo_root = PathBuf::from(output_line(&root.stdout));
    let branch = git(
        &repo_root,
        &["symbolic-ref", "--quiet", "HEAD"],
        cancel,
        deadline,
    )?;
    if !branch.status.success() {
        return Err("Git 拉取失败：当前处于游离 HEAD 状态，请先切换到跟踪远程的分支。".into());
    }
    let before = git(
        &repo_root,
        &["rev-parse", "--verify", "HEAD"],
        cancel,
        deadline,
    )?;
    if !before.status.success() {
        return Err("Git 拉取失败：当前分支尚无提交。".into());
    }
    let upstream = git(
        &repo_root,
        &["rev-parse", "--verify", "@{upstream}"],
        cancel,
        deadline,
    )?;
    if !upstream.status.success() {
        return Err("Git 拉取失败：当前分支未配置有效的上游分支，请先设置远程跟踪关系。".into());
    }
    let status = git(
        &repo_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
        cancel,
        deadline,
    )?;
    if !status.status.success() {
        return Err("Git 拉取失败：无法读取工作区状态。".into());
    }
    if !status.stdout.is_empty() {
        return Err("Git 拉取已停止：已跟踪文件存在本地修改，请先提交或自行处理后重试。".into());
    }
    let pull = git(
        &repo_root,
        &[
            "pull",
            "--ff-only",
            "--no-rebase",
            "--no-autostash",
            "--no-edit",
        ],
        cancel,
        deadline,
    )?;
    if !pull.status.success() {
        return Err(pull_error(&pull.stderr));
    }
    let after = git(
        &repo_root,
        &["rev-parse", "--verify", "HEAD"],
        cancel,
        deadline,
    )?;
    if !after.status.success() {
        return Err("Git 已完成拉取，但无法读取更新后的提交，请检查仓库。".into());
    }
    let before_commit = output_line(&before.stdout);
    let after_commit = output_line(&after.stdout);
    Ok(GitSyncResult {
        repo_root,
        updated: before_commit != after_commit,
        before_commit,
        after_commit,
    })
}

fn output_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

// Only fixed, classified messages leave this module. Git diagnostics can contain
// URLs, credential-helper output, branch names and other private input.
fn pull_error(stderr: &[u8]) -> String {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    if message.contains("not possible to fast-forward") || message.contains("divergent branches") {
        "Git 拉取已停止：本地与上游分支已分叉，无法仅快进更新，请手动处理分支后重试。"
    } else if message.contains("would be overwritten") || message.contains("local changes") {
        "Git 拉取已停止：远程更新会覆盖本地文件，请先自行处理本地文件后重试。"
    } else if message.contains("authentication failed")
        || message.contains("could not read username")
        || message.contains("terminal prompts disabled")
        || message.contains("permission denied")
    {
        "Git 拉取失败：远程认证失败或需要交互登录，请先在本机配置 Git 凭据或 SSH 密钥。"
    } else if message.contains("could not resolve")
        || message.contains("unable to access")
        || message.contains("connection timed out")
        || message.contains("connection refused")
    {
        "Git 拉取失败：无法连接远程仓库，请检查网络、代理和远程地址。"
    } else if message.contains("couldn't find remote ref")
        || message.contains("no such ref")
        || message.contains("no such ref was fetched")
    {
        "Git 拉取失败：远程上游分支不存在或无法获取，请检查分支跟踪配置。"
    } else {
        "Git 拉取失败：远程不可用或仓库状态不允许快进更新，请在本机 Git 中检查后重试。"
    }
    .into()
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn git(
    path: &Path,
    args: &[&str],
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<ProcessOutput, String> {
    let mut command = Command::new("git");
    command
        .current_dir(path)
        .args([
            "-c",
            "core.askPass=",
            "-c",
            "credential.interactive=false",
            "-c",
            "merge.autoStash=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=20")
        .env("GIT_SSH_VARIANT", "ssh")
        .env("LC_ALL", "C")
        .env("LANG", "C");
    // An inherited Git environment must not redirect the selected checkout.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(variable);
    }
    run_command(command, cancel, deadline)
}

fn drain(mut reader: impl Read) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(0) | Err(_) => break,
            Ok(length) => {
                let keep = length.min(OUTPUT_LIMIT.saturating_sub(captured.len()));
                captured.extend_from_slice(&buffer[..keep]);
            }
        }
    }
    captured
}

fn run_command(
    mut command: Command,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<ProcessOutput, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Git 拉取已取消。".into());
    }
    if Instant::now() >= deadline {
        return Err("Git 拉取超时，请检查网络或仓库状态后重试。".into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Git 拉取失败：未找到 Git，请安装 Git 并确认已加入 PATH。"
        } else {
            "Git 拉取失败：无法启动 Git 进程，请检查本机权限和 Git 安装。"
        }
        .to_owned()
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_thread = thread::spawn(move || drain(stdout));
    let err_thread = thread::spawn(move || drain(stderr));
    let mut process_status = None;
    let outcome = loop {
        if process_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => process_status = Some(status),
                Ok(None) => {}
                Err(_) => break Err("Git 拉取失败：无法读取 Git 进程状态。".to_owned()),
            }
        }
        // A helper can retain an output pipe after Git exits. Keep the deadline
        // active until both pipes have closed, rather than blocking in join().
        if let Some(status) = process_status {
            if out_thread.is_finished() && err_thread.is_finished() {
                break Ok(status);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            break Err("Git 拉取已取消；如取消发生在更新期间，请检查仓库状态。".to_owned());
        }
        if Instant::now() >= deadline {
            break Err("Git 拉取超时；如超时发生在更新期间，请检查仓库状态后重试。".to_owned());
        }
        thread::sleep(Duration::from_millis(25));
    };
    if outcome.is_err() {
        kill_process_tree(child.id());
        let _ = child.kill();
        let _ = child.wait();
    }
    // Do not wait indefinitely for a detached credential helper holding pipes.
    // On error the process group/tree has been terminated; readers finish when
    // its handles close, and detached reader threads cannot block cancellation.
    let status = outcome?;
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
    })
}

fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Repositories {
        _temp: TempDir,
        source: PathBuf,
        target: PathBuf,
    }
    fn git_ok(path: &Path, args: &[&str]) {
        // A real empty file works on Windows too; Git for Windows rejects NUL
        // as GIT_CONFIG_GLOBAL on some releases.
        let isolated = tempfile::tempdir().unwrap();
        let empty_config = isolated.path().join("empty-git-config");
        fs::write(&empty_config, "").unwrap();
        let output = Command::new("git")
            .current_dir(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &empty_config)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn commit(path: &Path, file: &str, content: &str) {
        fs::write(path.join(file), content).unwrap();
        git_ok(path, &["add", file]);
        git_ok(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
    }
    fn fixture() -> Repositories {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        git_ok(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
        git_ok(
            temp.path(),
            &["clone", remote.to_str().unwrap(), source.to_str().unwrap()],
        );
        commit(&source, "config.txt", "one\n");
        git_ok(&source, &["push", "-u", "origin", "HEAD"]);
        git_ok(
            temp.path(),
            &["clone", remote.to_str().unwrap(), target.to_str().unwrap()],
        );
        Repositories {
            _temp: temp,
            source,
            target,
        }
    }
    #[test]
    fn pulls_remote_file_and_reports_unchanged() {
        let repo = fixture();
        commit(&repo.source, "new.txt", "new remote file\n");
        git_ok(&repo.source, &["push"]);
        let result = pull_repository(&repo.target, &AtomicBool::new(false)).unwrap();
        assert!(result.updated);
        assert_ne!(result.before_commit, result.after_commit);
        assert_eq!(
            fs::read_to_string(repo.target.join("new.txt")).unwrap(),
            "new remote file\n"
        );
        assert_eq!(
            result.repo_root.canonicalize().unwrap(),
            repo.target.canonicalize().unwrap()
        );
        let unchanged = pull_repository(&repo.target, &AtomicBool::new(false)).unwrap();
        assert!(!unchanged.updated);
        assert_eq!(unchanged.before_commit, unchanged.after_commit);
    }
    #[test]
    fn rejects_dirty_tracked_files_without_overwriting() {
        let repo = fixture();
        fs::write(repo.target.join("config.txt"), "local change").unwrap();
        let error = pull_repository(&repo.target, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("本地修改"), "{error}");
        assert_eq!(
            fs::read_to_string(repo.target.join("config.txt")).unwrap(),
            "local change"
        );
    }
    #[test]
    fn rejects_divergence_without_resetting_local_commit() {
        let repo = fixture();
        commit(&repo.target, "local.txt", "local commit");
        commit(&repo.source, "remote.txt", "remote commit");
        git_ok(&repo.source, &["push"]);
        let before = Command::new("git")
            .current_dir(&repo.target)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;
        let error = pull_repository(&repo.target, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("分叉"), "{error}");
        let after = Command::new("git")
            .current_dir(&repo.target)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(before, after);
        assert!(!repo.target.join("remote.txt").exists());
    }
    #[test]
    fn rejects_missing_upstream_and_non_repository() {
        let repo = fixture();
        git_ok(&repo.target, &["branch", "--unset-upstream"]);
        assert!(pull_repository(&repo.target, &AtomicBool::new(false))
            .unwrap_err()
            .contains("上游"));
        assert!(pull_repository(repo._temp.path(), &AtomicBool::new(false))
            .unwrap_err()
            .contains("工作区"));
    }
    #[test]
    fn cancellation_and_timeout_are_bounded() {
        let repo = fixture();
        assert!(pull_repository(&repo.target, &AtomicBool::new(true))
            .unwrap_err()
            .contains("取消"));
        assert!(
            pull_with_timeout(&repo.target, &AtomicBool::new(false), Duration::ZERO)
                .unwrap_err()
                .contains("超时")
        );
        let cancel = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| {
                thread::sleep(Duration::from_millis(150));
                cancel.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            let error = run_command(sleeper(), &cancel, start + Duration::from_secs(10))
                .err()
                .unwrap();
            assert!(error.contains("取消"));
            assert!(start.elapsed() < Duration::from_secs(3));
        });
        let start = Instant::now();
        let error = run_command(
            sleeper(),
            &AtomicBool::new(false),
            start + Duration::from_millis(150),
        )
        .err()
        .unwrap();
        assert!(error.contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    fn sleeper() -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "git_sync::tests::process_test_helper",
                "--nocapture",
            ])
            .env("REVIEWER_GIT_SYNC_SLEEP_HELPER", "1");
        command
    }
    #[cfg(unix)]
    #[test]
    fn timeout_includes_pipes_held_after_git_process_exits() {
        let mut command = sleeper();
        command.env("REVIEWER_GIT_SYNC_PIPE_HELPER", "1");
        let start = Instant::now();
        let error = run_command(
            command,
            &AtomicBool::new(false),
            start + Duration::from_millis(200),
        )
        .err()
        .unwrap();
        assert!(error.contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn process_test_helper() {
        if std::env::var_os("REVIEWER_GIT_SYNC_PIPE_HELPER").is_some() {
            let mut command = sleeper();
            command.env_remove("REVIEWER_GIT_SYNC_PIPE_HELPER");
            // Deliberately exit before this descendant: it must keep stdout open
            // so the outer test exercises the deadline after the parent exits.
            // run_command kills the entire test process group on that deadline.
            #[allow(clippy::zombie_processes)]
            let _child = command.spawn().unwrap();
            return;
        }
        if std::env::var_os("REVIEWER_GIT_SYNC_SLEEP_HELPER").is_some() {
            thread::sleep(Duration::from_secs(30));
        }
    }
    #[test]
    fn errors_do_not_disclose_credentials_or_urls() {
        let error = pull_error(b"fatal: Authentication failed for 'https://alice:secret-token@example.invalid/private.git'");
        assert!(error.contains("认证"));
        for private in ["alice", "secret-token", "example.invalid", "private.git"] {
            assert!(!error.contains(private));
        }
    }
}
