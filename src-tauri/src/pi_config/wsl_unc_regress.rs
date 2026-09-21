//! Deep WSL UNC / path-normalization regression for SCHEMA 18.
//!
//! Live Windows layout these strings stand in for:
//! `\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude`
//! `\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex`
//! `\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent`
//!
//! Linux CI has no `Prefix::UNC`; parsers must still accept the same strings,
//! including trailing slashes, mixed separators, missing distro, and non-UNC.

use super::{
    canonicalize_pi_agent_dir, inferred_wsl_pi_agent_dir, is_usable_pi_agent_dir,
    is_windows_unc_path, resolve_pi_agent_dir, wsl_unc_home_from_app_config_dir,
};
use std::path::{Path, PathBuf};

const DISTRO: &str = "Ubuntu-22.04";
const USER: &str = "tfdx8045";

fn as_unc(path: &Path) -> String {
    path.to_string_lossy().replace('/', r"\")
}

fn assert_ends_unc(path: &Path, suffix: &str) {
    let got = as_unc(path);
    let want = suffix.replace('/', r"\");
    assert!(
        got.ends_with(&want),
        "expected {:?} to end with {want:?}, got {got}",
        path
    );
}

fn tfdx_unc(app_dir: &str) -> String {
    format!(r"\\wsl.localhost\{DISTRO}\home\{USER}\{app_dir}")
}

#[test]
fn canonicalize_dot_pi_tfdx8045_unc_and_posix_table() {
    let cases: &[(&str, &str)] = &[
        (
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi",
            r"home\tfdx8045\.pi\agent",
        ),
        (
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\",
            r"home\tfdx8045\.pi\agent",
        ),
        (
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi/",
            r"home\tfdx8045\.pi\agent",
        ),
        (
            r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.PI",
            r"home\tfdx8045\.PI\agent",
        ),
        (
            r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi",
            r"home/tfdx8045/.pi/agent",
        ),
        (
            r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/",
            r"home/tfdx8045/.pi/agent",
        ),
        (
            r"\\wsl.localhost\Ubuntu-22.04/home/tfdx8045/.pi",
            r"home/tfdx8045/.pi/agent",
        ),
        (
            r"\\wsl$\Ubuntu-22.04\home\tfdx8045\.pi",
            r"home\tfdx8045\.pi\agent",
        ),
        (
            r"\\?\UNC\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi",
            r"home\tfdx8045\.pi\agent",
        ),
        (
            r"\\WSL.LOCALHOST\Ubuntu-22.04\home\tfdx8045\.pi\",
            r"home\tfdx8045\.pi\agent",
        ),
        ("/home/tfdx8045/.pi", "/home/tfdx8045/.pi/agent"),
        ("/home/tfdx8045/.pi/", "/home/tfdx8045/.pi/agent"),
        ("/home/tfdx8045/.PI", "/home/tfdx8045/.PI/agent"),
    ];

    for (input, suffix) in cases {
        let canonical = canonicalize_pi_agent_dir(PathBuf::from(*input));
        let got = canonical.to_string_lossy();
        assert!(
            got.replace('\\', "/").ends_with(&suffix.replace('\\', "/"))
                || as_unc(&canonical).ends_with(&suffix.replace('/', r"\")),
            "canonicalize({input:?}) => {got} should end with {suffix}"
        );
        assert!(
            !got.to_ascii_lowercase().contains("pi-wsl-sessions"),
            "must not invent a C: session mirror: {got}"
        );
    }
}

#[test]
fn canonicalize_dot_pi_agent_dir_is_idempotent() {
    let already: &[&str] = &[
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent",
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent\",
        r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/agent",
        r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/agent/",
        "/home/tfdx8045/.pi/agent",
        "/home/tfdx8045/.pi/agent/",
        r"\\wsl$\Ubuntu-22.04\root\.pi\agent",
    ];
    for input in already {
        let canonical = canonicalize_pi_agent_dir(PathBuf::from(*input));
        let got = canonical.to_string_lossy();
        assert!(
            got.replace('\\', "/")
                .trim_end_matches('/')
                .ends_with(".pi/agent")
                || as_unc(&canonical)
                    .trim_end_matches('\\')
                    .ends_with(r".pi\agent"),
            "agent dir must stay under .pi/agent: {input:?} => {got}"
        );
        assert!(
            !got.replace('\\', "/")
                .trim_end_matches('/')
                .ends_with(".pi/agent/agent"),
            "must not append a second agent segment: {got}"
        );
    }
}

#[test]
fn canonicalize_dot_pi_leaves_unrelated_paths_alone() {
    let unchanged: &[&str] = &[
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude",
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.codex",
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi-extra",
        "/home/tfdx8045/.claude",
        "/opt/pi",
        r"C:\Users\tfdx8045\.pi-not",
    ];
    for input in unchanged {
        assert_eq!(
            canonicalize_pi_agent_dir(PathBuf::from(*input)),
            PathBuf::from(*input),
            "must not rewrite {input:?}"
        );
    }
}

#[test]
fn inferred_wsl_pi_agent_follows_claude_or_codex_tfdx8045_home() {
    let claude = PathBuf::from(tfdx_unc(".claude"));
    let inferred = inferred_wsl_pi_agent_dir(Some(&claude), None).expect("Claude UNC home");
    assert_ends_unc(&inferred, r"home\tfdx8045\.pi\agent");
    assert!(as_unc(&inferred).contains(DISTRO));

    let claude_slash = PathBuf::from(format!(r"{}\", tfdx_unc(".claude")));
    let from_slash =
        inferred_wsl_pi_agent_dir(Some(&claude_slash), None).expect("trailing slash Claude");
    assert_ends_unc(&from_slash, r"home\tfdx8045\.pi\agent");

    let mixed = PathBuf::from(r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.claude/");
    let from_mixed = inferred_wsl_pi_agent_dir(Some(&mixed), None).expect("mixed separators");
    assert_ends_unc(&from_mixed, r"home\tfdx8045\.pi\agent");

    let mixed_inner = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04/home/tfdx8045/.codex");
    let from_inner =
        inferred_wsl_pi_agent_dir(None, Some(&mixed_inner)).expect("inner mixed Claude/Codex");
    assert_ends_unc(&from_inner, r"home\tfdx8045\.pi\agent");

    let verbatim = PathBuf::from(r"\\?\UNC\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude");
    let from_verbatim = inferred_wsl_pi_agent_dir(Some(&verbatim), None).expect("verbatim UNC");
    assert_ends_unc(&from_verbatim, r"home\tfdx8045\.pi\agent");
    assert!(
        as_unc(&from_verbatim).starts_with(r"\\?\UNC\"),
        "verbatim prefix must be preserved: {}",
        as_unc(&from_verbatim)
    );

    let wsl_dollar = PathBuf::from(r"\\wsl$\Ubuntu-22.04\home\tfdx8045\.codex");
    let from_dollar = inferred_wsl_pi_agent_dir(None, Some(&wsl_dollar)).expect("wsl$ Codex");
    assert_ends_unc(&from_dollar, r"home\tfdx8045\.pi\agent");

    let root_codex = PathBuf::from(r"\\wsl.localhost\Ubuntu-22.04\root\.codex\");
    let from_root = inferred_wsl_pi_agent_dir(None, Some(&root_codex)).expect("root Codex");
    assert_ends_unc(&from_root, r"root\.pi\agent");
}

#[test]
fn inferred_wsl_pi_agent_prefers_claude_over_codex() {
    let claude = PathBuf::from(tfdx_unc(".claude"));
    let other = PathBuf::from(r"\\wsl.localhost\Debian\home\other\.codex");
    let inferred = inferred_wsl_pi_agent_dir(Some(&claude), Some(&other)).expect("prefer Claude");
    assert!(
        as_unc(&inferred).contains(DISTRO) && as_unc(&inferred).contains(USER),
        "Claude home must win: {}",
        as_unc(&inferred)
    );
}

#[test]
fn inferred_wsl_pi_agent_rejects_missing_distro_and_non_unc() {
    let missing_distro: &[&str] = &[
        r"\\wsl.localhost",
        r"\\wsl.localhost\",
        r"\\wsl.localhost\.claude",
        r"\\wsl.localhost\home\tfdx8045\.claude",
        r"\\wsl$\",
        r"\\wsl$",
        r"\\wsl.localhost\Ubuntu-22.04\.claude",
        r"\\wsl.localhost\Ubuntu-22.04\home\.claude",
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045",
        r"\\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude\projects",
        r"\\wsl.localhost\Ubuntu-22.04\opt\claude\.claude",
    ];
    for input in missing_distro {
        assert!(
            inferred_wsl_pi_agent_dir(Some(Path::new(*input)), None).is_none(),
            "must not infer a Pi agent dir from {input:?}"
        );
        assert!(
            wsl_unc_home_from_app_config_dir(Path::new(*input)).is_none(),
            "must not extract a WSL home from {input:?}"
        );
    }

    let non_unc: &[&str] = &[
        r"/home/tfdx8045/.claude",
        r"/home/tfdx8045/.codex",
        r"/home/tfdx8045/.pi/agent",
        r"C:\Users\tfdx8045\.claude",
        r"D:\profiles\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude",
        r"\\nas\share\home\tfdx8045\.claude",
        r"\\localhost\c$\Users\tfdx8045\.claude",
        r"\\192.168.1.1\share\.claude",
        "",
        ".",
        r"wsl.localhost\Ubuntu-22.04\home\tfdx8045\.claude",
        r"home/tfdx8045/.claude",
    ];
    for input in non_unc {
        assert!(
            inferred_wsl_pi_agent_dir(Some(Path::new(*input)), None).is_none(),
            "non-UNC must not infer WSL Pi home: {input:?}"
        );
    }
}

#[test]
fn resolve_pi_agent_dir_accepts_tfdx8045_unc_edge_overrides() {
    let unused = PathBuf::from("/unused");
    let agent = tfdx_unc(r".pi\agent");
    let resolved = resolve_pi_agent_dir(Some(PathBuf::from(&agent)), None, unused.clone())
        .expect("UNC agent override");
    assert_eq!(resolved, PathBuf::from(&agent));
    assert!(is_usable_pi_agent_dir(&resolved));

    let from_root = resolve_pi_agent_dir(
        Some(PathBuf::from(format!(r"{}\", tfdx_unc(".pi")))),
        None,
        unused.clone(),
    )
    .expect("trailing-slash .pi override");
    assert_ends_unc(&from_root, r"home\tfdx8045\.pi\agent");

    let mixed = resolve_pi_agent_dir(
        Some(PathBuf::from(
            r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/",
        )),
        None,
        unused.clone(),
    )
    .expect("mixed-separator .pi override");
    let mixed_text = mixed.to_string_lossy();
    assert!(
        mixed_text
            .replace('\\', "/")
            .ends_with("tfdx8045/.pi/agent"),
        "mixed UNC .pi must land on agent: {mixed_text}"
    );

    let posix = resolve_pi_agent_dir(Some(PathBuf::from("/home/tfdx8045/.pi/")), None, unused)
        .expect("POSIX .pi");
    assert_eq!(posix, PathBuf::from("/home/tfdx8045/.pi/agent"));
}

#[test]
fn windows_unc_recognition_covers_wsl_and_rejects_posix() {
    assert!(is_windows_unc_path(Path::new(&tfdx_unc(".pi\\agent"))));
    assert!(is_windows_unc_path(Path::new(
        r"//wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/agent"
    )));
    assert!(is_windows_unc_path(Path::new(
        r"\\?\UNC\wsl.localhost\Ubuntu-22.04\home\tfdx8045\.pi\agent"
    )));
    assert!(is_windows_unc_path(Path::new(
        r"\\wsl$\Ubuntu-22.04\home\tfdx8045\.pi\"
    )));
    assert!(is_usable_pi_agent_dir(Path::new(&tfdx_unc(".pi\\agent"))));
    assert!(is_usable_pi_agent_dir(Path::new(
        "/home/tfdx8045/.pi/agent"
    )));

    assert!(!is_windows_unc_path(Path::new("/home/tfdx8045/.pi/agent")));
    assert!(!is_windows_unc_path(Path::new(
        r"C:\Users\tfdx8045\.pi\agent"
    )));
    assert!(!is_usable_pi_agent_dir(Path::new("relative/pi-agent")));
    assert!(!is_usable_pi_agent_dir(Path::new(
        r"home\tfdx8045\.pi\agent"
    )));
}

#[test]
fn linux_standin_layout_string_never_looks_like_windows_c_mirror() {
    let standin = PathBuf::from(
        "/tmp/cc-switch-test-home/profiles/wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/agent",
    );
    let resolved = resolve_pi_agent_dir(Some(standin.clone()), None, PathBuf::from("/unused"))
        .expect("stand-in");
    assert_eq!(resolved, standin);
    let text = resolved.to_string_lossy();
    assert!(text.ends_with("/home/tfdx8045/.pi/agent"));
    assert!(!text.contains("C:") && !text.to_ascii_lowercase().contains("c:\\"));
    assert!(!text.to_ascii_lowercase().contains("pi-wsl-sessions"));

    let from_dot_pi = canonicalize_pi_agent_dir(PathBuf::from(
        "/tmp/cc-switch-test-home/profiles/wsl.localhost/Ubuntu-22.04/home/tfdx8045/.pi/",
    ));
    assert_eq!(from_dot_pi, standin);
}
