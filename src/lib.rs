use clap::{Args, Parser, Subcommand};
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A file as understood by both `cat` (read from disk) and `extract`
/// (parsed from a snapshot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub path: PathBuf,
    /// Raw bytes of the file.  Empty vec means the file has no content.
    pub content: Vec<u8>,
}

impl File {
    /// Write this file's entry to a cat-llm markdown snapshot.
    pub fn write_snapshot<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writeln!(writer, "{}", self.path.display())?;
        writeln!(writer, "```")?;
        if self.content.is_empty() {
            writeln!(writer, " ")?;
        } else {
            // Caller is responsible for content validity; we write it as-is,
            // trimming only the trailing newline that run_cat originally stripped.
            let trimmed = self
                .content
                .iter()
                .rposition(|&b| b != b'\n' && b != b'\r')
                .map(|i| &self.content[..=i])
                .unwrap_or(&self.content);
            writer.write_all(trimmed)?;
            writeln!(writer)?;
        }
        writeln!(writer, "```\n")?;
        Ok(())
    }

    /// Write this file's content to disk, creating parent directories as needed.
    pub fn write_to_disk(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, &self.content)
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "cat-llm",
    about = "Concatenate files for LLM context, respecting ignore rules."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    // When no subcommand is given, the remaining args are forwarded to `cat`.
    #[command(flatten)]
    pub cat_args: CatArgs,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Concatenate files into LLM-ready markdown (default behaviour)
    Cat(CatArgs),

    /// Extract files from a cat-llm markdown snapshot back to disk
    Extract(ExtractArgs),
}

#[derive(Args, Debug, Default, Clone)]
pub struct CatArgs {
    #[arg(long, value_name = "FILE_NAME")]
    pub ignore_file: Vec<String>,

    #[arg(long, value_name = "PATTERN")]
    pub ignore_pattern: Vec<String>,

    #[arg(long)]
    pub path_only: bool,

    #[arg(name = "PATHS", default_value = ".")]
    pub paths: Vec<PathBuf>,
}

/// Walk the filesystem according to `args` and return a sorted `Vec<File>`.
/// File content is returned as raw bytes; the caller decides how to handle them.
pub fn collect_files(args: &CatArgs) -> Vec<File> {
    if args.paths.is_empty() {
        return Vec::new();
    }

    let mut builder = WalkBuilder::new(&args.paths[0]);
    for path in args.paths.iter().skip(1) {
        builder.add(path);
    }

    builder.add_custom_ignore_filename(".gitignore");
    for ignore_file in &args.ignore_file {
        builder.add_custom_ignore_filename(ignore_file);
    }

    if !args.ignore_pattern.is_empty() {
        let mut ov = OverrideBuilder::new(&args.paths[0]);
        for pat in &args.ignore_pattern {
            let _ = ov.add(&format!("!{}", pat));
        }
        if let Ok(override_set) = ov.build() {
            builder.overrides(override_set);
        }
    }

    let mut paths: Vec<PathBuf> = builder
        .build()
        .flatten()
        .filter_map(|e| {
            let p = e.into_path();
            p.is_file().then_some(p)
        })
        .collect();
    paths.sort();

    paths
        .into_iter()
        .filter_map(|path| fs::read(&path).ok().map(|content| File { path, content }))
        .collect()
}

/// Run the `cat` subcommand: walk, then render each `File` to the writer.
pub fn run_cat<W: Write>(args: &CatArgs, writer: &mut W) -> std::io::Result<()> {
    for file in collect_files(args) {
        writeln!(writer, "{}", file.path.display())?;
        if args.path_only {
            continue;
        }
        file.write_snapshot(writer)?;
    }
    Ok(())
}

#[derive(Args, Debug, Default, Clone)]
pub struct ExtractArgs {
    /// Input snapshot file produced by cat-llm (use `-` for stdin)
    #[arg(short, long, value_name = "FILE")]
    pub input: PathBuf,

    /// Output directory to extract files into (default: current directory).
    /// Absolute paths in the snapshot are rebased relative to this directory.
    #[arg(short, long, value_name = "DIR", default_value = ".")]
    pub output: PathBuf,

    /// Overwrite existing files (default: skip)
    #[arg(long)]
    pub overwrite: bool,
}

/// Rebase `path` under `output`.
///
/// * Absolute paths: strip the leading `/` (or drive prefix on Windows) and
///   join the remainder under `output`, so `/project/src/main.rs` becomes
///   `<output>/project/src/main.rs`.
/// * Relative paths: join as-is under `output`.
fn rebase(path: &Path, output: &Path) -> PathBuf {
    let rel = if path.is_absolute() {
        // Strip the root component (`/` on Unix, `C:\` on Windows).
        path.components().skip(1).collect::<PathBuf>()
    } else {
        path.to_path_buf()
    };
    output.join(rel)
}

/// Run the `extract` subcommand.  Returns `(written, skipped)` counts.
pub fn run_extract<W: Write>(
    args: &ExtractArgs,
    writer: &mut W,
) -> std::io::Result<(usize, usize)> {
    let raw = if args.input == Path::new("-") {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        buf
    } else {
        fs::read_to_string(&args.input)?
    };

    let mut written = 0usize;
    let mut skipped = 0usize;

    for mut file in parse_snapshot(&raw) {
        file.path = rebase(&file.path, &args.output);

        if file.path.exists() && !args.overwrite {
            writeln!(writer, "skip  {}", file.path.display())?;
            skipped += 1;
            continue;
        }
        file.write_to_disk()?;
        writeln!(writer, "write {}", file.path.display())?;
        written += 1;
    }

    Ok((written, skipped))
}

/// Returns true when a line looks like a file-system path as emitted by
/// `run_cat` (i.e. the result of `path.display()`).
///
/// On Unix, `WalkBuilder` always returns absolute paths starting with `/`.
/// On Windows they start with a drive letter + `:\`.
/// Explicit relative paths start with `./` or `.\`.
fn is_path_line(s: &str) -> bool {
    if s.is_empty() || s == "```" {
        return false;
    }
    if s.starts_with('/') {
        return true;
    }
    if s.starts_with("./") || s.starts_with(".\\") {
        return true;
    }
    // Windows absolute: C:\... or C:/...
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Parse a cat-llm markdown snapshot into a `Vec<File>`.
pub fn parse_snapshot(input: &str) -> Vec<File> {
    let mut results = Vec::new();
    let mut lines = input.lines().peekable();

    while let Some(line) = lines.next() {
        let candidate = line.trim_end();

        if !is_path_line(candidate) {
            continue;
        }

        if lines.peek().map(|l| l.trim_end()) == Some("```") {
            let path = PathBuf::from(candidate);
            lines.next(); // consume opening fence

            let mut content_lines: Vec<&str> = Vec::new();
            for inner in lines.by_ref() {
                if inner.trim_end() == "```" {
                    break;
                }
                content_lines.push(inner);
            }

            // Single-space sentinel → empty file.
            let content = if content_lines.len() == 1 && content_lines[0] == " " {
                Vec::new()
            } else {
                let joined = content_lines.join("\n");
                let mut bytes = joined.into_bytes();
                if !bytes.is_empty() {
                    bytes.push(b'\n');
                }
                bytes
            };

            results.push(File { path, content });
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ignore_mechanisms() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join(".gitignore"), "*.log\n").unwrap();
        fs::write(base.join(".llmignore"), "secret.txt\n").unwrap();
        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("empty.txt"), "").unwrap();
        fs::write(base.join("app.log"), "log data").unwrap();
        fs::write(base.join("secret.txt"), "secret").unwrap();
        fs::write(base.join("uv.lock"), "lock").unwrap();

        let args = CatArgs {
            ignore_file: vec![".llmignore".to_string()],
            ignore_pattern: vec!["*.lock".to_string()],
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run_cat(&args, &mut output).unwrap();
        let out = String::from_utf8(output).unwrap();

        assert!(out.contains("main.rs"));
        assert!(!out.contains("app.log"), ".gitignore should exclude *.log");
        assert!(
            !out.contains("secret.txt"),
            ".llmignore should exclude secret.txt"
        );
        assert!(
            !out.contains("uv.lock"),
            "CLI pattern should exclude *.lock"
        );
        assert!(
            out.contains("empty.txt\n```\n \n```"),
            "empty file sentinel"
        );
    }

    #[test]
    fn test_hierarchical_isolation() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join(".customignore"), "file1.txt\n").unwrap();
        fs::write(base.join("file1.txt"), "root").unwrap();
        fs::write(base.join("file2.txt"), "root").unwrap();

        let sub1 = base.join("sub1");
        fs::create_dir(&sub1).unwrap();
        fs::write(sub1.join(".customignore"), "file2.txt\n").unwrap();
        fs::write(sub1.join("file1.txt"), "sub1").unwrap();
        fs::write(sub1.join("file2.txt"), "sub1").unwrap();
        fs::write(sub1.join("file3.txt"), "sub1").unwrap();

        let sub2 = base.join("sub2");
        fs::create_dir(&sub2).unwrap();
        fs::write(sub2.join(".customignore"), "file3.txt\n").unwrap();
        fs::write(sub2.join("file2.txt"), "sub2").unwrap();
        fs::write(sub2.join("file3.txt"), "sub2").unwrap();

        let args = CatArgs {
            ignore_file: vec![".customignore".to_string()],
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run_cat(&args, &mut output).unwrap();
        let out = String::from_utf8(output).unwrap();

        assert!(!out.contains(&base.join("file1.txt").display().to_string()));
        assert!(out.contains(&base.join("file2.txt").display().to_string()));
        assert!(!out.contains(&sub1.join("file1.txt").display().to_string()));
        assert!(!out.contains(&sub1.join("file2.txt").display().to_string()));
        assert!(out.contains(&sub1.join("file3.txt").display().to_string()));
        assert!(out.contains(&sub2.join("file2.txt").display().to_string()));
        assert!(!out.contains(&sub2.join("file3.txt").display().to_string()));
    }

    #[test]
    fn test_negation_rules_mixing() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join(".gitignore"), "*.log\n!important.log\n").unwrap();
        fs::write(base.join("app.log"), "ignore me").unwrap();
        fs::write(base.join("important.log"), "keep me").unwrap();
        fs::write(base.join("main.rs"), "code").unwrap();

        let args = CatArgs {
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run_cat(&args, &mut output).unwrap();
        let out = String::from_utf8(output).unwrap();

        assert!(out.contains("main.rs"));
        assert!(!out.contains("app.log"));
        assert!(out.contains("important.log"));
    }

    #[test]
    fn test_clap_multiple_args_parsing() {
        let cli = Cli::try_parse_from([
            "cat-llm",
            "cat",
            "--ignore-file",
            ".llmignore",
            "--ignore-file",
            ".custom_ignore",
            "--ignore-pattern",
            "*.lock",
            "--ignore-pattern",
            "*.log",
            "src",
            "tests",
        ])
        .expect("Failed to parse CLI arguments");

        let args = match cli.command {
            Some(Command::Cat(a)) => a,
            _ => panic!("expected Cat subcommand"),
        };

        assert_eq!(args.ignore_file, vec![".llmignore", ".custom_ignore"]);
        assert_eq!(args.ignore_pattern, vec!["*.lock", "*.log"]);
        assert_eq!(
            args.paths,
            vec![PathBuf::from("src"), PathBuf::from("tests")]
        );
    }

    #[test]
    fn test_path_only() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub fn foo() {}").unwrap();

        let args = CatArgs {
            path_only: true,
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run_cat(&args, &mut output).unwrap();
        let out = String::from_utf8(output).unwrap();

        assert!(out.contains("main.rs"));
        assert!(out.contains("lib.rs"));
        assert!(!out.contains("```"), "path-only must not emit code fences");
        assert!(
            !out.contains("fn main"),
            "path-only must not emit file contents"
        );
    }

    #[test]
    fn test_file_write_snapshot_normal() {
        let file = File {
            path: PathBuf::from("/proj/src/main.rs"),
            content: b"fn main() {}\n".to_vec(),
        };
        let mut out = Vec::new();
        file.write_snapshot(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "/proj/src/main.rs\n```\nfn main() {}\n```\n\n");
    }

    #[test]
    fn test_file_write_snapshot_empty() {
        let file = File {
            path: PathBuf::from("/proj/empty.txt"),
            content: Vec::new(),
        };
        let mut out = Vec::new();
        file.write_snapshot(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "/proj/empty.txt\n```\n \n```\n\n");
    }

    #[test]
    fn test_file_write_to_disk() {
        let dir = tempdir().unwrap();
        let dest = dir.path().join("sub").join("file.txt");
        let file = File {
            path: dest.clone(),
            content: b"hello\n".to_vec(),
        };
        file.write_to_disk().unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello\n");
    }

    #[test]
    fn test_parse_snapshot_basic() {
        let snapshot = concat!(
            "/project/src/main.rs\n",
            "```\n",
            "fn main() {}\n",
            "```\n",
            "\n",
            "/project/src/lib.rs\n",
            "```\n",
            "pub fn foo() {}\n",
            "```\n",
        );

        let files = parse_snapshot(snapshot);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, PathBuf::from("/project/src/main.rs"));
        assert_eq!(files[0].content, b"fn main() {}\n");
        assert_eq!(files[1].path, PathBuf::from("/project/src/lib.rs"));
        assert_eq!(files[1].content, b"pub fn foo() {}\n");
    }

    #[test]
    fn test_parse_snapshot_empty_file() {
        let files = parse_snapshot("/project/empty.txt\n```\n \n```\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("/project/empty.txt"));
        assert!(files[0].content.is_empty(), "empty sentinel → zero bytes");
    }

    // Regression: content lines like `}` or `strip = true` immediately before
    // a fence were misidentified as path lines, producing spurious file writes.
    #[test]
    fn test_parse_snapshot_does_not_misidentify_content_as_paths() {
        let snapshot = concat!(
            "/project/src/lib.rs\n",
            "```\n",
            "pub fn foo() {\n",
            "    42\n",
            "}\n",
            "```\n",
            "\n",
            "/project/pyproject.toml\n",
            "```\n",
            "[tool.maturin]\n",
            "strip = true\n",
            "```\n",
        );

        let files = parse_snapshot(snapshot);
        assert_eq!(
            files.len(),
            2,
            "expected 2 files, got {}: {:?}",
            files.len(),
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(files[0].content, b"pub fn foo() {\n    42\n}\n");
        assert_eq!(files[1].content, b"[tool.maturin]\nstrip = true\n");
    }

    #[test]
    fn test_is_path_line() {
        assert!(is_path_line("/abs/path/file.rs"));
        assert!(is_path_line("./relative/file.rs"));
        assert!(is_path_line(".\\windows\\file.rs"));
        assert!(is_path_line("C:\\Users\\file.rs"));
        assert!(is_path_line("C:/Users/file.rs"));

        assert!(!is_path_line(""));
        assert!(!is_path_line("```"));
        assert!(!is_path_line("}"));
        assert!(!is_path_line("strip = true"));
        assert!(!is_path_line("fn main() {}"));
        assert!(!is_path_line("run: uv tool install ."));
        assert!(!is_path_line("        run: uv tool install ."));
    }

    #[test]
    fn test_rebase_absolute() {
        let out = PathBuf::from("/out");
        assert_eq!(
            rebase(&PathBuf::from("/project/src/main.rs"), &out),
            PathBuf::from("/out/project/src/main.rs"),
        );
    }

    #[test]
    fn test_rebase_relative() {
        let out = PathBuf::from("/out");
        assert_eq!(
            rebase(&PathBuf::from("src/main.rs"), &out),
            PathBuf::from("/out/src/main.rs"),
        );
    }

    #[test]
    fn test_extract_skip_existing() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        let snap_path = base.join("snap.md");
        fs::write(&snap_path, "/project/main.rs\n```\nnew content\n```\n").unwrap();

        let rebased = base.join("project").join("main.rs");
        fs::create_dir_all(rebased.parent().unwrap()).unwrap();
        fs::write(&rebased, "original").unwrap();

        let mut out = Vec::new();
        let (written, skipped) = run_extract(
            &ExtractArgs {
                input: snap_path,
                overwrite: false,
                output: base.clone(),
            },
            &mut out,
        )
        .unwrap();

        assert_eq!(written, 0);
        assert_eq!(skipped, 1);
        assert_eq!(fs::read_to_string(&rebased).unwrap(), "original");
    }

    #[test]
    fn test_extract_overwrite() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        let snap_path = base.join("snap.md");
        fs::write(&snap_path, "/project/main.rs\n```\nnew content\n```\n").unwrap();

        let rebased = base.join("project").join("main.rs");
        fs::create_dir_all(rebased.parent().unwrap()).unwrap();
        fs::write(&rebased, "original").unwrap();

        let mut out = Vec::new();
        let (written, skipped) = run_extract(
            &ExtractArgs {
                input: snap_path,
                overwrite: true,
                output: base.clone(),
            },
            &mut out,
        )
        .unwrap();

        assert_eq!(written, 1);
        assert_eq!(skipped, 0);
        assert_eq!(fs::read_to_string(&rebased).unwrap(), "new content\n");
    }

    #[test]
    fn test_extract_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        let snap_path = base.join("snap.md");
        fs::write(&snap_path, "/project/a/b/c.txt\n```\nhello\n```\n").unwrap();

        let mut out = Vec::new();
        run_extract(
            &ExtractArgs {
                input: snap_path,
                overwrite: false,
                output: base.clone(),
            },
            &mut out,
        )
        .unwrap();

        let rebased = base.join("project").join("a").join("b").join("c.txt");
        assert_eq!(fs::read_to_string(&rebased).unwrap(), "hello\n");
    }

    #[test]
    fn test_roundtrip() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join("hello.rs"), "fn hello() {}\n").unwrap();
        fs::write(base.join("empty.txt"), "").unwrap();

        let mut snapshot_bytes = Vec::new();
        run_cat(
            &CatArgs {
                paths: vec![base.clone()],
                ..Default::default()
            },
            &mut snapshot_bytes,
        )
        .unwrap();

        let out_dir = tempdir().unwrap();
        let out_base = out_dir.path().canonicalize().unwrap();

        for mut file in parse_snapshot(&String::from_utf8(snapshot_bytes).unwrap()) {
            let rel = file.path.strip_prefix(&base).unwrap().to_owned();
            file.path = out_base.join(rel);
            file.write_to_disk().unwrap();
        }

        assert_eq!(
            fs::read_to_string(out_base.join("hello.rs")).unwrap(),
            "fn hello() {}\n"
        );
        assert_eq!(fs::read_to_string(out_base.join("empty.txt")).unwrap(), "");
    }
}
