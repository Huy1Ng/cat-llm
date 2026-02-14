# cat-llm

A fast, lightweight Rust CLI tool to concatenate files for Large Language Model (LLM) context windows. It automatically respects your `.gitignore` rules, skips binary files, and supports custom `.llmignore` configurations so you only feed the LLM exactly what it needs to see.

## Features

* **Lightning Fast:** Written in Rust, duh.
* **Smart Ignore:** Supports hierarchical multiple ignore files (like `.llmignore`, defaults to `.gitignore`) that apply only to their specific subdirectories and children.
* **Pattern Filtering:** Supports ignore patterns for run-time file skipping.
* **Unix-Friendly:** Designed to pipe effortlessly with standard tools like `xargs` and `git diff`.

## Installation

You can install `cat-llm` natively via Rust's `cargo`, or as a standalone tool via Python's `uv`.

### Via Cargo (Rust)
```bash
# From local source
cargo install --path .

# Or directly from GitHub repository
cargo install --git [https://github.com/Huy1Ng/cat-llm](https://github.com/Huy1Ng/cat-llm)
```

### Via UV (Python)
```bash
# From local source
uv tool install .

# Or directly from GitHub repository
uv tool install git+[https://github.com/Huy1Ng/cat-llm](https://github.com/Huy1Ng/cat-llm)
```

## Usage Workflows

By default, `cat-llm` processes the current directory, formatting each valid file into a clear Markdown code block.

### 1. Snapshot an Entire Repository
Gather context for your entire codebase. You can pass multiple ignore files or patterns using a **comma-separated list**.
*(Note: Always wrap wildcard patterns in quotes so your shell doesn't expand them first!)*
```bash
cat-llm --ignore-files .llmignore,.custom_ignore --ignore-patterns '*.lock,*.log' . > context.txt
```
*(Alternatively, you can repeat the flags: `--ignore-patterns '*.lock' --ignore-patterns '*.log'`)*

### 2. Pass Specific Files
You don't have to scan whole directories; you can provide exact file paths:
```bash
cat-llm src/main.rs src/lib.rs > context.txt
```

### 3. The Git Diff Workflow (Changes Only)
If you only want the LLM to review the files you are currently working on, you can pipe `git diff` directly into `cat-llm` using `xargs`:
```bash
git diff --name-only --diff-filter=d HEAD | xargs -r cat-llm > changes.txt
```

## Output Format

For every included file, the tool outputs the file path followed by its contents enclosed in standard code fences:

````text
/path/to/src/main.rs
```
fn main() {
    println!("Hello, LLM!");
}
```

/path/to/empty_file.txt
```

```
````
*(Note: Completely empty files will contain a single blank space inside the fences to maintain valid formatting).*
