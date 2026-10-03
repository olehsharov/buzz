use super::{first_candidate_in, sidecar_search_dirs};
use std::path::{Path, PathBuf};

const EXE_DIR: &str = "/Applications/Buzz.app/Contents/MacOS";
const WORKSPACE: &str = "/src/buzz";
const CWD: &str = "/home/dev/checkout";

const BUNDLED: &str = "/Applications/Buzz.app/Contents/MacOS/buzz-acp";
const WS_DEBUG: &str = "/src/buzz/target/debug/buzz-acp";
const WS_RELEASE: &str = "/src/buzz/target/release/buzz-acp";
const CWD_DEBUG: &str = "/home/dev/checkout/target/debug/buzz-acp";

fn resolve(is_debug: bool, exe_dir: Option<&str>, present: &[&str]) -> Option<PathBuf> {
    let dirs = sidecar_search_dirs(
        is_debug,
        exe_dir.map(PathBuf::from),
        Path::new(WORKSPACE),
        Some(PathBuf::from(CWD)),
    );
    first_candidate_in(&dirs, "buzz-acp", |candidate| {
        present.iter().any(|p| candidate == Path::new(p))
    })
}

/// Table over build profile x exe-dir availability x where candidates exist.
#[test]
fn sidecar_resolution_table() {
    type Case<'a> = (bool, Option<&'a str>, &'a [&'a str], Option<&'a str>);
    #[rustfmt::skip]
    let cases: &[Case] = &[
        // Release: the bundled sidecar wins even when stale workspace builds exist.
        (false, Some(EXE_DIR), &[BUNDLED, WS_DEBUG, WS_RELEASE, CWD_DEBUG], Some(BUNDLED)),
        (false, Some(EXE_DIR), &[BUNDLED], Some(BUNDLED)),
        // Release never falls back to the compile-time workspace or cwd target dirs.
        (false, Some(EXE_DIR), &[WS_DEBUG, WS_RELEASE, CWD_DEBUG], None),
        (false, None, &[WS_DEBUG, WS_RELEASE, CWD_DEBUG], None),
        (false, Some(EXE_DIR), &[], None),
        // Debug: fresh workspace target/debug beats release output and the exe dir.
        (true, Some(EXE_DIR), &[BUNDLED, WS_DEBUG, WS_RELEASE, CWD_DEBUG], Some(WS_DEBUG)),
        (true, Some(EXE_DIR), &[BUNDLED, WS_RELEASE, CWD_DEBUG], Some(WS_RELEASE)),
        (true, Some(EXE_DIR), &[BUNDLED, CWD_DEBUG], Some(CWD_DEBUG)),
        (true, Some(EXE_DIR), &[BUNDLED], Some(BUNDLED)),
        (true, None, &[BUNDLED], None),
        (true, None, &[WS_DEBUG], Some(WS_DEBUG)),
        (true, Some(EXE_DIR), &[], None),
    ];
    for (is_debug, exe_dir, present, expected) in cases {
        assert_eq!(
            resolve(*is_debug, *exe_dir, present),
            expected.map(PathBuf::from),
            "is_debug={is_debug} exe_dir={exe_dir:?} present={present:?}"
        );
    }
}

#[test]
fn release_search_dirs_are_only_the_exe_dir() {
    assert_eq!(
        sidecar_search_dirs(
            false,
            Some(PathBuf::from(EXE_DIR)),
            Path::new(WORKSPACE),
            Some(PathBuf::from(CWD)),
        ),
        vec![PathBuf::from(EXE_DIR)]
    );
}

#[test]
fn debug_search_dirs_dedupe_when_cwd_is_workspace() {
    assert_eq!(
        sidecar_search_dirs(
            true,
            Some(PathBuf::from(EXE_DIR)),
            Path::new(WORKSPACE),
            Some(PathBuf::from(WORKSPACE)),
        ),
        vec![
            PathBuf::from("/src/buzz/target/debug"),
            PathBuf::from("/src/buzz/target/release"),
            PathBuf::from(EXE_DIR),
        ]
    );
}
