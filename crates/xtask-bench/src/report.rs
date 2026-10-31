//! Results files, summary statistics, the readable report, and the
//! baseline comparison with its regression policy.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Bumped when a results field changes name, unit, or meaning.
pub const RESULTS_FORMAT_VERSION: u32 = 2;

/// One binary's results for one run of the suite.
#[derive(Debug, Serialize, Deserialize)]
pub struct Results {
    pub format_version: u32,
    pub label: String,
    pub cabin_version: String,
    pub build: Build,
    /// The checkout `cargo perf` itself was built from, which defines
    /// the scenarios and fixtures.
    pub harness: Checkout,
    pub fixture_version: u32,
    pub host: Host,
    pub hyperfine: String,
    pub ninja: Option<String>,
    pub scenarios: Vec<ScenarioResult>,
}

/// A git checkout; `None` where git could not tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    pub commit: Option<String>,
    /// Whether tracked or untracked files differ from `commit`.
    pub dirty: Option<bool>,
}

/// How a measured binary was built.  Every field is `None` for a
/// `--cabin` binary: the harness did not build it and does not guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub source: Checkout,
    pub profile: Option<String>,
    /// `rustc -V` in the build's environment.
    pub rustc: Option<String>,
}

impl Build {
    pub const UNKNOWN: Self = Self {
        source: Checkout {
            commit: None,
            dirty: None,
        },
        profile: None,
        rustc: None,
    };

    fn describe(&self) -> String {
        if *self == Self::UNKNOWN {
            return "prebuilt; source, profile, and toolchain unknown".to_owned();
        }
        let unknown = || "unknown".to_owned();
        format!(
            "built from {}, {} profile, {}",
            self.source.describe(),
            self.profile.clone().unwrap_or_else(unknown),
            self.rustc
                .clone()
                .unwrap_or_else(|| "rustc unknown".to_owned())
        )
    }
}

impl Checkout {
    fn describe(&self) -> String {
        let commit = self.commit.as_deref().unwrap_or("an unknown commit");
        match self.dirty {
            Some(true) => format!("{commit} with uncommitted changes"),
            Some(false) => commit.to_owned(),
            None => format!("{commit} (cleanliness unknown)"),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub cpus: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub name: String,
    /// Whether the untimed check run behaved as the scenario expects;
    /// an invalid scenario has no samples.
    pub valid: bool,
    pub detail: String,
    /// Raw per-run values; `instructions` is empty where the host
    /// exposes no hardware counters.
    pub samples: Samples,
    /// The check run's `CABIN_TRACE_DIR` timings summary phases.
    pub phases: serde_json::Value,
    /// The check run's `getrusage` block from the same summary, which
    /// separates Cabin alone from its largest child; `Null` where the
    /// summary has none.
    pub process: serde_json::Value,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Samples {
    pub wall_s: Vec<f64>,
    pub user_s: Vec<f64>,
    pub system_s: Vec<f64>,
    pub peak_rss_bytes: Vec<f64>,
    pub instructions: Vec<f64>,
}

pub fn read(path: &Path) -> Result<Results> {
    #[derive(Deserialize)]
    struct Format {
        format_version: u32,
    }
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let parse = || format!("parsing {}", path.display());
    let format: Format = serde_json::from_slice(&bytes).with_context(parse)?;
    if format.format_version != RESULTS_FORMAT_VERSION {
        bail!(
            "{} has results format {}, this harness reads {RESULTS_FORMAT_VERSION}; measure that binary again with this harness",
            path.display(),
            format.format_version
        );
    }
    serde_json::from_slice(&bytes).with_context(parse)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub median: f64,
    pub q1: f64,
    pub q3: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

impl Stats {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        #[allow(clippy::cast_precision_loss)]
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        Some(Self {
            median: quantile(&sorted, 0.5),
            q1: quantile(&sorted, 0.25),
            q3: quantile(&sorted, 0.75),
            mean,
            min: sorted[0],
            max: sorted[sorted.len() - 1],
        })
    }

    /// Interquartile range relative to the median: the run-to-run
    /// spread the comparison treats as noise.
    pub fn relative_spread(&self) -> f64 {
        if self.median == 0.0 {
            0.0
        } else {
            (self.q3 - self.q1) / self.median
        }
    }
}

/// Linear interpolation between the closest ranks.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let rank = q * (sorted.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64)
}

/// The metrics the comparison judges, each with the smallest change
/// the policy counts as a regression or an improvement: relative, and
/// absolute (seconds, bytes, instructions).  The floors sit past what
/// the median moved in a quieter run of one binary against itself
/// (`docs/design/performance-observability.md`): the wall time of
/// scenarios under 30 ms, peak RSS, and instruction counts that do not
/// depend on load.
const METRICS: [(&str, f64, f64); 3] = [
    ("wall", 0.05, FLOOR_WALL_S),
    ("peak_rss", 0.05, FLOOR_RSS_BYTES),
    ("instructions", 0.02, FLOOR_INSTRUCTIONS),
];
const FLOOR_WALL_S: f64 = 0.002;
const FLOOR_RSS_BYTES: f64 = 1024.0 * 1024.0;
const FLOOR_INSTRUCTIONS: f64 = 1e6;

/// The two-sided 1% critical value of the standard normal: a rank-sum
/// statistic past it means the two sides' samples are unlikely to come
/// from one distribution.
const Z_CRITICAL: f64 = 2.576;

/// The Mann-Whitney rank-sum statistic of `candidate` against
/// `baseline`, by the normal approximation with tie and continuity
/// corrections: how many standard deviations the candidate's ranks sit
/// from where they would if both sides came from one distribution.
#[allow(clippy::cast_precision_loss)]
fn rank_sum_z(baseline: &[f64], candidate: &[f64]) -> f64 {
    let mut all: Vec<(f64, bool)> = baseline.iter().map(|&v| (v, false)).collect();
    all.extend(candidate.iter().map(|&v| (v, true)));
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    let (n1, n2) = (baseline.len() as f64, candidate.len() as f64);
    let total = n1 + n2;
    let (mut rank_sum, mut ties, mut start) = (0.0, 0.0, 0);
    while start < all.len() {
        let value = all[start].0;
        let end = start
            + all[start..]
                .iter()
                .take_while(|x| x.0.total_cmp(&value).is_eq())
                .count();
        let mean_rank = (start + end + 1) as f64 / 2.0;
        rank_sum += mean_rank * all[start..end].iter().filter(|x| x.1).count() as f64;
        let run = (end - start) as f64;
        ties += run * run * run - run;
        start = end;
    }
    let u = rank_sum - n2 * (n2 + 1.0) / 2.0;
    let sigma = (n1 * n2 / 12.0 * (total + 1.0 - ties / (total * (total - 1.0)))).sqrt();
    if sigma.is_nan() || sigma == 0.0 {
        return 0.0;
    }
    ((u - n1 * n2 / 2.0).abs() - 0.5).max(0.0) / sigma
}

fn metric<'a>(samples: &'a Samples, name: &str) -> &'a [f64] {
    match name {
        "wall" => &samples.wall_s,
        "peak_rss" => &samples.peak_rss_bytes,
        _ => &samples.instructions,
    }
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub scenario: String,
    pub metric: String,
    pub baseline_median: Option<f64>,
    pub candidate_median: Option<f64>,
    /// `candidate / baseline - 1`.
    pub change: Option<f64>,
    /// The larger interquartile range of the two sides, relative to the
    /// baseline median like `change`.
    pub noise: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// A real decrease past the policy's minimum effect and floor.
    Improved,
    /// No difference beyond run-to-run noise.
    Unchanged,
    /// A real increase past the policy's minimum effect and floor.
    Regressed,
    /// A real increase the policy does not count: smaller than the
    /// minimum effect or the absolute floor.
    Increased,
    /// A real decrease the policy does not count.
    Decreased,
    /// Only one side recorded the metric (its platform or revision
    /// does not measure it), so there is nothing to compare.
    Incomparable,
    /// The candidate failed its check or lacks the scenario.
    Invalid,
    /// Only the baseline failed its check or lacks the scenario, as
    /// an older Cabin does when a scenario uses a newer flag: there is
    /// nothing to compare against.
    NoBaseline,
}

/// Classify every scenario both sides measured.  A difference is real
/// when the samples' ranks separate (`rank_sum_z`) and the medians move
/// further than the noise, and the policy counts a real one only past
/// the metric's minimum effect and floor.  Sides measured in separate
/// runs did not share the machine's load, which can still make a false
/// difference.
pub fn compare(baseline: &Results, candidate: &Results) -> Vec<Comparison> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let names = candidate.scenarios.iter().chain(&baseline.scenarios);
    for scenario in names.map(|s| &s.name).filter(|&n| seen.insert(n)) {
        let base = baseline.scenarios.iter().find(|s| &s.name == scenario);
        let cand = candidate.scenarios.iter().find(|s| &s.name == scenario);
        for (name, min_effect, floor) in METRICS {
            let sides = base
                .zip(cand)
                .map(|(b, c)| (metric(&b.samples, name), metric(&c.samples, name)));
            let valid = base.is_some_and(|b| b.valid) && cand.is_some_and(|c| c.valid);
            let Some((bs, cs)) = sides.filter(|_| valid) else {
                let verdict = if cand.is_some_and(|c| c.valid) && !base.is_some_and(|b| b.valid) {
                    Verdict::NoBaseline
                } else {
                    Verdict::Invalid
                };
                if name == "wall" {
                    out.push(Comparison {
                        scenario: scenario.clone(),
                        metric: name.to_owned(),
                        baseline_median: None,
                        candidate_median: None,
                        change: None,
                        noise: None,
                        verdict,
                    });
                }
                continue;
            };
            let (b, c) = match (Stats::of(bs), Stats::of(cs)) {
                (Some(b), Some(c)) => (b, c),
                (None, None) => continue,
                (b, c) => {
                    out.push(Comparison {
                        scenario: scenario.clone(),
                        metric: name.to_owned(),
                        baseline_median: b.map(|b| b.median),
                        candidate_median: c.map(|c| c.median),
                        change: None,
                        noise: None,
                        verdict: Verdict::Incomparable,
                    });
                    continue;
                }
            };
            let shift = c.median - b.median;
            let change = shift / b.median;
            // The spread in the metric's own units, so which side is the
            // baseline cannot decide whether a difference is real.
            let noise = (b.q3 - b.q1).max(c.q3 - c.q1) / b.median;
            let verdict = if rank_sum_z(bs, cs) <= Z_CRITICAL || change.abs() <= noise {
                Verdict::Unchanged
            } else if change.abs() < min_effect || shift.abs() < floor {
                if shift > 0.0 {
                    Verdict::Increased
                } else {
                    Verdict::Decreased
                }
            } else if shift > 0.0 {
                Verdict::Regressed
            } else {
                Verdict::Improved
            };
            out.push(Comparison {
                scenario: scenario.clone(),
                metric: name.to_owned(),
                baseline_median: Some(b.median),
                candidate_median: Some(c.median),
                change: Some(change),
                noise: Some(noise),
                verdict,
            });
        }
    }
    out
}

pub fn write_compare(
    out: &Path,
    baseline: &Results,
    candidate: &Results,
    rows: &[Comparison],
) -> Result<()> {
    let side = |r: &Results| {
        serde_json::json!({
            "label": r.label,
            "cabin_version": r.cabin_version,
            "build": r.build,
            "harness": r.harness,
        })
    };
    let json = serde_json::json!({
        "format_version": RESULTS_FORMAT_VERSION,
        "baseline": side(baseline),
        "candidate": side(candidate),
        "comparisons": rows,
    });
    std::fs::write(out.join("compare.json"), serde_json::to_vec_pretty(&json)?)?;
    let mut md = String::from("# Cabin benchmark comparison\n\n");
    for (side, results) in [("Baseline", baseline), ("Candidate", candidate)] {
        writeln!(
            md,
            "- {side} `{}`: {}, {}; measured by the harness at {}.",
            results.label,
            results.cabin_version,
            results.build.describe(),
            results.harness.describe()
        )?;
    }
    writeln!(md, "\n{}\n", scopes(&candidate.host.os))?;
    if baseline.host != candidate.host || baseline.fixture_version != candidate.fixture_version {
        md.push_str(
            "**Warning:** the two sides ran on different hosts or fixtures; the numbers are not comparable.\n\n",
        );
    }
    for verdict in [
        Verdict::Regressed,
        Verdict::Improved,
        Verdict::Increased,
        Verdict::Decreased,
        Verdict::Incomparable,
        Verdict::Invalid,
        Verdict::NoBaseline,
    ] {
        let count = rows.iter().filter(|r| r.verdict == verdict).count();
        writeln!(md, "- {verdict:?}: {count}")?;
    }
    md.push_str("\n| Scenario | Metric | Baseline | Candidate | Change | Noise | Verdict |\n");
    md.push_str("| --- | --- | ---: | ---: | ---: | ---: | --- |\n");
    for row in rows {
        let fmt = |v: Option<f64>| v.map_or("-".to_owned(), |v| format_metric(&row.metric, v));
        let pct = |v: Option<f64>| v.map_or("-".to_owned(), |v| format!("{:+.1}%", v * 100.0));
        writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {:?} |",
            row.scenario,
            row.metric,
            fmt(row.baseline_median),
            fmt(row.candidate_median),
            pct(row.change),
            row.noise
                .map_or("-".to_owned(), |v| format!("{:.1}%", v * 100.0)),
            row.verdict
        )?;
    }
    std::fs::write(out.join("compare.md"), md)?;
    Ok(())
}

pub fn write_report(out: &Path, all: &[Results]) -> Result<()> {
    let mut md = String::from("# Cabin benchmark report\n\n");
    if let Some(first) = all.first() {
        writeln!(
            md,
            "Host: {} {} ({} CPUs); {}; ninja {}; fixtures v{}; harness {}.\n",
            first.host.os,
            first.host.arch,
            first.host.cpus,
            first.hyperfine,
            first.ninja.as_deref().unwrap_or("not found"),
            first.fixture_version,
            first.harness.describe()
        )?;
        writeln!(md, "{}\n", scopes(&first.host.os))?;
    }
    for results in all {
        writeln!(
            md,
            "## `{}`: {}\n\n{}.\n",
            results.label,
            results.cabin_version,
            results.build.describe()
        )?;
        md.push_str("| Scenario | Check | Median | IQR | User | System | Peak RSS | Instructions | Check run: Cabin alone | Check run: largest child | Slowest phases |\n");
        md.push_str(
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n",
        );
        for s in &results.scenarios {
            let med = |values: &[f64], metric: &str| {
                Stats::of(values).map_or("-".to_owned(), |st| format_metric(metric, st.median))
            };
            let iqr = Stats::of(&s.samples.wall_s).map_or("-".to_owned(), |st| {
                format!("{:.1}%", st.relative_spread() * 100.0)
            });
            let usage = |key: &str| s.process[key].as_f64();
            let cabin = usage("self_peak_rss_bytes")
                .zip(usage("self_user_us").zip(usage("self_system_us")))
                .map_or("-".to_owned(), |(rss, (user, system))| {
                    format!(
                        "{}, {} CPU",
                        format_metric("peak_rss", rss),
                        format_metric("wall", (user + system) / 1e6)
                    )
                });
            let child = usage("children_largest_peak_rss_bytes")
                .filter(|&rss| rss > 0.0)
                .map_or("-".to_owned(), |rss| format_metric("peak_rss", rss));
            writeln!(
                md,
                "| {} | {} | {} | {iqr} | {} | {} | {} | {} | {cabin} | {child} | {} |",
                s.name,
                if s.valid {
                    "ok".to_owned()
                } else {
                    format!("FAILED: {}", one_line(&s.detail))
                },
                med(&s.samples.wall_s, "wall"),
                med(&s.samples.user_s, "wall"),
                med(&s.samples.system_s, "wall"),
                med(&s.samples.peak_rss_bytes, "peak_rss"),
                med(&s.samples.instructions, "instructions"),
                slowest_phases(&s.phases),
            )?;
        }
        md.push('\n');
    }
    std::fs::write(out.join("report.md"), md)?;
    Ok(())
}

/// What hyperfine 2's per-run metrics cover on `os`, which differs by
/// platform.  The check-run columns come from Cabin's own `getrusage`.
fn scopes(os: &str) -> &'static str {
    match os {
        "linux" => {
            "Scopes: User and System sum Cabin and every process it waited for; Peak RSS is the largest single process, never a sum and never below hyperfine's own peak; Instructions sum Cabin and its children, user mode only; a check run's largest child is never below Cabin's own peak when it started."
        }
        "macos" => {
            "Scopes: User and System sum Cabin and every process it waited for; Peak RSS is the largest single process, never a sum; Instructions are Cabin's own, user and kernel mode."
        }
        "windows" => {
            "Scopes: User and System sum every process in Cabin's job; Peak RSS and Instructions are not measured."
        }
        _ => {
            "Scopes: User and System sum Cabin and every process it waited for; Peak RSS is the largest single process, never a sum; Instructions are not measured."
        }
    }
}

/// A failure detail fit for a table cell: no escape sequences, no line
/// breaks, no column separators; the results JSON keeps it whole.
fn one_line(detail: &str) -> String {
    let mut out = String::new();
    let mut chars = detail.chars();
    while let Some(c) = chars.next() {
        match c {
            // CSI sequence: skip through its final byte.
            '\u{1b}' => {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            '\n' | '\r' => out.push(' '),
            '|' => out.push('/'),
            c => out.push(c),
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    match out.char_indices().nth(240) {
        Some((cut, _)) => format!("{}...", &out[..cut]),
        None => out,
    }
}

/// The three costliest spans below the root, from the check run.
fn slowest_phases(phases: &serde_json::Value) -> String {
    let mut list: Vec<(&str, u64)> = phases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"] != "cabin")
        .filter_map(|p| Some((p["name"].as_str()?, p["total_us"].as_u64()?)))
        .collect();
    list.sort_by_key(|&(_, us)| std::cmp::Reverse(us));
    list.iter()
        .take(3)
        .map(|&(name, us)| {
            let ms = std::time::Duration::from_micros(us).as_secs_f64() * 1000.0;
            format!("{name} {ms:.1}ms")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_metric(metric: &str, value: f64) -> String {
    match metric {
        "wall" if value < 1.0 => format!("{:.1} ms", value * 1000.0),
        "wall" => format!("{value:.3} s"),
        "peak_rss" => format!("{:.1} MiB", value / 1_048_576.0),
        _ => format!("{:.1} M", value / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn results(label: &str, scenarios: Vec<(&str, Vec<f64>, bool)>) -> Results {
        Results {
            format_version: RESULTS_FORMAT_VERSION,
            label: label.to_owned(),
            cabin_version: "cabin 0.0.0".to_owned(),
            build: Build::UNKNOWN,
            harness: Checkout {
                commit: Some("0123abc".to_owned()),
                dirty: Some(false),
            },
            fixture_version: 1,
            host: Host {
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                cpus: 4,
            },
            hyperfine: "hyperfine 2.0.0".to_owned(),
            ninja: None,
            scenarios: scenarios
                .into_iter()
                .map(|(name, wall_s, valid)| ScenarioResult {
                    name: name.to_owned(),
                    valid,
                    detail: String::new(),
                    samples: Samples {
                        wall_s,
                        ..Samples::default()
                    },
                    phases: serde_json::Value::Null,
                    process: serde_json::Value::Null,
                })
                .collect(),
        }
    }

    fn verdict(base: Vec<f64>, cand: Vec<f64>) -> Verdict {
        let rows = compare(
            &results("base", vec![("s", base, true)]),
            &results("cand", vec![("s", cand, true)]),
        );
        rows[0].verdict
    }

    #[test]
    fn quartiles_interpolate_between_ranks() {
        let stats = Stats::of(&[4.0, 1.0, 3.0, 2.0]).unwrap();
        assert!((stats.median - 2.5).abs() < 1e-12);
        assert!((stats.q1 - 1.75).abs() < 1e-12);
        assert!((stats.q3 - 3.25).abs() < 1e-12);
    }

    #[test]
    fn the_rank_sum_statistic_corrects_for_ties_and_continuity() {
        let z = rank_sum_z(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
        assert!((z - 4.0 / 5.25_f64.sqrt()).abs() < 1e-12, "{z}");
        let z = rank_sum_z(&[1.0, 1.0, 2.0], &[2.0, 3.0, 3.0]);
        assert!((z - 3.5 / 4.8_f64.sqrt()).abs() < 1e-12, "{z}");
        assert!(rank_sum_z(&[1.0; 5], &[1.0; 5]).abs() < 1e-12);
    }

    #[test]
    fn a_real_change_is_judged_by_the_policy() {
        let base = || {
            (0..10)
                .map(|i| 1.0 + f64::from(i) * 1e-3)
                .collect::<Vec<_>>()
        };
        let shifted = |by: f64| base().into_iter().map(|v| v * by).collect::<Vec<_>>();
        assert_eq!(verdict(base(), shifted(1.2)), Verdict::Regressed);
        assert_eq!(verdict(base(), shifted(0.8)), Verdict::Improved);
        // Below the 5% minimum effect, still reported.
        assert_eq!(verdict(base(), shifted(1.03)), Verdict::Increased);
        assert_eq!(verdict(base(), shifted(0.97)), Verdict::Decreased);
        // 15%, but 1.5 ms is under the wall-time floor.
        let fast = || base().into_iter().map(|v| v * 0.01).collect::<Vec<_>>();
        let slower = fast().into_iter().map(|v| v * 1.15).collect();
        assert_eq!(verdict(fast(), slower), Verdict::Increased);
    }

    #[test]
    fn a_shift_within_the_noise_is_unchanged() {
        // A 20% median shift between samples that interleave.
        let base = vec![0.8, 0.9, 1.0, 1.2, 1.3, 0.85, 1.25, 0.95, 1.0, 1.1];
        let cand = vec![1.2, 0.8, 1.3, 1.25, 0.9, 1.2, 1.35, 0.95, 1.15, 1.3];
        assert!(rank_sum_z(&base, &cand) <= Z_CRITICAL);
        assert_eq!(verdict(base, cand), Verdict::Unchanged);
        // Ranks that separate, by less than the interquartile spread.
        let base: Vec<f64> = (0..40).map(|i| 0.6 + f64::from(i % 10) * 0.1).collect();
        let cand: Vec<f64> = base.iter().map(|v| v + 0.25).collect();
        assert!(rank_sum_z(&base, &cand) > Z_CRITICAL);
        assert_eq!(verdict(base, cand), Verdict::Unchanged);
        // Separated, but within the wider side's spread whichever side
        // is the baseline: +102% from the narrow side, -50% from the wide.
        let wide = vec![0.6, 0.62, 0.7, 0.75, 0.9, 1.1, 1.25, 1.3, 1.35, 1.4];
        let narrow: Vec<f64> = (0..10).map(|i| 0.45 + f64::from(i) * 0.01).collect();
        assert_eq!(verdict(narrow.clone(), wide.clone()), Verdict::Unchanged);
        assert_eq!(verdict(wide, narrow), Verdict::Unchanged);
    }

    #[test]
    fn untied_samples_need_six_runs_a_side() {
        let base = |n| {
            (0..n)
                .map(|i| 1.0 + f64::from(i) * 1e-3)
                .collect::<Vec<_>>()
        };
        let shifted = |n| base(n).into_iter().map(|v| v * 1.2).collect::<Vec<_>>();
        // Fully separated, yet five runs a side cannot reach 1%.
        assert_eq!(verdict(base(5), shifted(5)), Verdict::Unchanged);
        assert_eq!(verdict(base(6), shifted(6)), Verdict::Regressed);
        // Ties narrow the variance, so five identical runs a side can.
        assert_eq!(verdict(vec![0.05; 5], vec![0.06; 5]), Verdict::Regressed);
    }

    #[test]
    fn a_metric_only_one_side_records_is_incomparable() {
        let mut base = results("base", vec![("s", vec![1.0; 10], true)]);
        base.scenarios[0].samples.instructions = vec![5e7; 10];
        let rows = compare(&base, &results("cand", vec![("s", vec![1.0; 10], true)]));
        let row = rows.iter().find(|r| r.metric == "instructions").unwrap();
        assert_eq!(row.verdict, Verdict::Incomparable);
        assert_eq!(row.baseline_median, Some(5e7));
        assert!(rows.iter().all(|r| r.metric != "peak_rss"), "{rows:?}");
    }

    #[test]
    fn only_the_candidate_failing_or_lacking_a_scenario_is_invalid() {
        let rows = compare(
            &results(
                "base",
                vec![
                    ("old-flag", vec![1.0], false),
                    ("gone", vec![1.0], true),
                    ("broken", vec![1.0], true),
                ],
            ),
            &results(
                "cand",
                vec![
                    ("old-flag", vec![1.0], true),
                    ("new", vec![1.0], true),
                    ("broken", vec![1.0], false),
                ],
            ),
        );
        assert_eq!(rows.len(), 4, "one wall row per scenario: {rows:?}");
        let verdicts: BTreeMap<_, _> = rows
            .iter()
            .map(|r| (r.scenario.as_str(), r.verdict))
            .collect();
        assert_eq!(
            verdicts,
            BTreeMap::from([
                ("broken", Verdict::Invalid),
                ("gone", Verdict::Invalid),
                ("new", Verdict::NoBaseline),
                ("old-flag", Verdict::NoBaseline),
            ])
        );
    }
}
