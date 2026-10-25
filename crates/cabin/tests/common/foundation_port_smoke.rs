use std::path::PathBuf;

use super::cabin;

pub struct PortBuildRun<'a> {
    pub label: &'a str,
    pub manifest: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// `--index-path` registry for consumers of `cabin-ports/*`
    /// packages.
    pub index_path: Option<PathBuf>,
    pub expected_stdout: &'a [&'a str],
}

pub fn run_port_build_then_run(spec: &PortBuildRun<'_>) -> String {
    let mut build = cabin();
    build
        .args(["build", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(&spec.build_dir)
        .arg("--cache-dir")
        .arg(&spec.cache_dir);
    if let Some(index) = &spec.index_path {
        build.arg("--index-path").arg(index);
    }
    build.assert().success();

    let mut run = cabin();
    run.args(["run", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(&spec.build_dir)
        .arg("--cache-dir")
        .arg(&spec.cache_dir);
    if let Some(index) = &spec.index_path {
        run.arg("--index-path").arg(index);
    }
    let output = run.assert().success().get_output().clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert_stdout_contains(spec.label, &stdout, spec.expected_stdout);
    stdout
}

fn assert_stdout_contains(label: &str, stdout: &str, expected: &[&str]) {
    for needle in expected {
        assert!(
            stdout.contains(needle),
            "{label} stdout should contain `{needle}`; stdout = {stdout}"
        );
    }
}
