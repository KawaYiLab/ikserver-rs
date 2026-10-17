//! 错误码登记检查（AGENTS.md 规则 3）：代码里每个对外错误码都必须登记在 `docs/api-guidelines.md`。
//! 只读源码与文档，不连数据库。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// 构造对外错误码的写法：错误码是第几个参数之前要跳过的逗号数。
const CONSTRUCTORS: [(&str, usize); 3] = [
    ("Problem::new(", 1),
    ("Self::new(StatusCode::", 1),
    ("Problem::conflict(", 0),
];

/// 允许错误码不是字面量的地方：`Problem` 自己的通用构造函数把调用方给的码原样转交。
const PASS_THROUGH: &str = "crates/kernel/src/error.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 从构造调用的左括号之后取错误码参数：字面量返回 `Ok(码)`，否则返回 `Err(原样参数)`。
fn code_argument(rest: &str, skip_commas: usize) -> Result<String, String> {
    let mut depth = 0usize;
    let mut commas = 0usize;
    let mut start = 0usize;
    for (i, ch) in rest.char_indices() {
        if commas == skip_commas {
            start = i;
            break;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    let arg = rest[start..].trim_start();
    if let Some(literal) = arg.strip_prefix('"') {
        let end = literal.find('"').unwrap();
        Ok(literal[..end].to_owned())
    } else {
        let end = arg.find([',', ')']).unwrap_or(arg.len());
        Err(arg[..end].trim().to_owned())
    }
}

fn registered_codes(guidelines: &str) -> BTreeSet<String> {
    guidelines
        .lines()
        .filter_map(|line| line.strip_prefix('|')?.split('|').next())
        .map(str::trim)
        .filter(|cell| {
            !cell.is_empty()
                && cell
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_error_code_is_registered() {
    let root = workspace_root();
    let registered =
        registered_codes(&fs::read_to_string(root.join("docs/api-guidelines.md")).unwrap());

    let mut files = Vec::new();
    for krate in fs::read_dir(root.join("crates")).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }

    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(file).unwrap();
        for (needle, skip_commas) in CONSTRUCTORS {
            for (at, _) in source.match_indices(needle) {
                let open = at + needle.find('(').unwrap() + 1;
                match code_argument(&source[open..], skip_commas) {
                    Ok(code) => {
                        if !registered.contains(&code) {
                            problems.push(format!(
                                "{relative}: 错误码 {code} 没有登记在 docs/api-guidelines.md"
                            ));
                        }
                        used.insert(code);
                    }
                    Err(_) if relative == PASS_THROUGH => {}
                    Err(arg) => problems.push(format!(
                        "{relative}: 错误码必须写成字符串字面量（这里是 `{arg}`），否则无法检查是否登记"
                    )),
                }
            }
        }
    }

    assert!(
        used.len() > 50,
        "只找到 {} 个错误码，检查本身可能失效了",
        used.len()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
