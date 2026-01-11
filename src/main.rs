use clap::Parser;
use regex::Regex;
use serde::Deserialize;
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

struct PrHeadInfo {
    head_ref: String,
    head_repo_owner: String,
    head_repo_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPrHeadResponse {
    head_ref_name: String,
    head_repository: GhRepoInfo,
    head_repository_owner: GhOwnerInfo,
}

#[derive(Deserialize)]
struct GhRepoInfo {
    name: String,
}

#[derive(Deserialize)]
struct GhOwnerInfo {
    login: String,
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

fn fetch_pr_head_info(pr_info: &PrInfo) -> Result<PrHeadInfo, String> {
    let repo = format!("{}/{}", pr_info.owner, pr_info.repo);

    let output = Command::new("gh")
        .args([
            "pr", "view",
            &pr_info.pr_number.to_string(),
            "-R", &repo,
            "--json", "headRefName,headRepository,headRepositoryOwner",
        ])
        .output()
        .map_err(|e| format!("Failed to run gh command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr view (head info) failed: {}", stderr));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let response: GhPrHeadResponse = serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse PR head info: {}", e))?;

    Ok(PrHeadInfo {
        head_ref: response.head_ref_name,
        head_repo_owner: response.head_repository_owner.login,
        head_repo_name: response.head_repository.name,
    })
}

fn remote_exists(remote_name: &str) -> bool {
    Command::new("git")
        .args(["remote", "get-url", remote_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn setup_git_remote(head_info: &PrHeadInfo) -> Result<String, String> {
    let remote_name = &head_info.head_repo_owner;
    let remote_url = format!(
        "https://github.com/{}/{}.git",
        head_info.head_repo_owner, head_info.head_repo_name
    );

    if remote_exists(remote_name) {
        eprintln!("Remote '{}' already exists", remote_name);
    } else {
        eprintln!("Adding remote '{}' -> {}", remote_name, remote_url);
        let output = Command::new("git")
            .args(["remote", "add", remote_name, &remote_url])
            .output()
            .map_err(|e| format!("Failed to run git remote add: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git remote add failed: {}", stderr));
        }
    }

    Ok(remote_name.clone())
}

fn fetch_and_checkout_branch(remote_name: &str, branch_name: &str) -> Result<(), String> {
    eprintln!("Fetching from remote '{}'...", remote_name);
    let output = Command::new("git")
        .args(["fetch", remote_name, branch_name])
        .output()
        .map_err(|e| format!("Failed to run git fetch: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git fetch failed: {}", stderr));
    }

    let checkout_ref = format!("{}/{}", remote_name, branch_name);
    eprintln!("Checking out '{}'...", checkout_ref);
    let output = Command::new("git")
        .args(["checkout", &checkout_ref])
        .output()
        .map_err(|e| format!("Failed to run git checkout: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git checkout failed: {}", stderr));
    }

    Ok(())
}

fn review_with_qwen(pr_metadata: &str, diff: &str) -> Result<(), String> {
    let prompt = format!(
        r#"You are a code reviewer. Review this GitHub Pull Request by analyzing BOTH the diff AND the local codebase.

## PR Information
{}

## Diff
```diff
{}
```

## Your Task

IMPORTANT: You have access to the full codebase in the current directory. Use your tools to:
1. Read files that are modified in this PR to understand the full context
2. Search for callers/usages of any modified functions, classes, or APIs
3. Check if the changes break any existing code or tests
4. Look at related files to understand architectural patterns

Then provide a review covering:

1. **Summary**: What this PR does
2. **Codebase Impact**: How these changes affect other parts of the codebase (search for usages!)
3. **Breaking Changes**: Any APIs, function signatures, or behaviors that could break callers
4. **Code Quality**: Style, readability, maintainability issues
5. **Potential Bugs**: Logic errors, edge cases, race conditions
6. **Security**: Any security concerns
7. **Suggestions**: Improvements or alternative approaches
8. **Suggestions on what lines**: If comments are required, suggest on which file paths and line numbers the comments should be left

Be specific with file paths and line numbers. Actually explore the codebase - don't just review the diff in isolation."#,
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

    // Fetch PR head branch info and setup git
    let head_info = match fetch_pr_head_info(&pr_info) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("Error fetching PR head info: {}", e);
            std::process::exit(1);
        }
    };

    eprintln!(
        "PR branch: {}/{}/{}",
        head_info.head_repo_owner, head_info.head_repo_name, head_info.head_ref
    );

    let remote_name = match setup_git_remote(&head_info) {
        Ok(name) => name,
        Err(e) => {
            eprintln!("Error setting up git remote: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(e) = fetch_and_checkout_branch(&remote_name, &head_info.head_ref) {
        eprintln!("Error checking out PR branch: {}", e);
        std::process::exit(1);
    }

    eprintln!("Ready for review on branch '{}'", head_info.head_ref);

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
