use clap::Parser;
use regex::Regex;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "qwen-review")]
#[command(about = "Review GitHub PRs using Qwen AI")]
struct Args {
    /// GitHub PR URL (e.g., https://github.com/owner/repo/pull/123)
    pr_url: String,
}

struct PrInfo {
    owner: String,
    repo: String,
    pr_number: u64,
}

fn parse_pr_url(url: &str) -> Result<PrInfo, String> {
    let re = Regex::new(r"github\.com/([^/]+)/([^/]+)/pull/(\d+)").unwrap();

    match re.captures(url) {
        Some(caps) => Ok(PrInfo {
            owner: caps[1].to_string(),
            repo: caps[2].to_string(),
            pr_number: caps[3].parse().unwrap(),
        }),
        None => Err(format!("Invalid GitHub PR URL: {}", url)),
    }
}

fn fetch_pr_diff(pr_info: &PrInfo) -> Result<String, String> {
    let repo = format!("{}/{}", pr_info.owner, pr_info.repo);

    let output = Command::new("gh")
        .args(["pr", "diff", &pr_info.pr_number.to_string(), "-R", &repo])
        .output()
        .map_err(|e| format!("Failed to run gh command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr diff failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn fetch_pr_info(pr_info: &PrInfo) -> Result<String, String> {
    let repo = format!("{}/{}", pr_info.owner, pr_info.repo);

    let output = Command::new("gh")
        .args([
            "pr", "view",
            &pr_info.pr_number.to_string(),
            "-R", &repo,
            "--json", "title,body,author",
        ])
        .output()
        .map_err(|e| format!("Failed to run gh command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr view failed: {}", stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn review_with_qwen(pr_metadata: &str, diff: &str) -> Result<(), String> {
    let prompt = format!(
        r#"You are a code reviewer. Please review this GitHub Pull Request.

## PR Information
{}

## Diff
```diff
{}
```

Please provide a thorough code review covering:
1. **Summary**: Brief overview of what this PR does
2. **Code Quality**: Issues with code style, readability, or maintainability
3. **Potential Bugs**: Any logic errors or edge cases not handled
4. **Security**: Any security concerns
5. **Suggestions**: Improvements or alternative approaches

Be specific and reference line numbers or code snippets where relevant."#,
        pr_metadata, diff
    );

    let mut child = Command::new("qwen")
        .args(["--approval-mode", "yolo", "-p", &prompt])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn qwen: {}", e))?;

    let status = child.wait().map_err(|e| format!("Failed to wait for qwen: {}", e))?;

    if !status.success() {
        return Err("qwen exited with non-zero status".to_string());
    }

    Ok(())
}

fn main() {
    let args = Args::parse();

    let pr_info = match parse_pr_url(&args.pr_url) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!(
        "Reviewing PR #{} in {}/{}...",
        pr_info.pr_number, pr_info.owner, pr_info.repo
    );

    let pr_metadata = match fetch_pr_info(&pr_info) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error fetching PR info: {}", e);
            std::process::exit(1);
        }
    };

    let diff = match fetch_pr_diff(&pr_info) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error fetching diff: {}", e);
            std::process::exit(1);
        }
    };

    if diff.is_empty() {
        eprintln!("Warning: PR diff is empty");
    }

    if let Err(e) = review_with_qwen(&pr_metadata, &diff) {
        eprintln!("Error during review: {}", e);
        std::process::exit(1);
    }
}
