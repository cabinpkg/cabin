use std::path::PathBuf;

use super::cabin;

pub struct PortBuildRun<'a> {
    pub label: &'a str,
    pub manifest: PathBuf,
    pub build_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub expected_stdout: &'a [&'a str],
}

pub fn run_port_build_then_run(spec: &PortBuildRun<'_>) -> String {
    cabin()
        .args(["build", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(&spec.build_dir)
        .arg("--cache-dir")
        .arg(&spec.cache_dir)
        .assert()
        .success();

    let output = cabin()
        .args(["run", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(&spec.build_dir)
        .arg("--cache-dir")
        .arg(&spec.cache_dir)
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert_stdout_contains(spec.label, &stdout, spec.expected_stdout);
    stdout
}

pub struct PortCacheLifecycle<'a> {
    pub label: &'a str,
    pub manifest: PathBuf,
    pub build_root: PathBuf,
    pub warm_cache: PathBuf,
    pub pristine_cache: PathBuf,
    pub expected_stdout: &'a [&'a str],
    pub expected_downloads: &'a [&'a str],
    pub frozen_port: &'a str,
}

pub fn run_port_cache_lifecycle(spec: &PortCacheLifecycle<'_>) {
    let cold = cabin()
        .args(["run", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(spec.build_root.join("cold"))
        .arg("--cache-dir")
        .arg(&spec.warm_cache)
        .assert()
        .success()
        .get_output()
        .clone();
    let cold_stdout = String::from_utf8(cold.stdout).expect("stdout is utf-8");
    assert_stdout_contains(spec.label, &cold_stdout, spec.expected_stdout);
    for name in spec.expected_downloads {
        assert!(
            cold_stdout.contains(&format!("Downloaded {name}")),
            "{} cold cache should announce `{name}` download; stdout = {cold_stdout}",
            spec.label
        );
    }

    let warm = cabin()
        .args(["build", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(spec.build_root.join("warm"))
        .arg("--cache-dir")
        .arg(&spec.warm_cache)
        .assert()
        .success()
        .get_output()
        .clone();
    let warm_stdout = String::from_utf8(warm.stdout).expect("stdout is utf-8");
    assert!(
        !warm_stdout.contains("Downloaded"),
        "{} warm cache must not re-announce a download; stdout = {warm_stdout}",
        spec.label
    );

    let offline = cabin()
        .args(["run", "--offline", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(spec.build_root.join("offline"))
        .arg("--cache-dir")
        .arg(&spec.warm_cache)
        .assert()
        .success()
        .get_output()
        .clone();
    let offline_stdout = String::from_utf8(offline.stdout).expect("stdout is utf-8");
    assert_stdout_contains(spec.label, &offline_stdout, spec.expected_stdout);

    let frozen = cabin()
        .args(["build", "--frozen", "--manifest-path"])
        .arg(&spec.manifest)
        .arg("--build-dir")
        .arg(spec.build_root.join("frozen"))
        .arg("--cache-dir")
        .arg(&spec.pristine_cache)
        .assert()
        .failure()
        .get_output()
        .clone();
    let frozen_stderr = String::from_utf8_lossy(&frozen.stderr).to_string();
    assert!(
        frozen_stderr.contains(spec.frozen_port)
            && (frozen_stderr.contains("frozen") || frozen_stderr.contains("not cached")),
        "{} frozen cold cache should name `{}`; stderr = {frozen_stderr}",
        spec.label,
        spec.frozen_port
    );
}

fn assert_stdout_contains(label: &str, stdout: &str, expected: &[&str]) {
    for needle in expected {
        assert!(
            stdout.contains(needle),
            "{label} stdout should contain `{needle}`; stdout = {stdout}"
        );
    }
}
