//! Reading the layout of a TypeScript workspace's configuration files (ADR 0023, rule R1).
//!
//! tsserver searches only the projects it has loaded: the project of each opened file and the
//! projects reachable from a loaded solution. `textDocument/references` is therefore complete
//! over the whole workspace only when a single project already contains every source. Rule R1
//! deems a workspace folder complete only when all of the following hold:
//!
//! - exactly one `tsconfig.json` / `jsconfig.json` exists under the folder (`node_modules` and
//!   `.git` are skipped), and it sits at the folder root
//! - it has no `extends` (the inherited `exclude` or `allowJs` could narrow the project)
//! - its `files` / `include` / `exclude` contain every source file under the folder, with
//!   tsserver's own matching rules (a wildcard does not match a name starting with `.`)
//! - JavaScript sources exist only if the configuration makes `allowJs` effective
//!
//! Nothing is asked of tsserver. Whatever the reader does not understand counts as incomplete:
//! what is lost is only the promise, never an answer (ADR 0023).

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// The configuration files tsserver looks for, nearest first.
const CONFIG_NAMES: [&str; 2] = ["tsconfig.json", "jsconfig.json"];
/// Directories that are not part of the workspace's own sources.
const SKIPPED_DIRS: [&str; 2] = ["node_modules", ".git"];
const TYPESCRIPT_EXTENSIONS: [&str; 4] = ["ts", "tsx", "mts", "cts"];
const JAVASCRIPT_EXTENSIONS: [&str; 4] = ["js", "jsx", "mjs", "cjs"];
/// tsserver's `exclude` when a configuration writes none (the `outDir` is added to it).
const DEFAULT_EXCLUDE: [&str; 3] = ["node_modules", "bower_components", "jspm_packages"];

/// What rule R1 found in one workspace folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderLayout {
    pub root: PathBuf,
    /// Whether one project contains every source of the folder.
    pub complete: bool,
    /// Whether the folder's configuration puts JavaScript files into the project. `false`
    /// when there is no single configuration to read.
    pub allow_js: bool,
    /// The files the configuration takes in. `None` when there is no single configuration to
    /// read.
    scope: Option<ProjectScope>,
}

/// A configuration's `files` / `include` / `exclude`, compiled for tsserver's matching.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectScope {
    files: Vec<String>,
    include: Vec<Vec<String>>,
    exclude: Vec<Vec<String>>,
}

/// Reads each workspace folder.
pub fn assess(roots: &[PathBuf]) -> Vec<FolderLayout> {
    roots.iter().map(|root| assess_folder(root)).collect()
}

/// Whether every folder is complete. A workspace without any known folder is not.
pub fn all_complete(layouts: &[FolderLayout]) -> bool {
    !layouts.is_empty() && layouts.iter().all(|layout| layout.complete)
}

/// The path relative to `layout`'s folder, or `None` for a path outside it or under
/// `node_modules` / `.git`.
fn inside<'a>(layout: &FolderLayout, path: &'a Path) -> Option<&'a Path> {
    let relative = path.strip_prefix(&layout.root).ok()?;
    let skipped = relative.components().any(|component| {
        matches!(component, Component::Normal(name) if SKIPPED_DIRS.iter().any(|d| name == *d))
    });
    (!skipped).then_some(relative)
}

fn is_config_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CONFIG_NAMES.contains(&name))
}

/// Whether `path` is a configuration file in `layout`'s folder, whose change calls for
/// [`reassess`] (ADR 0023 addendum 2026-09-26). Paths under `node_modules` and `.git` are not.
pub fn is_config_change(layout: &FolderLayout, path: &Path) -> bool {
    inside(layout, path).is_some() && is_config_file(path)
}

/// Whether a newly created `path` is a source in `layout`'s folder that the configuration does
/// not take in (by `files` / `include` / `exclude`, and `allowJs` for JavaScript). Paths under
/// `node_modules` and `.git` are not.
pub fn new_source_outside(layout: &FolderLayout, path: &Path) -> bool {
    if inside(layout, path).is_none() || is_config_file(path) {
        return false;
    }
    let is_javascript = has_extension(path, &JAVASCRIPT_EXTENSIONS);
    if !is_javascript && !has_extension(path, &TYPESCRIPT_EXTENSIONS) {
        return false;
    }
    let takes_in = (!is_javascript || layout.allow_js)
        && layout.scope.as_ref().is_some_and(|scope| {
            relative_slash_path(&layout.root, path).is_some_and(|source| scope.takes_in(&source))
        });
    !takes_in
}

/// Reads `layout`'s folder again from the disk as it is now.
pub fn reassess(layout: &FolderLayout) -> FolderLayout {
    assess_folder(&layout.root)
}

fn assess_folder(root: &Path) -> FolderLayout {
    let incomplete = |allow_js| FolderLayout {
        root: root.to_path_buf(),
        complete: false,
        allow_js,
        scope: None,
    };
    let mut configs = Vec::new();
    let mut sources = Vec::new();
    if walk(root, root, &mut configs, &mut sources).is_err() {
        return incomplete(false);
    }
    let [config] = configs.as_slice() else {
        return incomplete(false);
    };
    if config.parent() != Some(root) {
        return incomplete(false);
    }
    let Some(settings) = std::fs::read_to_string(config)
        .ok()
        .and_then(|text| strip_jsonc(&text))
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .filter(Value::is_object)
    else {
        return incomplete(false);
    };
    let is_jsconfig = config
        .file_name()
        .is_some_and(|name| name == "jsconfig.json");
    // jsconfig.json turns `allowJs` on unless it says otherwise; tsconfig.json leaves it off.
    let allow_js = match compiler_option(&settings, "allowJs") {
        Ok(None) => is_jsconfig,
        Ok(Some(Value::Bool(value))) => *value,
        _ => return incomplete(false),
    };
    let Some(scope) = ProjectScope::read(&settings) else {
        return incomplete(allow_js);
    };
    let complete = settings.get("extends").is_none()
        && sources.iter().all(|source| {
            (allow_js || !has_extension(Path::new(source), &JAVASCRIPT_EXTENSIONS))
                && scope.takes_in(source)
        });
    FolderLayout {
        root: root.to_path_buf(),
        complete,
        allow_js,
        scope: Some(scope),
    }
}

/// Collects the configuration files (absolute paths) and the source files (paths relative to
/// `root`, joined by `/`). Symbolic links are not followed.
fn walk(
    root: &Path,
    dir: &Path,
    configs: &mut Vec<PathBuf>,
    sources: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let name = entry.file_name();
        if file_type.is_dir() {
            if !SKIPPED_DIRS.iter().any(|skipped| name == *skipped) {
                walk(root, &path, configs, sources)?;
            }
        } else if file_type.is_file() {
            if CONFIG_NAMES.iter().any(|config| name == *config) {
                configs.push(path);
            } else if has_extension(&path, &TYPESCRIPT_EXTENSIONS)
                || has_extension(&path, &JAVASCRIPT_EXTENSIONS)
            {
                let Some(relative) = relative_slash_path(root, &path) else {
                    // A name that is not UTF-8 cannot be matched against the patterns.
                    sources.push(String::new());
                    continue;
                };
                sources.push(relative);
            }
        }
    }
    Ok(())
}

fn relative_slash_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extensions.contains(&extension))
}

impl ProjectScope {
    /// `None` when the configuration writes something this reader does not understand.
    fn read(settings: &Value) -> Option<Self> {
        let list = |key: &str| -> Result<Option<Vec<String>>, ()> {
            match settings.get(key) {
                None => Ok(None),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| item.as_str().map(normalize_entry).ok_or(()))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some),
                Some(_) => Err(()),
            }
        };
        let (Ok(files), Ok(include), Ok(exclude)) =
            (list("files"), list("include"), list("exclude"))
        else {
            return None;
        };
        let include = include.unwrap_or_else(|| {
            if files.is_some() {
                Vec::new()
            } else {
                vec!["**/*".to_string()]
            }
        });
        let exclude = match exclude {
            Some(exclude) => exclude,
            None => {
                let mut defaults: Vec<String> =
                    DEFAULT_EXCLUDE.iter().map(|d| d.to_string()).collect();
                match compiler_option(settings, "outDir") {
                    Ok(None) => {}
                    Ok(Some(Value::String(out_dir))) => defaults.push(normalize_entry(out_dir)),
                    _ => return None,
                }
                defaults
            }
        };
        Some(ProjectScope {
            files: files.unwrap_or_default(),
            include: compile_all(&include)?,
            exclude: compile_all(&exclude)?,
        })
    }

    /// Whether a source (relative to the folder, joined by `/`) belongs to the project.
    fn takes_in(&self, source: &str) -> bool {
        if source.is_empty() {
            return false;
        }
        if self.files.iter().any(|file| file == source) {
            return true;
        }
        let segments: Vec<&str> = source.split('/').collect();
        self.include
            .iter()
            .any(|pattern| matches(pattern, &segments))
            && !self.exclude.iter().any(|pattern| {
                // An `exclude` pattern also drops everything under a directory it matches.
                (1..=segments.len()).any(|end| matches(pattern, &segments[..end]))
            })
    }
}

/// A `compilerOptions` entry: `Ok(None)` when it is not written, `Err` when `compilerOptions`
/// is written but is not an object. An explicit `null` comes back as `Ok(Some(Null))`, for the
/// caller to reject.
fn compiler_option<'a>(settings: &'a Value, key: &str) -> Result<Option<&'a Value>, ()> {
    match settings.get("compilerOptions") {
        None => Ok(None),
        Some(Value::Object(options)) => Ok(options.get(key)),
        Some(_) => Err(()),
    }
}

/// Drops a leading `./` (any number of them).
fn normalize_entry(entry: &str) -> String {
    let mut entry = entry.trim();
    while let Some(rest) = entry.strip_prefix("./") {
        entry = rest;
    }
    entry.to_string()
}

/// A path pattern split into segments. `None` for a pattern this reader does not handle
/// (absolute, `..`, backslashes, empty): the caller then treats the folder as incomplete.
fn compile(pattern: &str) -> Option<Vec<String>> {
    if pattern.is_empty() || pattern.starts_with('/') || pattern.contains('\\') {
        return None;
    }
    let mut segments: Vec<String> = pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(str::to_string)
        .collect();
    if segments.is_empty() || segments.iter().any(|segment| segment == "..") {
        return None;
    }
    // A last segment without a wildcard or an extension names a directory.
    let last = segments.last().unwrap();
    if !has_wildcard(last) && !last.contains('.') {
        segments.push("**".to_string());
        segments.push("*".to_string());
    }
    Some(segments)
}

fn compile_all(patterns: &[String]) -> Option<Vec<Vec<String>>> {
    patterns.iter().map(|pattern| compile(pattern)).collect()
}

fn has_wildcard(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?')
}

/// tsserver's matching: `**` stands for any number of directories, `*` and `?` for characters
/// within one segment, and a wildcard never matches a name starting with `.`.
fn matches(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => (0..=path.len()).any(|skip| {
            path[..skip].iter().all(|dir| !dir.starts_with('.')) && matches(rest, &path[skip..])
        }),
        Some((first, rest)) => match path.split_first() {
            Some((name, remaining)) => matches_segment(first, name) && matches(rest, remaining),
            None => false,
        },
    }
}

fn matches_segment(pattern: &str, name: &str) -> bool {
    if !has_wildcard(pattern) {
        return pattern == name;
    }
    if name.starts_with('.') {
        return false;
    }
    // tsserver keeps minified files out of a wildcard unless the pattern names them.
    if pattern.contains('*') && name.ends_with(".min.js") && !pattern.ends_with(".min.js") {
        return false;
    }
    wildcard(pattern.as_bytes(), name.as_bytes())
}

fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| wildcard(rest, &name[skip..])),
        Some((b'?', rest)) => !name.is_empty() && wildcard(rest, &name[1..]),
        Some((c, rest)) => name.first() == Some(c) && wildcard(rest, &name[1..]),
    }
}

/// Removes the comments and trailing commas that tsconfig.json allows, so that the text parses
/// as JSON. Strings are left alone. `None` for a block comment that never closes.
fn strip_jsonc(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                let mut closed = false;
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        closed = true;
                        break;
                    }
                    previous = next;
                }
                if !closed {
                    return None;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    Some(remove_trailing_commas(&out))
}

/// Drops a comma followed only by whitespace before `}` or `]` (outside strings).
fn remove_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|next| !next.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// A throwaway workspace folder for tests, cleaned up on drop. No dependency is added for
/// this (`CLAUDE.md`'s absolute constraint).
#[cfg(test)]
pub(crate) struct TempWorkspace {
    pub path: PathBuf,
}

#[cfg(test)]
impl TempWorkspace {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lsp-det-typescript-layout-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create the temporary directory");
        TempWorkspace { path }
    }

    /// Writes `contents` at `relative`, creating the directories on the way.
    pub fn write(&self, relative: &str, contents: &str) -> &Self {
        let path = self.path.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        self
    }
}

#[cfg(test)]
impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "export const x = 1;\n";

    fn complete(workspace: &TempWorkspace) -> bool {
        all_complete(&assess(std::slice::from_ref(&workspace.path)))
    }

    // --- complete layouts --------------------------------------------------------------

    #[test]
    fn a_single_root_config_with_the_default_include_is_complete() {
        let w = TempWorkspace::new("default-include");
        w.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#)
            .write("a.ts", SOURCE)
            .write("src/deep/b.tsx", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn the_conformance_fixture_layout_is_complete() {
        // The layout of the 7.2 fixture, the basis of the declaration so far.
        let w = TempWorkspace::new("fixture");
        w.write(
            "tsconfig.json",
            r#"{"compilerOptions":{"strict":true},"include":["**/*.ts"]}"#,
        )
        .write("a.ts", SOURCE)
        .write("b.ts", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn an_include_naming_a_directory_takes_in_everything_under_it() {
        let w = TempWorkspace::new("dir-include");
        w.write("tsconfig.json", r#"{"include":["src"]}"#)
            .write("src/a.ts", SOURCE)
            .write("src/lib/b.ts", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn comments_and_trailing_commas_are_read() {
        let w = TempWorkspace::new("jsonc");
        w.write(
            "tsconfig.json",
            "{\n  // a comment\n  \"compilerOptions\": {\"strict\": true, /* inline */},\n  \"include\": [\"**/*\",],\n}\n",
        )
        .write("a.ts", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn node_modules_and_git_are_not_part_of_the_workspace() {
        let w = TempWorkspace::new("skipped");
        w.write("tsconfig.json", "{}")
            .write("a.ts", SOURCE)
            .write("node_modules/dep/tsconfig.json", "{}")
            .write("node_modules/dep/index.js", SOURCE)
            .write(".git/hooks/x.ts", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn javascript_is_complete_under_a_jsconfig() {
        let w = TempWorkspace::new("jsconfig");
        w.write("jsconfig.json", "{}")
            .write("a.js", SOURCE)
            .write("b.ts", SOURCE);
        let layouts = assess(std::slice::from_ref(&w.path));
        assert!(all_complete(&layouts));
        assert!(layouts[0].allow_js);
    }

    #[test]
    fn javascript_is_complete_under_a_tsconfig_that_allows_it() {
        let w = TempWorkspace::new("allowjs");
        w.write("tsconfig.json", r#"{"compilerOptions":{"allowJs":true}}"#)
            .write("a.js", SOURCE)
            .write("b.ts", SOURCE);
        assert!(complete(&w));
    }

    #[test]
    fn every_folder_must_be_complete() {
        let good = TempWorkspace::new("two-good");
        good.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let other = TempWorkspace::new("two-other");
        other.write("tsconfig.json", "{}").write("b.ts", SOURCE);
        assert!(all_complete(&assess(&[
            good.path.clone(),
            other.path.clone()
        ])));

        let bad = TempWorkspace::new("two-bad");
        bad.write("b.ts", SOURCE);
        assert!(!all_complete(&assess(&[
            good.path.clone(),
            bad.path.clone()
        ])));
    }

    // --- incomplete layouts ------------------------------------------------------------

    #[test]
    fn no_folder_is_not_complete() {
        assert!(!all_complete(&assess(&[])));
    }

    #[test]
    fn a_workspace_without_a_config_is_not_complete() {
        // Every file would go into an inferred project of its own.
        let w = TempWorkspace::new("no-config");
        w.write("a.ts", SOURCE).write("b.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn several_projects_without_a_solution_are_not_complete() {
        // The layout measured in research/typescript-language-server-no-solution-coverage.md.
        let w = TempWorkspace::new("no-solution");
        w.write("packages/a/tsconfig.json", "{}")
            .write("packages/a/index.ts", SOURCE)
            .write("packages/b/tsconfig.json", "{}")
            .write("packages/b/use.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn a_root_solution_is_not_complete_under_rule_r1() {
        // Rule R2 would accept this; R1 comes first (ADR 0023 decision 2).
        let w = TempWorkspace::new("solution");
        w.write(
            "tsconfig.json",
            r#"{"files":[],"references":[{"path":"packages/a"}]}"#,
        )
        .write(
            "packages/a/tsconfig.json",
            r#"{"compilerOptions":{"composite":true}}"#,
        )
        .write("packages/a/index.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn a_single_config_below_the_root_is_not_complete() {
        let w = TempWorkspace::new("nested");
        w.write("app/tsconfig.json", "{}")
            .write("app/a.ts", SOURCE)
            .write("tools/b.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn a_config_with_extends_is_not_complete() {
        let w = TempWorkspace::new("extends");
        w.write("tsconfig.json", r#"{"extends":"./base.json"}"#)
            .write("base.json", "{}")
            .write("a.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn javascript_outside_allow_js_is_not_complete() {
        let w = TempWorkspace::new("js-outside");
        w.write("tsconfig.json", "{}")
            .write("a.ts", SOURCE)
            .write("tools/build.mjs", SOURCE);
        let layouts = assess(std::slice::from_ref(&w.path));
        assert!(!all_complete(&layouts));
        assert!(!layouts[0].allow_js);
    }

    #[test]
    fn javascript_under_a_jsconfig_that_disallows_it_is_not_complete() {
        let w = TempWorkspace::new("jsconfig-off");
        w.write("jsconfig.json", r#"{"compilerOptions":{"allowJs":false}}"#)
            .write("a.js", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn a_source_outside_the_include_is_not_complete() {
        let w = TempWorkspace::new("outside-include");
        w.write("tsconfig.json", r#"{"include":["src"]}"#)
            .write("src/a.ts", SOURCE)
            .write("scripts/b.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn an_excluded_source_is_not_complete() {
        let w = TempWorkspace::new("excluded");
        w.write("tsconfig.json", r#"{"exclude":["test"]}"#)
            .write("a.ts", SOURCE)
            .write("test/a.test.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn files_without_include_take_in_only_the_listed_files() {
        let w = TempWorkspace::new("files");
        w.write("tsconfig.json", r#"{"files":["a.ts"]}"#)
            .write("a.ts", SOURCE)
            .write("b.ts", SOURCE);
        assert!(!complete(&w));

        let all = TempWorkspace::new("files-all");
        all.write("tsconfig.json", r#"{"files":["./a.ts","b.ts"]}"#)
            .write("a.ts", SOURCE)
            .write("b.ts", SOURCE);
        assert!(complete(&all));
    }

    #[test]
    fn a_source_in_a_dot_directory_is_not_complete() {
        // tsserver's wildcards do not match a name starting with ".".
        let w = TempWorkspace::new("dot-dir");
        w.write("tsconfig.json", "{}")
            .write("a.ts", SOURCE)
            .write(".storybook/main.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn an_allow_js_value_that_is_not_a_boolean_is_not_complete() {
        let js = TempWorkspace::new("allowjs-string-js");
        js.write(
            "jsconfig.json",
            r#"{"compilerOptions":{"allowJs":"false"}}"#,
        )
        .write("a.js", SOURCE);
        assert!(!complete(&js));

        let ts = TempWorkspace::new("allowjs-string-ts");
        ts.write("tsconfig.json", r#"{"compilerOptions":{"allowJs":"true"}}"#)
            .write("a.ts", SOURCE);
        assert!(!complete(&ts));
    }

    #[test]
    fn compiler_options_this_reader_cannot_interpret_are_not_complete() {
        for (tag, config) in [
            ("allowjs-null", r#"{"compilerOptions":{"allowJs":null}}"#),
            ("options-array", r#"{"compilerOptions":[]}"#),
            ("options-null", r#"{"compilerOptions":null}"#),
            ("outdir-number", r#"{"compilerOptions":{"outDir":1}}"#),
        ] {
            let w = TempWorkspace::new(tag);
            w.write("jsconfig.json", config).write("a.js", SOURCE);
            assert!(!complete(&w), "deemed complete: {config}");
        }
    }

    #[test]
    fn an_unterminated_block_comment_is_not_complete() {
        let w = TempWorkspace::new("open-comment");
        w.write("tsconfig.json", r#"{"include":["**/*"]} /*"#)
            .write("a.ts", SOURCE);
        assert!(!complete(&w));
    }

    #[test]
    fn an_unreadable_config_is_not_complete() {
        let w = TempWorkspace::new("broken");
        w.write("tsconfig.json", "{ not json").write("a.ts", SOURCE);
        assert!(!complete(&w));
    }

    // --- changes during the conversation -------------------------------------------------

    #[test]
    fn config_files_in_the_folder_are_layout_changes() {
        let w = TempWorkspace::new("is-config-change");
        w.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&w.path))[0];
        for inside in ["tsconfig.json", "jsconfig.json", "packages/b/tsconfig.json"] {
            assert!(is_config_change(layout, &w.path.join(inside)), "{inside}");
        }
        for outside in [
            "a.ts",
            "node_modules/dep/tsconfig.json",
            ".git/tsconfig.json",
        ] {
            assert!(
                !is_config_change(layout, &w.path.join(outside)),
                "{outside}"
            );
        }
        assert!(!is_config_change(
            layout,
            Path::new("/elsewhere/tsconfig.json")
        ));
    }

    #[test]
    fn new_sources_outside_the_project_are_detected() {
        let w = TempWorkspace::new("new-source");
        w.write(
            "tsconfig.json",
            r#"{"compilerOptions":{"allowJs":true},"include":["src"]}"#,
        )
        .write("src/a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&w.path))[0];
        for outside in ["tools.ts", "tools.js", "src/.hidden/b.ts"] {
            assert!(
                new_source_outside(layout, &w.path.join(outside)),
                "{outside}"
            );
        }
        for inside in [
            "src/b.ts",
            "src/b.js",
            "node_modules/dep/x.ts",
            "tsconfig.json",
            "README.md",
        ] {
            assert!(
                !new_source_outside(layout, &w.path.join(inside)),
                "{inside}"
            );
        }

        let ts = TempWorkspace::new("new-source-ts");
        ts.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&ts.path))[0];
        assert!(new_source_outside(layout, &ts.path.join("b.js")));
        assert!(!new_source_outside(layout, &ts.path.join("b.ts")));

        let js = TempWorkspace::new("new-source-js");
        js.write("jsconfig.json", "{}").write("a.js", SOURCE);
        let layout = &assess(std::slice::from_ref(&js.path))[0];
        assert!(!new_source_outside(layout, &js.path.join("b.js")));
    }

    #[test]
    fn reassessing_reads_the_folder_again() {
        let w = TempWorkspace::new("reassess");
        w.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = assess(std::slice::from_ref(&w.path)).remove(0);
        assert!(layout.complete);

        w.write("tsconfig.json", r#"{"compilerOptions":{"strict":false}}"#);
        assert!(
            reassess(&layout).complete,
            "a change that keeps every source"
        );

        w.write("tsconfig.json", r#"{"exclude":["a.ts"]}"#);
        assert!(!reassess(&layout).complete, "a change that drops a source");

        std::fs::remove_file(w.path.join("tsconfig.json")).unwrap();
        assert!(!reassess(&layout).complete, "the configuration is gone");
    }
}
