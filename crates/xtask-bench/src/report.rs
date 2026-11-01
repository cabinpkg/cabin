//! Results files, summary statistics, the readable report, and the
//! baseline comparison with its regression policy.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Bumped when a results field changes name, unit, or meaning.
pub const RESULTS_FORMAT_VERSION: u32 = 3;

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
    /// Whether the scenario was prepared, its untimed check run behaved
    /// as the scenario expects, and every timed block succeeded; an
    /// invalid scenario has no samples.
    pub valid: bool,
    pub detail: String,
    /// Raw per-run values; `instructions` is empty where the host
    /// exposes no hardware counters.
    pub samples: Samples,
    /// How many runs each timed block held, in the order the blocks
    /// ran; every binary of one `cargo perf run` timed its block `i` in
    /// round `i`.  Empty for an invalid scenario.
    pub blocks: Vec<u32>,
    /// The check run's `CABIN_TRACE_DIR` timings summary phases.
    pub phases: serde_json::Value,
    /// The check run's `getrusage` block from the same summary, which
    /// separates Cabin alone from its largest child; `Null` when the
    /// summary is missing, older than format 2, or has none.
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

    /// Interquartile range relative to the median.
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

/// The two-sided significance level a difference must reach to be
/// real.
const ALPHA: f64 = 0.01;

/// Whether the blocks of `candidate` differ from those of `baseline`:
/// the test's two-sided p-value, and the smallest p-value blocks of
/// these layouts could reach at all.  Runs within a block share that
/// moment's load, so the test takes one value per block, its median.
/// Blocks of one layout ran round by round and pair up, which cancels
/// the load a round shared: an exact Wilcoxon signed-rank test of the
/// rounds' differences.  Blocks of differing layouts come from separate
/// runs and are compared unpaired: an exact Mann-Whitney test.
fn block_test(
    baseline: &[f64],
    baseline_blocks: &[u32],
    candidate: &[f64],
    candidate_blocks: &[u32],
) -> (f64, f64) {
    let (Some(b), Some(c)) = (
        block_medians(baseline, baseline_blocks),
        block_medians(candidate, candidate_blocks),
    ) else {
        return (1.0, 1.0);
    };
    if baseline_blocks == candidate_blocks {
        let rounds: Vec<f64> = c.iter().zip(&b).map(|(c, b)| c - b).collect();
        let same_way = vec![1.0; rounds.len()];
        (signed_rank_p(&rounds), signed_rank_p(&same_way))
    } else {
        #[allow(clippy::cast_precision_loss)]
        let ordered: Vec<f64> = (0..b.len() + c.len()).map(|i| i as f64).collect();
        let (lower, upper) = ordered.split_at(b.len());
        (rank_sum_p(&b, &c), rank_sum_p(lower, upper))
    }
}

/// The interquartile range of the rounds' differences when the blocks
/// pair up: the noise left once pairing cancels the load each round
/// shared, which the spread of either side's runs still holds.
fn round_spread(
    baseline: &[f64],
    baseline_blocks: &[u32],
    candidate: &[f64],
    candidate_blocks: &[u32],
) -> Option<f64> {
    if baseline_blocks != candidate_blocks {
        return None;
    }
    let b = block_medians(baseline, baseline_blocks)?;
    let c = block_medians(candidate, candidate_blocks)?;
    let rounds: Vec<f64> = c.iter().zip(&b).map(|(c, b)| c - b).collect();
    Stats::of(&rounds).map(|stats| stats.q3 - stats.q1)
}

/// The median of each block of `values`, which `blocks` splits in
/// order; `None` unless the blocks hold exactly `values`.
fn block_medians(values: &[f64], blocks: &[u32]) -> Option<Vec<f64>> {
    let mut rest = values;
    let medians = blocks
        .iter()
        .map(|&runs| {
            let (block, tail) = rest.split_at_checked(usize::try_from(runs).ok()?)?;
            rest = tail;
            Stats::of(block).map(|stats| stats.median)
        })
        .collect::<Option<Vec<_>>>()?;
    rest.is_empty().then_some(medians)
}

/// The ranks of `values`, ties sharing their mean rank, doubled so that
/// every rank is whole: `n` values' doubled ranks sum to `n * (n + 1)`.
fn doubled_ranks(values: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&x, &y| values[x].total_cmp(&values[y]));
    let mut ranks = vec![0; values.len()];
    let mut start = 0;
    while start < order.len() {
        let value = values[order[start]];
        let tied = order[start..]
            .iter()
            .take_while(|&&i| values[i].total_cmp(&value).is_eq())
            .count();
        for &i in &order[start..start + tied] {
            ranks[i] = 2 * start + tied + 1;
        }
        start += tied;
    }
    ranks
}

/// The share of equally likely outcomes, counted by their statistic in
/// `ways`, whose statistic lies at least as far from `mean` as
/// `observed`: a two-sided exact p-value.
fn two_sided(ways: &[f64], observed: usize, mean: usize) -> f64 {
    let distance = observed.abs_diff(mean);
    let extreme: f64 = ways
        .iter()
        .enumerate()
        .filter(|&(statistic, _)| statistic.abs_diff(mean) >= distance)
        .map(|(_, ways)| ways)
        .sum();
    extreme / ways.iter().sum::<f64>()
}

/// The exact two-sided p-value of the Wilcoxon signed-rank test that
/// `differences` center on zero: were each sign a fair coin, how likely
/// a positive rank sum at least this far from its mean would be.  Zeros
/// drop out.
fn signed_rank_p(differences: &[f64]) -> f64 {
    let nonzero: Vec<f64> = differences.iter().copied().filter(|&d| d != 0.0).collect();
    let magnitudes: Vec<f64> = nonzero.iter().map(|d| d.abs()).collect();
    let ranks = doubled_ranks(&magnitudes);
    let total: usize = ranks.iter().sum();
    let positive: usize = ranks
        .iter()
        .zip(&nonzero)
        .filter(|&(_, &d)| d > 0.0)
        .map(|(rank, _)| rank)
        .sum();
    // `ways[s]`: the sign patterns whose positive ranks sum to `s`.
    let mut ways = vec![0.0; total + 1];
    ways[0] = 1.0;
    for &rank in &ranks {
        for sum in (rank..=total).rev() {
            ways[sum] += ways[sum - rank];
        }
    }
    two_sided(&ways, positive, total / 2)
}

/// The exact two-sided p-value of the Mann-Whitney test that `baseline`
/// and `candidate` come from one distribution: were every split of the
/// ranks between the sides equally likely, how likely a candidate rank
/// sum at least this far from its mean would be.
fn rank_sum_p(baseline: &[f64], candidate: &[f64]) -> f64 {
    let all: Vec<f64> = baseline.iter().chain(candidate).copied().collect();
    let ranks = doubled_ranks(&all);
    let total: usize = ranks.iter().sum();
    let picks = candidate.len();
    let observed: usize = ranks[baseline.len()..].iter().sum();
    // `ways[k][s]`: the sets of `k` ranks that sum to `s`.
    let mut ways = vec![vec![0.0; total + 1]; picks + 1];
    ways[0][0] = 1.0;
    for &rank in &ranks {
        for k in (1..=picks).rev() {
            for sum in (rank..=total).rev() {
                ways[k][sum] += ways[k - 1][sum - rank];
            }
        }
    }
    two_sided(&ways[picks], observed, picks * (all.len() + 1))
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
    /// No difference shown, from too few blocks to show one: even
    /// blocks that all moved the same way would not pass the test.
    Inconclusive,
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

/// Classify every scenario in either results file.  A difference is real
/// when the blocks differ (`block_test`) and the medians move further
/// than the noise, and the policy counts a real one only past
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
            let sides = base.zip(cand).map(|(b, c)| {
                let (bs, cs) = (metric(&b.samples, name), metric(&c.samples, name));
                (bs, &b.blocks, cs, &c.blocks)
            });
            let valid = base.is_some_and(|b| b.valid) && cand.is_some_and(|c| c.valid);
            let Some((bs, b_blocks, cs, c_blocks)) = sides.filter(|_| valid) else {
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
            let spread =
                round_spread(bs, b_blocks, cs, c_blocks).unwrap_or((b.q3 - b.q1).max(c.q3 - c.q1));
            let noise = spread / b.median;
            let (p, reachable) = block_test(bs, b_blocks, cs, c_blocks);
            let real = p <= ALPHA;
            let verdict = if !real && reachable > ALPHA {
                Verdict::Inconclusive
            } else if !real || change.abs() <= noise {
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

/// The rows that fail a `--fail-on-regression` gate: how many regressed
/// or are invalid, and how many are inconclusive, whose runs were too
/// few to tell either way and so cannot pass a gate.
pub fn gate_failures(rows: &[Comparison]) -> (usize, usize) {
    let count = |verdicts: &[Verdict]| {
        rows.iter()
            .filter(|r| verdicts.contains(&r.verdict))
            .count()
    };
    (
        count(&[Verdict::Regressed, Verdict::Invalid]),
        count(&[Verdict::Inconclusive]),
    )
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
        Verdict::Unchanged,
        Verdict::Inconclusive,
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
                    // One run per round.
                    blocks: vec![1; wall_s.len()],
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
    fn the_block_tests_are_exact() {
        let close = |p: f64, expected: f64| assert!((p - expected).abs() < 1e-12, "{p}");
        let rounds = |last: f64| [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, last];
        // 8 rounds the same way: 2 of the 2^8 equally likely signs.
        close(signed_rank_p(&rounds(8.0)), 2.0 / 256.0);
        close(signed_rank_p(&rounds(-8.0)), 50.0 / 256.0);
        // Zeros drop out; tied magnitudes share their mean rank.
        close(signed_rank_p(&[0.0, 0.0, 1.0, 2.0, 3.0]), 2.0 / 8.0);
        close(signed_rank_p(&[1.0, 1.0, -1.0, 2.0, 3.0]), 8.0 / 32.0);
        close(signed_rank_p(&[0.0; 4]), 1.0);
        // 5 blocks a side, separated: 2 of the C(10, 5) splits, ties or
        // not.
        let fives = [1.0, 2.0, 3.0, 4.0, 5.0];
        close(rank_sum_p(&fives, &fives.map(|v| v + 5.0)), 2.0 / 252.0);
        close(rank_sum_p(&[0.05; 5], &[0.06; 5]), 2.0 / 252.0);
        close(rank_sum_p(&[1.0, 1.0, 2.0], &[2.0, 3.0, 3.0]), 4.0 / 20.0);
        close(rank_sum_p(&[1.0; 5], &[1.0; 5]), 1.0);
    }

    #[test]
    fn rounds_pair_only_within_one_layout() {
        // Load that drifts across 10 rounds alike for both sides, and a
        // steady 1% increase: paired, every round shows it; unpaired, the
        // drift hides it.
        let base: Vec<f64> = (0..10).map(|i| 1.0 + f64::from(i) * 0.05).collect();
        let cand: Vec<f64> = base.iter().map(|v| v * 1.01).collect();
        let (p, reachable) = block_test(&base, &[1; 10], &cand, &[1; 10]);
        assert!(p <= ALPHA && reachable <= ALPHA, "{p}");
        let (p, reachable) = block_test(&base, &[2; 5], &cand, &[1; 10]);
        assert!(p > ALPHA && reachable <= ALPHA, "{p}");
        // Blocks that do not hold the runs cannot be tested.
        assert_eq!(block_test(&base, &[1; 9], &cand, &[1; 9]), (1.0, 1.0));
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
        assert!(block_test(&base, &[1; 10], &cand, &[1; 10]).0 > ALPHA);
        assert_eq!(verdict(base, cand), Verdict::Unchanged);
        // Unpaired blocks that separate, by less than the interquartile
        // spread of the runs.
        let base: Vec<f64> = (0..40).map(|i| 0.6 + f64::from(i % 10) * 0.1).collect();
        let cand: Vec<f64> = base.iter().map(|v| v + 0.25).collect();
        assert!(block_test(&base, &[2; 20], &cand, &[1; 40]).0 <= ALPHA);
        let mut baseline = results("base", vec![("s", base, true)]);
        baseline.scenarios[0].blocks = vec![2; 20];
        let rows = compare(&baseline, &results("cand", vec![("s", cand, true)]));
        assert_eq!(rows[0].verdict, Verdict::Unchanged);
        // Separated, but within the wider side's spread whichever side
        // is the baseline: +102% from the narrow side, -50% from the wide.
        let wide = vec![0.6, 0.62, 0.7, 0.75, 0.9, 1.1, 1.25, 1.3, 1.35, 1.4];
        let narrow: Vec<f64> = (0..10).map(|i| 0.45 + f64::from(i) * 0.01).collect();
        assert_eq!(verdict(narrow.clone(), wide.clone()), Verdict::Unchanged);
        assert_eq!(verdict(wide, narrow), Verdict::Unchanged);
    }

    #[test]
    fn paired_noise_is_the_spread_of_the_rounds() {
        // Load drifting 2% a round moves both sides alike.  Every round
        // moved 8%, which the rounds' differences show and the spread of
        // either side's runs, 9%, would hide.
        let base: Vec<f64> = (0..10).map(|i| 1.0 + f64::from(i) * 0.02).collect();
        let cand: Vec<f64> = base.iter().map(|v| v * 1.08).collect();
        assert_eq!(verdict(base, cand), Verdict::Regressed);
    }

    #[test]
    fn too_few_blocks_are_inconclusive() {
        let base = |n| {
            (0..n)
                .map(|i| 1.0 + f64::from(i) * 1e-3)
                .collect::<Vec<_>>()
        };
        let shifted = |n| base(n).into_iter().map(|v| v * 1.2).collect::<Vec<_>>();
        // 7 rounds that all moved the same way cannot reach 1%: no
        // claim either way.  Ties do not help.
        assert_eq!(verdict(base(7), shifted(7)), Verdict::Inconclusive);
        assert_eq!(verdict(base(7), base(7)), Verdict::Inconclusive);
        assert_eq!(verdict(vec![0.05; 5], vec![0.06; 5]), Verdict::Inconclusive);
        assert_eq!(verdict(base(8), shifted(8)), Verdict::Regressed);
        assert_eq!(verdict(base(8), base(8)), Verdict::Unchanged);
        // Unpaired, 5 blocks against 10 can reach it, 2 against 10 not.
        let unpaired = |blocks: Vec<u32>| {
            let mut baseline = results("base", vec![("s", base(10), true)]);
            baseline.scenarios[0].blocks = blocks;
            compare(&baseline, &results("cand", vec![("s", shifted(10), true)]))[0].verdict
        };
        assert_eq!(unpaired(vec![2; 5]), Verdict::Regressed);
        assert_eq!(unpaired(vec![5; 2]), Verdict::Inconclusive);
    }

    #[test]
    fn a_gate_passes_only_a_comparison_that_could_tell() {
        let rows = |verdicts: &[Verdict]| -> Vec<Comparison> {
            verdicts
                .iter()
                .map(|&verdict| Comparison {
                    scenario: "s".to_owned(),
                    metric: "wall".to_owned(),
                    baseline_median: None,
                    candidate_median: None,
                    change: None,
                    noise: None,
                    verdict,
                })
                .collect()
        };
        for passing in [
            Verdict::Improved,
            Verdict::Unchanged,
            Verdict::Increased,
            Verdict::Decreased,
            Verdict::Incomparable,
            Verdict::NoBaseline,
        ] {
            assert_eq!(gate_failures(&rows(&[passing])), (0, 0), "{passing:?}");
        }
        assert_eq!(gate_failures(&rows(&[Verdict::Regressed])), (1, 0));
        assert_eq!(gate_failures(&rows(&[Verdict::Invalid])), (1, 0));
        assert_eq!(gate_failures(&rows(&[Verdict::Inconclusive; 3])), (0, 3));
        let mixed = [
            Verdict::Unchanged,
            Verdict::Regressed,
            Verdict::Improved,
            Verdict::Inconclusive,
        ];
        assert_eq!(gate_failures(&rows(&mixed)), (1, 1));
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
