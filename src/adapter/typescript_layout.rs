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

use std::path::{Path, PathBuf};

/// What rule R1 found in one workspace folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderLayout {
    pub root: PathBuf,
    /// Whether one project contains every source of the folder.
    pub complete: bool,
    /// Whether the folder's configuration puts JavaScript files into the project. `false`
    /// when there is no single configuration to read.
    pub allow_js: bool,
}

/// Reads each workspace folder.
pub fn assess(roots: &[PathBuf]) -> Vec<FolderLayout> {
    let _ = roots;
    todo!("ADR 0023 rule R1")
}

/// Whether every folder is complete. A workspace without any known folder is not.
pub fn all_complete(layouts: &[FolderLayout]) -> bool {
    let _ = layouts;
    todo!("ADR 0023 rule R1")
}

/// Whether a change to `path` can break the verdict of [`assess`] for `layout`: a
/// configuration file anywhere in the folder, or (when `created`) a JavaScript file the
/// configuration does not take in. Paths under `node_modules` and `.git` never do.
pub fn change_breaks_verdict(layout: &FolderLayout, path: &Path, created: bool) -> bool {
    let _ = (layout, path, created);
    todo!("ADR 0023 rule R1")
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
    fn an_unreadable_config_is_not_complete() {
        let w = TempWorkspace::new("broken");
        w.write("tsconfig.json", "{ not json").write("a.ts", SOURCE);
        assert!(!complete(&w));
    }

    // --- changes during the conversation -------------------------------------------------

    #[test]
    fn a_config_file_change_breaks_the_verdict() {
        let w = TempWorkspace::new("change-config");
        w.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&w.path))[0];
        assert!(change_breaks_verdict(
            layout,
            &w.path.join("tsconfig.json"),
            false
        ));
        assert!(change_breaks_verdict(
            layout,
            &w.path.join("packages/b/tsconfig.json"),
            true
        ));
        assert!(change_breaks_verdict(
            layout,
            &w.path.join("jsconfig.json"),
            true
        ));
    }

    #[test]
    fn a_new_javascript_file_breaks_the_verdict_only_outside_allow_js() {
        let w = TempWorkspace::new("change-js");
        w.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&w.path))[0];
        assert!(change_breaks_verdict(layout, &w.path.join("b.js"), true));
        assert!(!change_breaks_verdict(layout, &w.path.join("b.js"), false));
        assert!(!change_breaks_verdict(layout, &w.path.join("b.ts"), true));

        let js = TempWorkspace::new("change-js-allowed");
        js.write("jsconfig.json", "{}").write("a.js", SOURCE);
        let layout = &assess(std::slice::from_ref(&js.path))[0];
        assert!(!change_breaks_verdict(layout, &js.path.join("b.js"), true));
    }

    #[test]
    fn changes_under_node_modules_or_outside_the_folder_do_not_break_the_verdict() {
        let w = TempWorkspace::new("change-skipped");
        w.write("tsconfig.json", "{}").write("a.ts", SOURCE);
        let layout = &assess(std::slice::from_ref(&w.path))[0];
        assert!(!change_breaks_verdict(
            layout,
            &w.path.join("node_modules/dep/tsconfig.json"),
            true
        ));
        assert!(!change_breaks_verdict(
            layout,
            &w.path.join(".git/tsconfig.json"),
            true
        ));
        assert!(!change_breaks_verdict(
            layout,
            Path::new("/elsewhere/tsconfig.json"),
            true
        ));
    }
}
