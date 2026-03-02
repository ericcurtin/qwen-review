use clap::{Parser, Subcommand};
use regex::Regex;
use serde::Deserialize;
use std::io::Read;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "qwen-review")]
#[command(about = "Review GitHub PRs using Qwen AI")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Review a single PR in depth with codebase context
    Review { pr_url: String },
    /// Score multiple PRs by merge-readiness
    Score { pr_urls: Vec<String> },
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

struct PrScore {
    url: String,
    owner: String,
    repo: String,
    pr_number: u64,
    title: String,
    author: String,
    score: u8,
    verdict: String,
    issues: String,
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

#[derive(Deserialize)]
struct GhPrViewBasic {
    title: String,
    author: GhAuthorInfo,
}

#[derive(Deserialize)]
struct GhAuthorInfo {
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

fn fetch_pr_basic_info(pr_info: &PrInfo) -> Result<GhPrViewBasic, String> {
    let repo = format!("{}/{}", pr_info.owner, pr_info.repo);

    let output = Command::new("gh")
        .args([
            "pr", "view",
            &pr_info.pr_number.to_string(),
            "-R", &repo,
            "--json", "title,author",
        ])
        .output()
        .map_err(|e| format!("Failed to run gh command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh pr view failed: {}", stderr));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&json_str)
        .map_err(|e| format!("Failed to parse PR basic info: {}", e))
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

fn score_pr_with_qwen(url: &str, metadata_json: &str, diff: &str) -> Result<PrScore, String> {
    let pr_info = parse_pr_url(url)?;

    let prompt = format!(
        r#"You are a code reviewer scoring a PR for merge readiness. Analyze BOTH the diff AND the local codebase.

## PR Information
{}

## Diff
```diff
{}
```

## Your Task

Use your tools to explore the codebase:
1. Read files modified in this PR to understand the full context
2. Search for callers/usages of any modified functions, classes, or APIs
3. Check if the changes break any existing code or tests
4. Look at related files to understand architectural patterns

Scoring rubric:
- 1–3: Not ready (major issues, missing tests, breaks API)
- 4–6: Needs work (minor issues, incomplete)
- 7–8: Nearly ready (small nits)
- 9–10: Ready to merge

After your analysis, end your response with EXACTLY one line in this format:
SCORE: X/10 | VERDICT: <text> | ISSUES: <brief comma-separated issues or None>"#,
        metadata_json, diff
    );

    let mut child = Command::new("qwen")
        .args(["--approval-mode", "yolo", "-p", &prompt])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn qwen: {}", e))?;

    let mut stdout_handle = child.stdout.take()
        .ok_or_else(|| "Failed to capture qwen stdout".to_string())?;

    let mut output_text = String::new();
    stdout_handle.read_to_string(&mut output_text)
        .map_err(|e| format!("Failed to read qwen output: {}", e))?;

    let status = child.wait().map_err(|e| format!("Failed to wait for qwen: {}", e))?;

    print!("{}", output_text);

    if !status.success() {
        return Err("qwen exited with non-zero status".to_string());
    }

    let score_re = Regex::new(r"SCORE: (\d+)/10 \| VERDICT: ([^|]+) \| ISSUES: (.+)").unwrap();

    let caps = score_re.captures(&output_text)
        .ok_or_else(|| "Could not find SCORE line in qwen output".to_string())?;

    let score: u8 = caps[1].parse()
        .map_err(|e| format!("Failed to parse score number: {}", e))?;
    let verdict = caps[2].trim().to_string();
    let issues = caps[3].trim().to_string();

    let basic: GhPrViewBasic = serde_json::from_str(metadata_json)
        .map_err(|e| format!("Failed to parse metadata JSON: {}", e))?;

    Ok(PrScore {
        url: url.to_string(),
        owner: pr_info.owner,
        repo: pr_info.repo,
        pr_number: pr_info.pr_number,
        title: basic.title,
        author: basic.author.login,
        score,
        verdict,
        issues,
    })
}

fn print_score_table(scores: &[PrScore]) {
    println!("| PR | Title | Author | Score | Verdict | Key Issues |");
    println!("|----|-------|--------|-------|---------|------------|");
    for s in scores {
        println!(
            "| {}/{}#{} | {} | {} | {}/10 | {} | {} |",
            s.owner, s.repo, s.pr_number,
            s.title,
            s.author,
            s.score,
            s.verdict,
            s.issues,
        );
    }
}

fn main() {
    let args = Args::parse();

    match args.command {
        Commands::Review { pr_url } => {
            let pr_info = match parse_pr_url(&pr_url) {
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

        Commands::Score { pr_urls } => {
            let mut scores: Vec<PrScore> = Vec::new();

            for url in &pr_urls {
                let pr_info = match parse_pr_url(url) {
                    Ok(info) => info,
                    Err(e) => {
                        eprintln!("Warning: skipping {}: {}", url, e);
                        continue;
                    }
                };

                eprintln!(
                    "Scoring PR #{} in {}/{}...",
                    pr_info.pr_number, pr_info.owner, pr_info.repo
                );

                let head_info = match fetch_pr_head_info(&pr_info) {
                    Ok(info) => info,
                    Err(e) => {
                        eprintln!("Warning: skipping {}: failed to fetch head info: {}", url, e);
                        continue;
                    }
                };

                let remote_name = match setup_git_remote(&head_info) {
                    Ok(name) => name,
                    Err(e) => {
                        eprintln!("Warning: skipping {}: failed to setup remote: {}", url, e);
                        continue;
                    }
                };

                if let Err(e) = fetch_and_checkout_branch(&remote_name, &head_info.head_ref) {
                    eprintln!("Warning: skipping {}: failed to checkout branch: {}", url, e);
                    continue;
                }

                let metadata_json = match fetch_pr_basic_info(&pr_info) {
                    Ok(basic) => match serde_json::to_string(&serde_json::json!({
                        "title": basic.title,
                        "author": { "login": basic.author.login }
                    })) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("Warning: skipping {}: failed to serialize metadata: {}", url, e);
                            continue;
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: skipping {}: failed to fetch PR info: {}", url, e);
                        continue;
                    }
                };

                let diff = match fetch_pr_diff(&pr_info) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("Warning: skipping {}: failed to fetch diff: {}", url, e);
                        continue;
                    }
                };

                match score_pr_with_qwen(url, &metadata_json, &diff) {
                    Ok(ps) => scores.push(ps),
                    Err(e) => {
                        eprintln!("Warning: skipping {}: scoring failed: {}", url, e);
                    }
                }
            }

            if !scores.is_empty() {
                println!();
                print_score_table(&scores);
            } else {
                eprintln!("No PRs were successfully scored.");
            }
        }
    }
}
