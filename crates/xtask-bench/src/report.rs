//! Results files, summary statistics, the readable report, and the
//! baseline comparison with its regression policy.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Bumped when a results field changes name, unit, or meaning.
pub const RESULTS_FORMAT_VERSION: u32 = 1;

/// One binary's results for one run of the suite.
#[derive(Debug, Serialize, Deserialize)]
pub struct Results {
    pub format_version: u32,
    pub label: String,
    pub cabin_version: String,
    pub fixture_version: u32,
    pub host: Host,
    pub hyperfine: String,
    pub ninja: Option<String>,
    pub scenarios: Vec<ScenarioResult>,
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
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let results: Results =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if results.format_version != RESULTS_FORMAT_VERSION {
        bail!(
            "{} has results format {}, this harness reads {RESULTS_FORMAT_VERSION}",
            path.display(),
            results.format_version
        );
    }
    Ok(results)
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
/// it reports.  Wall time and peak memory move a few percent between
/// identical runs on a quiet machine; retired instructions barely move.
const METRICS: [(&str, f64); 3] = [("wall", 0.05), ("peak_rss", 0.05), ("instructions", 0.02)];

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
    /// The larger relative interquartile range of the two sides.
    pub noise: Option<f64>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Improved,
    Unchanged,
    Regressed,
    /// The candidate failed its check or lacks the scenario.
    Invalid,
    /// Only the baseline failed its check or lacks the scenario, as
    /// an older Cabin does when a scenario uses a newer flag: there is
    /// nothing to compare against.
    NoBaseline,
}

/// Classify every scenario both sides measured.  A change counts only
/// when it exceeds both the metric's minimum effect and the observed
/// noise; a noisier machine therefore reports fewer, never false,
/// regressions.
pub fn compare(baseline: &Results, candidate: &Results) -> Vec<Comparison> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let names = candidate.scenarios.iter().chain(&baseline.scenarios);
    for scenario in names.map(|s| &s.name).filter(|&n| seen.insert(n)) {
        let base = baseline.scenarios.iter().find(|s| &s.name == scenario);
        let cand = candidate.scenarios.iter().find(|s| &s.name == scenario);
        for (name, min_effect) in METRICS {
            let sides = base
                .zip(cand)
                .map(|(b, c)| (metric(&b.samples, name), metric(&c.samples, name)));
            let stats = sides.and_then(|(b, c)| Some((Stats::of(b)?, Stats::of(c)?)));
            let valid = base.is_some_and(|b| b.valid) && cand.is_some_and(|c| c.valid);
            let Some((b, c)) = stats.filter(|_| valid) else {
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
            let change = c.median / b.median - 1.0;
            let noise = b.relative_spread().max(c.relative_spread());
            let threshold = min_effect.max(noise);
            let verdict = if change > threshold {
                Verdict::Regressed
            } else if change < -threshold {
                Verdict::Improved
            } else {
                Verdict::Unchanged
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
    std::fs::write(out.join("compare.json"), serde_json::to_vec_pretty(rows)?)?;
    let mut md = format!(
        "# Cabin benchmark comparison\n\nBaseline `{}` ({}) vs candidate `{}` ({}).\n\n",
        baseline.label, baseline.cabin_version, candidate.label, candidate.cabin_version
    );
    if baseline.host != candidate.host || baseline.fixture_version != candidate.fixture_version {
        md.push_str(
            "**Warning:** the two sides ran on different hosts or fixtures; the numbers are not comparable.\n\n",
        );
    }
    for verdict in [
        Verdict::Regressed,
        Verdict::Improved,
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
            "Host: {} {} ({} CPUs); {}; ninja {}; fixtures v{}.\n",
            first.host.os,
            first.host.arch,
            first.host.cpus,
            first.hyperfine,
            first.ninja.as_deref().unwrap_or("not found"),
            first.fixture_version
        )?;
    }
    for results in all {
        writeln!(md, "## `{}`: {}\n", results.label, results.cabin_version)?;
        md.push_str("| Scenario | Check | Median | IQR | User | System | Peak RSS | Instructions | Slowest phases |\n");
        md.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
        for s in &results.scenarios {
            let med = |values: &[f64], metric: &str| {
                Stats::of(values).map_or("-".to_owned(), |st| format_metric(metric, st.median))
            };
            let iqr = Stats::of(&s.samples.wall_s).map_or("-".to_owned(), |st| {
                format!("{:.1}%", st.relative_spread() * 100.0)
            });
            writeln!(
                md,
                "| {} | {} | {} | {iqr} | {} | {} | {} | {} | {} |",
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
    fn a_change_beyond_threshold_and_noise_is_classified() {
        let quiet = vec![1.0, 1.0, 1.0, 1.0];
        assert_eq!(verdict(quiet.clone(), vec![1.2; 4]), Verdict::Regressed);
        assert_eq!(verdict(quiet.clone(), vec![0.8; 4]), Verdict::Improved);
        assert_eq!(verdict(quiet, vec![1.03; 4]), Verdict::Unchanged);
    }

    #[test]
    fn noise_wider_than_the_change_is_not_a_regression() {
        // 40% interquartile spread swallows a 20% median shift.
        let noisy = vec![0.8, 0.9, 1.0, 1.2, 1.3];
        assert_eq!(verdict(noisy, vec![1.2; 5]), Verdict::Unchanged);
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
