//! PR range comparisons against a virtual base with upstream changes already merged.
//! Git objects live in an isolated, shallow, blob-filtered cache; no checkout is needed.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::process::{Output, Stdio};

use anyhow::{Context, Result, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::dirs::Paths;

pub async fn merged_range_diff(
    repository: &str,
    before: &str,
    last: &str,
    upstream: &str,
    ancestor: &str,
) -> Result<String> {
    for oid in [before, last, upstream, ancestor] {
        validate_oid(oid)?;
    }
    let cache = Paths::resolve(None)?.root.join("git-diffs");
    std::fs::create_dir_all(&cache).context("failed to create Git diff cache")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))?;
    }
    let key = format!("{:x}", md5::compute(repository.to_ascii_lowercase()));
    // Also serialize separate ghr processes: shallow fetches share a shallow file.
    let _lock = lock_cache(cache.join(format!("{key}.lock"))).await?;
    let repo = DiffRepository {
        path: cache.join(key),
        remote: format!("https://github.com/{repository}.git"),
    };
    repo.ensure().await?;
    repo.fetch(&[before, last, upstream, ancestor]).await?;
    repo.diff(before, last, upstream, ancestor).await
}

async fn lock_cache(path: PathBuf) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn validate_oid(oid: &str) -> Result<()> {
    if oid.len() != 40 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid GitHub commit object ID");
    }
    Ok(())
}

struct DiffRepository {
    path: PathBuf,
    remote: String,
}

impl DiffRepository {
    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&self.path)
            .args(["-c", "gc.auto=0", "-c", "maintenance.auto=false"])
            .args(["-c", "credential.helper="])
            .args(["-c", "remote.origin.promisor=true"])
            .args(["-c", "remote.origin.partialCloneFilter=blob:none"])
            .arg("-c")
            .arg(format!("remote.origin.url={}", self.remote))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_ATTR_NOSYSTEM", "1")
            .kill_on_drop(true);
        // Do not inherit a user's checkout, index, object store, or merge drivers.
        for variable in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
        ] {
            command.env_remove(variable);
        }
        if let Some(token) = crate::github_api::token_from_env() {
            // Keep credentials out of command arguments, cache files, and diagnostics.
            command.env("GHR_DIFF_TOKEN", token).args([
                "-c",
                r#"credential.helper=!f() { if [ "$1" = get ]; then printf '%s\n' 'username=x-access-token' "password=$GHR_DIFF_TOKEN"; fi; }; f"#,
            ]);
        } else {
            command.args(["-c", "credential.helper=!gh auth git-credential"]);
        }
        command
    }

    async fn output(&self, args: &[&str]) -> Result<Output> {
        self.command()
            .args(args)
            .output()
            .await
            .context("failed to run Git for PR range comparison (Git 2.46 or newer is required)")
    }

    async fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.output(args).await?;
        Self::checked_output(args[0], output)
    }

    fn checked_output(operation: &str, output: Output) -> Result<String> {
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                operation,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn index_command(&self, args: &[&str], input: &[u8]) -> Result<String> {
        let mut child = self
            .command()
            .args(args)
            .env("GIT_INDEX_FILE", self.path.join("merge.index"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to update virtual comparison index")?;
        let mut stdin = child.stdin.take().context("missing Git index input")?;
        stdin.write_all(input).await?;
        drop(stdin);
        Self::checked_output(args[0], child.wait_with_output().await?)
    }

    async fn virtual_base(&self, before: &str, upstream: &str, ancestor: &str) -> Result<String> {
        let output = self
            .output(&[
                "merge-tree",
                "--write-tree",
                "--no-messages",
                "-z",
                "-Xours",
                &format!("--merge-base={ancestor}"),
                before,
                upstream,
            ])
            .await?;
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Self::checked_output("merge-tree", output);
        }
        let mut records = output.stdout.split(|byte| *byte == 0);
        let tree =
            std::str::from_utf8(records.next().context("missing virtual base tree")?)?.trim();
        validate_oid(tree)?;
        if output.status.success() {
            return Ok(tree.to_string());
        }
        // -Xours handles content conflicts. Resolve structural conflicts (e.g.
        // modify/delete) using stage 2 as well; absence at stage 2 means deletion.
        // Keep paths as bytes so tabs, newlines, and non-UTF-8 names stay intact.
        let mut resolutions = BTreeMap::new();
        for record in records.take_while(|record| !record.is_empty()) {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .context("invalid merge conflict record")?;
            let fields = std::str::from_utf8(&record[..tab])?
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.len() != 3 {
                bail!("invalid merge conflict stages");
            }
            validate_oid(fields[1])?;
            let resolution = resolutions.entry(&record[tab + 1..]).or_insert(None);
            if fields[2] == "2" {
                *resolution = Some(format!("{} {}\t", fields[0], fields[1]));
            }
        }
        if resolutions.is_empty() {
            bail!("Git could not resolve the virtual comparison base");
        }
        self.index_command(&["read-tree", tree], &[]).await?;
        let mut input = Vec::new();
        for (path, resolution) in resolutions {
            input.extend_from_slice(
                resolution
                    .as_deref()
                    .unwrap_or("0 0000000000000000000000000000000000000000\t")
                    .as_bytes(),
            );
            input.extend_from_slice(path);
            input.push(0);
        }
        self.index_command(&["update-index", "-z", "--index-info"], &input)
            .await?;
        let tree = self.index_command(&["write-tree"], &[]).await?;
        validate_oid(tree.trim())?;
        Ok(tree.trim().to_string())
    }

    async fn ensure(&self) -> Result<()> {
        let version = self.run(&["--version"]).await?;
        let mut numbers = version
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .split('.');
        let major = numbers
            .next()
            .and_then(|part| part.parse::<u32>().ok())
            .unwrap_or(0);
        let minor = numbers
            .next()
            .and_then(|part| part.parse::<u32>().ok())
            .unwrap_or(0);
        if (major, minor) < (2, 46) {
            bail!(
                "PR range comparison requires Git 2.46 or newer; found {}",
                version.trim()
            );
        }
        if !self.path.join("HEAD").exists() {
            std::fs::create_dir_all(&self.path)?;
            self.run(&["init", "--bare", "--quiet", "--template="])
                .await?;
        }
        Ok(())
    }

    async fn fetch(&self, oids: &[&str]) -> Result<()> {
        let mut refs = Vec::new();
        for oid in oids {
            validate_oid(oid)?;
            let reference = format!("refs/ghr/{oid}");
            if !self
                .output(&["show-ref", "--verify", "--quiet", &reference])
                .await?
                .status
                .success()
            {
                let spec = format!("{oid}:{reference}");
                if !refs.contains(&spec) {
                    refs.push(spec);
                }
            }
        }
        if !refs.is_empty() {
            let mut args = vec![
                "fetch",
                "--quiet",
                "--depth=1",
                "--filter=blob:none",
                "--no-tags",
                "--no-write-fetch-head",
                "--no-auto-maintenance",
                "origin",
            ];
            args.extend(refs.iter().map(String::as_str));
            self.run(&args)
                .await
                .context("failed to fetch PR comparison snapshots")?;
        }
        Ok(())
    }

    async fn diff(
        &self,
        before: &str,
        last: &str,
        upstream: &str,
        ancestor: &str,
    ) -> Result<String> {
        let tree = self
            .virtual_base(before, upstream, ancestor)
            .await
            .context("failed to construct the PR range's virtual base")?;
        let tree = tree.trim();
        validate_oid(tree)?;
        self.run(&[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--find-renames",
            "--unified=3",
            // GitHub joins contexts separated by one unchanged line.
            "--inter-hunk-context=1",
            "--diff-algorithm=myers",
            tree,
            last,
            "--",
        ])
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST: AtomicUsize = AtomicUsize::new(0);

    struct TestRepository(PathBuf);

    impl TestRepository {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ghr-range-diff-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_TEST.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&path).unwrap();
            git(&path, &["init", "--quiet"]);
            git(&path, &["config", "user.name", "Range test"]);
            git(&path, &["config", "user.email", "range@example.invalid"]);
            git(&path, &["config", "uploadpack.allowFilter", "true"]);
            git(&path, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
            Self(path)
        }

        fn write(&self, path: &str, text: &str) {
            std::fs::write(self.0.join(path), text).unwrap();
        }

        fn commit(&self) -> String {
            git(&self.0, &["add", "--all"]);
            git(
                &self.0,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--quiet",
                    "-m",
                    "snapshot",
                ],
            );
            git(&self.0, &["rev-parse", "HEAD"])
        }

        fn cache(&self) -> DiffRepository {
            DiffRepository {
                path: self.0.join("cache.git"),
                remote: format!("file://{}", self.0.display()),
            }
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(path: &Path, args: &[&str]) -> String {
        let output = StdCommand::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn shallow_range_diff_excludes_upstream_changes_and_preserves_review_edits() {
        let source = TestRepository::new();
        let text = "shared=base\none\ntwo\nthree\nfour\nfive\nsix\nupstream=base\nend\n";
        source.write("code.txt", text);
        source.write("old-name.txt", "renamed on the base branch\n");
        let ancestor = source.commit();
        source.write("code.txt", &text.replace("shared=base", "shared=feature"));
        source.write("feature.txt", "already present before the selected range\n");
        let before = source.commit();
        git(&source.0, &["checkout", "--quiet", "--detach", &ancestor]);
        source.write(
            "code.txt",
            &text
                .replace("shared=base", "shared=upstream")
                .replace("upstream=base", "upstream=updated"),
        );
        source.write("upstream.txt", "this must not appear in the range diff\n");
        git(&source.0, &["mv", "old-name.txt", "new-name.txt"]);
        let upstream = source.commit();
        // Construct the final snapshot independently of the virtual merge algorithm.
        source.write(
            "code.txt",
            &text
                .replace("shared=base", "shared=reviewed")
                .replace("upstream=base", "upstream=updated")
                .replace("\nend\n", "\nend=reviewed\n"),
        );
        source.write("feature.txt", "already present before the selected range\n");
        source.write("review.txt", "added in the selected range\n");
        git(&source.0, &["add", "--all"]);
        let tree = git(&source.0, &["write-tree"]);
        let last = git(
            &source.0,
            &[
                "commit-tree",
                &tree,
                "-p",
                &before,
                "-p",
                &upstream,
                "-m",
                "reviewed merge",
            ],
        );
        let checkout_before = git(&source.0, &["status", "--porcelain"]);
        let cache = source.cache();
        cache.ensure().await.unwrap();
        cache
            .fetch(&[&before, &last, &upstream, &ancestor])
            .await
            .unwrap();
        assert!(cache.path.join("shallow").exists());
        let diff = cache
            .diff(&before, &last, &upstream, &ancestor)
            .await
            .unwrap();
        let patch = diff
            .lines()
            .filter(|line| !line.starts_with("index "))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            patch,
            "diff --git a/code.txt b/code.txt\n--- a/code.txt\n+++ b/code.txt\n@@ -1,9 +1,9 @@\n-shared=feature\n+shared=reviewed\n one\n two\n three\n four\n five\n six\n upstream=updated\n-end\n+end=reviewed\ndiff --git a/review.txt b/review.txt\nnew file mode 100644\n--- /dev/null\n+++ b/review.txt\n@@ -0,0 +1 @@\n+added in the selected range"
        );
        // A warm cache must not require the remote or touch the source checkout.
        let cached = DiffRepository {
            path: cache.path.clone(),
            remote: "file:///missing-range-test-remote".into(),
        };
        cached
            .fetch(&[&before, &last, &upstream, &ancestor])
            .await
            .unwrap();
        assert_eq!(
            cached
                .diff(&before, &last, &upstream, &ancestor)
                .await
                .unwrap(),
            diff
        );
        let checkout_after = git(
            &source.0,
            &["status", "--porcelain", "--untracked-files=no"],
        );
        assert_eq!(checkout_before, checkout_after);
        assert_eq!(git(&source.0, &["rev-parse", "HEAD"]), upstream);
    }

    #[tokio::test]
    async fn virtual_base_keeps_our_side_of_delete_modify_conflicts() {
        let source = TestRepository::new();
        source.write("deleted.txt", "original\n");
        source.write("kept.txt", "original\n");
        let ancestor = source.commit();
        git(&source.0, &["rm", "--quiet", "deleted.txt"]);
        source.write("kept.txt", "feature\n");
        let before = source.commit();
        git(&source.0, &["checkout", "--quiet", "--detach", &ancestor]);
        source.write("deleted.txt", "changed upstream\n");
        git(&source.0, &["rm", "--quiet", "kept.txt"]);
        let upstream = source.commit();
        let cache = source.cache();
        cache.ensure().await.unwrap();
        cache.fetch(&[&before, &upstream, &ancestor]).await.unwrap();
        // Both modify/delete directions favor the range's starting snapshot.
        assert_eq!(
            cache
                .diff(&before, &before, &upstream, &ancestor)
                .await
                .unwrap(),
            ""
        );
    }
}
