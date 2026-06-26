use super::{Context, InitArgs, NewArgs, Path, Reporter, Result, bail, scaffold};

pub(super) fn scaffold_kind_from_flags(_bin: bool, lib: bool) -> scaffold::ScaffoldKind {
    // clap's `group` constraint already rejected the `--bin
    // --lib` combination, so `_bin` is only observed for
    // symmetry with the CLI surface; binary is the default
    // whether `--bin` was explicit or absent.
    if lib {
        scaffold::ScaffoldKind::Library
    } else {
        scaffold::ScaffoldKind::Binary
    }
}

pub(super) fn report_scaffold(
    reporter: Reporter,
    verb: &str,
    report: &scaffold::ScaffoldReport,
    dest: &Path,
) {
    // Cargo-style aligned status line: the verb (`Created` /
    // `Initialized`) is right-padded to column 12 by
    // `Reporter::status`, which keeps the banner aligned with
    // `Compiling` and `Finished` and styles the verb in bright
    // green + bold when color is enabled.  The rendered shape
    // is:
    //
    // Created binary (application) `<name>` package
    // Created library `<name>` package
    reporter.status(
        verb,
        format_args!(
            "{kind} `{name}` package",
            kind = report.kind.label(),
            name = report.name.as_str(),
        ),
    );
    for created in &report.files_created {
        let relative = created.strip_prefix(dest).unwrap_or(created);
        reporter.verbose(format_args!(
            "cabin: wrote {}",
            relative.display().to_string().replace('\\', "/")
        ));
    }
}

pub(super) fn init(args: &InitArgs, reporter: Reporter) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    let kind = scaffold_kind_from_flags(args.bin, args.lib);
    let request = scaffold::ScaffoldRequest::new(&cwd)
        .with_name(args.name.as_deref())
        .with_kind(kind)
        .with_gitignore(true);
    let report = scaffold::scaffold(request)?;
    // `cabin init` and `cabin new` share the same `Created …`
    // status line so scripts can parse either path uniformly.
    report_scaffold(reporter, "Created", &report, &cwd);
    Ok(())
}

pub(super) fn new(args: &NewArgs, reporter: Reporter) -> Result<()> {
    let target = args.path.clone();
    if target.as_os_str().is_empty() {
        bail!("destination path must not be empty");
    }
    if target.exists() {
        bail!(
            "destination {} already exists; use `cabin init` to initialize an existing directory",
            target.display()
        );
    }
    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        bail!(
            "parent directory {} does not exist; create it first or pass a path under an existing directory",
            parent.display()
        );
    }

    std::fs::create_dir(&target)
        .with_context(|| format!("failed to create directory {}", target.display()))?;

    let kind = scaffold_kind_from_flags(args.bin, args.lib);
    let request = scaffold::ScaffoldRequest::new(&target)
        .with_name(args.name.as_deref())
        .with_kind(kind)
        .with_gitignore(true);
    match scaffold::scaffold(request) {
        Ok(report) => {
            report_scaffold(reporter, "Created", &report, &target);
            Ok(())
        }
        Err(err) => {
            // Best-effort cleanup of the directory we created
            // created; surface the scaffold error regardless of
            // whether removal succeeds.
            let _ = std::fs::remove_dir_all(&target);
            Err(err.into())
        }
    }
}
