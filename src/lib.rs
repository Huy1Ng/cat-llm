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
    #[arg(long, value_name = "FILE_NAME")]
    pub ignore_file: Vec<String>,

    #[arg(long, value_name = "PATTERN")]
    pub ignore_pattern: Vec<String>,

    #[arg(long)]
    pub path_only: bool,

    #[arg(name = "PATHS", default_value = ".")]
    pub paths: Vec<PathBuf>,
}

pub fn run<W: Write>(args: &Args, writer: &mut W) -> std::io::Result<()> {
    if args.paths.is_empty() {
        return Ok(());
    }

    let mut builder = WalkBuilder::new(&args.paths[0]);
    for path in args.paths.iter().skip(1) {
        builder.add(path);
    }

    // Force .gitignore to act as a universal ignore file, even outside of git repos.
    builder.add_custom_ignore_filename(".gitignore");

    // Add any additional user-provided ignore files (e.g., .llmignore)
    for ignore_file in &args.ignore_file {
        builder.add_custom_ignore_filename(ignore_file);
    }

    // Apply custom ignore patterns
    if !args.ignore_pattern.is_empty() {
        let mut ov = OverrideBuilder::new(&args.paths[0]);
        for pat in &args.ignore_pattern {
            let _ = ov.add(&format!("!{}", pat));
        }
        if let Ok(override_set) = ov.build() {
            builder.overrides(override_set);
        }
    }

    // Walk directories, collect, and sort for deterministic output
    let mut files: Vec<PathBuf> = builder
        .build()
        .flatten()
        .filter_map(|e| {
            let p = e.into_path();
            p.is_file().then_some(p)
        })
        .collect();
    files.sort();

    for path in files {
        writeln!(writer, "{}", path.display())?;

        if args.path_only {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
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

        let args = Args {
            ignore_file: vec![".llmignore".to_string()],
            ignore_pattern: vec!["*.lock".to_string()],
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(out_str.contains("main.rs"), "Standard files should be read");
        assert!(
            !out_str.contains("app.log"),
            ".gitignore should successfully exclude *.log"
        );
        assert!(
            !out_str.contains("secret.txt"),
            ".llmignore should successfully exclude secret.txt"
        );
        assert!(
            !out_str.contains("uv.lock"),
            "CLI pattern should successfully exclude *.lock"
        );
        assert!(
            out_str.contains("empty.txt\n```\n \n```"),
            "Empty files should contain a single space inside code fences"
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

        let args = Args {
            ignore_file: vec![".customignore".to_string()],
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        let root_f1 = base.join("file1.txt").display().to_string();
        let root_f2 = base.join("file2.txt").display().to_string();
        let sub1_f1 = sub1.join("file1.txt").display().to_string();
        let sub1_f2 = sub1.join("file2.txt").display().to_string();
        let sub1_f3 = sub1.join("file3.txt").display().to_string();
        let sub2_f2 = sub2.join("file2.txt").display().to_string();
        let sub2_f3 = sub2.join("file3.txt").display().to_string();

        assert!(
            !out_str.contains(&root_f1),
            "Root file1 ignored by root rule"
        );
        assert!(
            out_str.contains(&root_f2),
            "Root file2 NOT ignored by root rule"
        );
        assert!(
            !out_str.contains(&sub1_f1),
            "Sub1 file1 ignored by inherited root rule"
        );
        assert!(
            !out_str.contains(&sub1_f2),
            "Sub1 file2 ignored by its own local rule"
        );
        assert!(out_str.contains(&sub1_f3), "Sub1 file3 NOT ignored");
        assert!(
            out_str.contains(&sub2_f2),
            "Sub2 file2 NOT ignored (sibling sub1 rules must not leak)"
        );
        assert!(
            !out_str.contains(&sub2_f3),
            "Sub2 file3 ignored by its own local rule"
        );
    }

    #[test]
    fn test_negation_rules_mixing() {
        let dir = tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();

        fs::write(base.join(".gitignore"), "*.log\n!important.log\n").unwrap();
        fs::write(base.join("app.log"), "ignore me").unwrap();
        fs::write(base.join("important.log"), "keep me").unwrap();
        fs::write(base.join("main.rs"), "code").unwrap();

        let args = Args {
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(
            out_str.contains("main.rs"),
            "Standard files should be included"
        );
        assert!(
            !out_str.contains("app.log"),
            "Regular .log should be ignored by *.log"
        );
        assert!(
            out_str.contains("important.log"),
            "Negation rule (!important.log) should override and include the file"
        );
    }

    #[test]
    fn test_clap_multiple_args_parsing() {
        let cli_input = vec![
            "cat-llm",
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
        ];
        let args = Args::try_parse_from(cli_input).expect("Failed to parse CLI arguments");

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

        let args = Args {
            path_only: true,
            paths: vec![base.clone()],
            ..Default::default()
        };

        let mut output = Vec::new();
        run(&args, &mut output).unwrap();
        let out_str = String::from_utf8(output).unwrap();

        assert!(out_str.contains("main.rs"), "path-only should list main.rs");
        assert!(out_str.contains("lib.rs"), "path-only should list lib.rs");
        assert!(
            !out_str.contains("```"),
            "path-only must not emit code fences"
        );
        assert!(
            !out_str.contains("fn main"),
            "path-only must not emit file contents"
        );
    }
}
