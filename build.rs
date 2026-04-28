use std::process::Command;

fn main() {
    let git_sha = run_git(["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_owned());
    let status = run_git(["status", "--porcelain"]).unwrap_or_default();
    let dirty_suffix = if status.trim().is_empty() {
        ""
    } else {
        "-dirty"
    };
    let build_date = current_utc_date();

    println!("cargo:rustc-env=CGDIFF_GIT_SHA={git_sha}{dirty_suffix}");
    println!("cargo:rustc-env=CGDIFF_BUILD_DATE={build_date}");
}

fn run_git<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn current_utc_date() -> String {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .and_then(|output| output.status.success().then_some(output.stdout))
        .and_then(|stdout| String::from_utf8(stdout).ok());

    output
        .as_deref()
        .map(str::trim)
        .filter(|date| !date.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "unknown-date".to_owned())
}
