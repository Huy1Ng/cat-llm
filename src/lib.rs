use clap::Parser;
use ignore::{WalkBuilder, overrides::OverrideBuilder};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug, Default)]
#[command(
    name = "cat-llm",
    about = "Concatenate files for LLM context, respecting ignore rules."
)]
pub struct Args {
    #[arg(long, value_delimiter = ',', value_name = "FILE_NAMES")]
    pub ignore_files: Vec<String>,

    #[arg(long, value_delimiter = ',', value_name = "PATTERNS")]
    pub ignore_patterns: Vec<String>,

    #[arg(name = "PATHS", default_value = ".")]
    pub paths: Vec<PathBuf>,
}

pub fn run<W: Write>(args: &Args, writer: &mut W) -> std::io::Result<()> {
    if args.paths.is_empty() {
        return Ok(());
    }

    // Initialize the WalkBuilder with the first path
    let mut builder = WalkBuilder::new(&args.paths[0]);

    for path in args.paths.iter().skip(1) {
        builder.add(path);
    }

    // 1. Apply Custom Ignore Files
    // Force .gitignore to act as a universal ignore file, even outside of git repos.
    builder.add_custom_ignore_filename(".gitignore");

    // Add any additional user-provided ignore files (e.g., .llmignore)
    for ignore_file in &args.ignore_files {
        builder.add_custom_ignore_filename(ignore_file);
    }

    // 2. Apply Custom Ignore Patterns
    if !args.ignore_patterns.is_empty() {
        // Anchor the overrides to the first path so patterns match correctly
        let mut ov = OverrideBuilder::new(&args.paths[0]);
        for pat in &args.ignore_patterns {
            let _ = ov.add(&format!("!{}", pat));
        }
        if let Ok(override_set) = ov.build() {
            builder.overrides(override_set);
        }
    }

    // 3. Walk directories, collect, and sort for deterministic testing
    let walker = builder.build();
    let mut files = Vec::new();

    for entry in walker.flatten() {
        let path = entry.path().to_path_buf();
        if path.is_file() {
            files.push(path);
        }
    }

    // Sort files to ensure outputs are consistent
    files.sort();

    // 4. Write output to the provided writer
    for path in files {
        if let Ok(content) = fs::read_to_string(&path) {
            writeln!(writer, "{}", path.display())?;
            writeln!(writer, "```")?;
            if content.trim().is_empty() {
                writeln!(writer, " ")?;
            } else {
                writeln!(writer, "{}", content.trim_end())?;
            }
            writeln!(writer, "```\n")?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Helper function to populate the test directory structure
    fn setup_test_env() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let base = dir.path();

        // Root files
        fs::write(base.join(".gitignore"), "*.log\nbuild/").unwrap();
        fs::write(base.join(".llmignore"), "secret.txt").unwrap();
        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("empty.txt"), "").unwrap();
        fs::write(base.join("app.log"), "log data").unwrap();
        fs::write(base.join("secret.txt"), "secret").unwrap();
        fs::write(base.join("uv.lock"), "lock").unwrap();

        // Subdir files
        let subdir = base.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join(".llmignore"), "local_ignore.txt").unwrap();
        fs::write(subdir.join("helper.rs"), "fn help() {}").unwrap();
        fs::write(subdir.join("local_ignore.txt"), "ignored").unwrap();
        fs::write(subdir.join("poetry.lock"), "lock").unwrap();

        dir // Return the TempDir so it stays alive for the duration of the test
    }

    #[test]
    fn test_default_behavior_respects_gitignore() {
        let dir = setup_test_env();
        let args = Args {
            paths: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(out_str.contains("main.rs"));
        assert!(out_str.contains("secret.txt")); // Not ignored yet
        assert!(!out_str.contains("app.log")); // .gitignore worked
    }

    #[test]
    fn test_custom_ignore_files() {
        let dir = setup_test_env();
        let args = Args {
            ignore_files: vec![".llmignore".to_string()],
            paths: vec![dir.path().to_path_buf()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(out_str.contains("main.rs"));
        assert!(!out_str.contains("secret.txt")); // Root .llmignore worked
        assert!(!out_str.contains("local_ignore.txt")); // Subdir .llmignore worked
    }

    #[test]
    fn test_custom_ignore_patterns() {
        let dir = setup_test_env();
        let args = Args {
            ignore_files: vec![".llmignore".to_string()],
            ignore_patterns: vec!["uv.lock".to_string(), "poetry.lock".to_string()],
            paths: vec![dir.path().to_path_buf()],
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(out_str.contains("main.rs"));
        assert!(out_str.contains("helper.rs"));
        assert!(!out_str.contains("uv.lock")); // Pattern worked
        assert!(!out_str.contains("poetry.lock")); // Pattern worked
    }

    #[test]
    fn test_empty_file_formatting() {
        let dir = setup_test_env();
        let args = Args {
            paths: vec![dir.path().join("empty.txt")],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        // Ensure it contains a blank space inside the fences for empty files
        assert!(out_str.contains("empty.txt\n```\n \n```"));
    }

    #[test]
    fn test_child_ignore_does_not_leak_to_parent() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        // 1. Create a file in the parent directory
        fs::write(base.join("leak_test.txt"), "parent content").unwrap();

        // 2. Create a child directory
        let subdir = base.join("child");
        fs::create_dir(&subdir).unwrap();

        // 3. Create a .gitignore in the CHILD directory that ignores "leak_test.txt"
        fs::write(subdir.join(".gitignore"), "leak_test.txt").unwrap();

        // 4. Create a file with the same name in the child directory
        fs::write(subdir.join("leak_test.txt"), "child content").unwrap();

        let args = Args {
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        // Format the expected paths to match how they will be printed by the tool
        let parent_file_path = base.join("leak_test.txt").display().to_string();
        let child_file_path = subdir.join("leak_test.txt").display().to_string();

        // The parent file MUST be present (child ignore should not leak up)
        assert!(
            out_str.contains(&parent_file_path),
            "Parent file was incorrectly ignored by child .gitignore!"
        );

        // The child file MUST NOT be present (child ignore should apply to its own directory)
        assert!(
            !out_str.contains(&child_file_path),
            "Child file was not correctly ignored by its own .gitignore!"
        );
    }

    #[test]
    fn test_clap_multiple_args_parsing() {
        // Simulate command line input using comma delimiters
        let cli_input = vec![
            "cat-llm",
            "--ignore-files",
            ".llmignore,.custom_ignore",
            "--ignore-patterns",
            "*.lock,*.log",
            "src",
            "tests",
        ];

        // Parse the simulated input
        let args = Args::try_parse_from(cli_input).expect("Failed to parse CLI arguments");

        // Verify vectors captured multiple items properly
        assert_eq!(args.ignore_files, vec![".llmignore", ".custom_ignore"]);
        assert_eq!(args.ignore_patterns, vec!["*.lock", "*.log"]);

        // Verify positional paths were cleanly separated
        assert_eq!(
            args.paths,
            vec![PathBuf::from("src"), PathBuf::from("tests")]
        );
    }

    #[test]
    fn test_ignore_patterns_with_wildcard_globs() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        // 1. Create a mix of target files and files to ignore in the root
        fs::write(base.join("Cargo.lock"), "lock data").unwrap();
        fs::write(base.join("uv.lock"), "lock data").unwrap();
        fs::write(base.join("main.rs"), "rust code").unwrap();

        // 2. Create a subdirectory with more files
        let subdir = base.join("subdir");
        fs::create_dir(&subdir).unwrap();
        fs::write(subdir.join("poetry.lock"), "lock data").unwrap();
        fs::write(subdir.join("lib.rs"), "rust code").unwrap();

        // 3. Pass the wildcard string exactly as if the user quoted it in bash
        let args = Args {
            ignore_patterns: vec!["*.lock".to_string()],
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        // The valid code files should be present
        assert!(out_str.contains("main.rs"));
        assert!(out_str.contains("lib.rs"));

        // ALL .lock files must be gone, proving the wildcard evaluation works
        assert!(!out_str.contains("Cargo.lock"));
        assert!(!out_str.contains("uv.lock"));
        assert!(!out_str.contains("poetry.lock"));
    }
}
