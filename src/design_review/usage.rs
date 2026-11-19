//! Usage tab aggregation (plan section 7): pure functions over ledger rows.
//! Nothing here reads the database or touches `App`; the room loads
//! [`UsageRow`]s and renders what these functions return.

use std::collections::BTreeSet;

use crate::token_cost::ledger::{Agent, Phase, Task};
use crate::token_tracking::{format_dollar_cost, format_token_count};

/// One `design_review_usage` row, as the tab needs it. Token counts stay
/// `Option`: absent means unavailable, not zero.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageRow {
    pub id: i64,
    pub phase: String,
    pub task: String,
    pub agent: String,
    pub model: Option<String>,
    pub round: Option<i64>,
    pub item: Option<String>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub effective_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub estimated: bool,
    pub price_assumed: bool,
    pub cache_hit: bool,
    pub saved_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub started_at: String,
}

/// What the table's rows are grouped by. `g` cycles in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    Phase,
    Task,
    #[default]
    Agent,
    Model,
    Item,
    Round,
}

impl Grouping {
    pub const ALL: [Grouping; 6] = [
        Grouping::Phase,
        Grouping::Task,
        Grouping::Agent,
        Grouping::Model,
        Grouping::Item,
        Grouping::Round,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grouping::Phase => "phase",
            Grouping::Task => "task",
            Grouping::Agent => "agent",
            Grouping::Model => "model",
            Grouping::Item => "item",
            Grouping::Round => "round",
        }
    }

    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|g| *g == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    fn key(self, row: &UsageRow) -> String {
        match self {
            Grouping::Phase => row.phase.clone(),
            Grouping::Task => row.task.clone(),
            Grouping::Agent => row.agent.clone(),
            Grouping::Model => row.model.clone().unwrap_or_else(|| "(unknown)".to_string()),
            Grouping::Item => row
                .item
                .clone()
                .unwrap_or_else(|| "(packet-wide)".to_string()),
            Grouping::Round => row
                .round
                .map_or_else(|| "(none)".to_string(), |r| format!("round {r}")),
        }
    }

    /// Sort rank within the table: the ledger's own order for the closed
    /// vocabularies, numeric for rounds, then the key.
    fn rank(self, key: &str, row: &UsageRow) -> usize {
        let position = |names: Vec<&'static str>| {
            names
                .iter()
                .position(|name| *name == key)
                .unwrap_or(names.len())
        };
        match self {
            Grouping::Phase => position(Phase::ALL.iter().map(|p| p.as_str()).collect()),
            Grouping::Task => position(Task::ALL.iter().map(|t| t.as_str()).collect()),
            Grouping::Agent => position(Agent::ALL.iter().map(|a| a.as_str()).collect()),
            Grouping::Round => row.round.map_or(usize::MAX, |r| r.max(0) as usize),
            Grouping::Model | Grouping::Item => 0,
        }
    }
}

/// Sum of the present values; `None` when none is present.
fn sum_opt(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    values.fold(None, |acc, v| match (acc, v) {
        (None, v) => v,
        (acc, None) => acc,
        (Some(a), Some(b)) => Some(a + b),
    })
}

/// Totals for a set of rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UsageStats {
    pub calls: usize,
    /// Input plus cache read plus cache write, as the sidebar shows it.
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    /// Only when every row ran on the same single model: effective tokens do
    /// not add up across models.
    pub effective: Option<u64>,
    pub cost: Option<f64>,
    pub any_estimated: bool,
    pub price_assumed: bool,
}

impl UsageStats {
    pub fn of(rows: &[&UsageRow]) -> Self {
        let in_total =
            |row: &UsageRow| sum_opt([row.input, row.cache_read, row.cache_write].into_iter());
        let models: BTreeSet<&Option<String>> = rows.iter().map(|r| &r.model).collect();
        let single_model = models.len() == 1 && models.iter().all(|m| m.is_some());
        let cost = rows
            .iter()
            .filter_map(|r| r.cost_usd)
            .fold(None, |acc: Option<f64>, c| Some(acc.unwrap_or(0.0) + c));
        Self {
            calls: rows.len(),
            input: sum_opt(rows.iter().map(|r| in_total(r))),
            output: sum_opt(rows.iter().map(|r| r.output)),
            cache_read: sum_opt(rows.iter().map(|r| r.cache_read)),
            effective: if single_model {
                sum_opt(rows.iter().map(|r| r.effective_tokens))
            } else {
                None
            },
            cost,
            any_estimated: rows.iter().any(|r| r.estimated),
            price_assumed: rows.iter().any(|r| r.price_assumed),
        }
    }
}

/// One table row: a group and its totals.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageGroup {
    pub key: String,
    pub stats: UsageStats,
    /// Share of the table's cost, 0-100, when the table has any cost.
    pub percent: Option<u32>,
    /// Indices into the row slice the group was built from.
    pub rows: Vec<usize>,
}

/// The whole table.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageTable {
    pub grouping: Grouping,
    pub groups: Vec<UsageGroup>,
    pub total: UsageStats,
    pub premium_usd: f64,
    pub staff_usd: f64,
    pub saved_usd: f64,
    pub sections_reused: usize,
}

pub fn group_usage(rows: &[UsageRow], grouping: Grouping) -> UsageTable {
    let mut keyed: Vec<(String, usize)> = Vec::new();
    let mut members: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (index, row) in rows.iter().enumerate() {
        let key = grouping.key(row);
        if !members.contains_key(&key) {
            keyed.push((key.clone(), grouping.rank(&key, row)));
        }
        members.entry(key).or_default().push(index);
    }
    keyed.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));

    let all: Vec<&UsageRow> = rows.iter().collect();
    let total = UsageStats::of(&all);
    let total_cost = total.cost.unwrap_or(0.0);
    let groups = keyed
        .into_iter()
        .map(|(key, _)| {
            let indices = members.remove(&key).unwrap_or_default();
            let refs: Vec<&UsageRow> = indices.iter().map(|i| &rows[*i]).collect();
            let stats = UsageStats::of(&refs);
            let percent = (total_cost > 0.0)
                .then(|| ((stats.cost.unwrap_or(0.0) / total_cost) * 100.0).round() as u32);
            UsageGroup {
                key,
                stats,
                percent,
                rows: indices,
            }
        })
        .collect();

    let spend = |developer: bool| {
        rows.iter()
            .filter(|r| (r.agent == Agent::Developer.as_str()) == developer)
            .filter_map(|r| r.cost_usd)
            .sum::<f64>()
    };
    UsageTable {
        grouping,
        groups,
        total,
        premium_usd: spend(true),
        staff_usd: spend(false),
        saved_usd: rows.iter().filter_map(|r| r.saved_usd).sum(),
        sections_reused: rows.iter().filter(|r| r.cache_hit).count(),
    }
}

/// Dollars for the table; an exact zero reads `$0.00`, not `<$0.01`.
pub fn money(usd: f64) -> String {
    if usd == 0.0 {
        "$0.00".to_string()
    } else {
        format_dollar_cost(usd)
    }
}

fn tokens(value: Option<u64>, estimated: bool) -> String {
    match value {
        None => "-".to_string(),
        Some(v) => format!(
            "{}{}",
            if estimated { "~" } else { "" },
            format_token_count(v)
        ),
    }
}

/// A cost cell: `-` unavailable, `~` estimated, `*` price assumed.
pub fn cost_cell(cost: Option<f64>, estimated: bool, price_assumed: bool) -> String {
    match cost {
        None => "-".to_string(),
        Some(c) => format!(
            "{}{}{}",
            if estimated { "~" } else { "" },
            money(c),
            if price_assumed { "*" } else { "" }
        ),
    }
}

/// Cells of one table row, in column order:
/// name, Calls, In, Out, Cache rd, Eff, Cost, %.
pub fn stats_cells(name: &str, stats: &UsageStats, percent: Option<u32>) -> Vec<String> {
    let est = stats.any_estimated;
    vec![
        name.to_string(),
        format!("{}{}", if est { "~" } else { "" }, stats.calls),
        tokens(stats.input, est),
        tokens(stats.output, est),
        tokens(stats.cache_read, est),
        tokens(stats.effective, est),
        cost_cell(stats.cost, est, stats.price_assumed),
        percent.map_or_else(|| "-".to_string(), |p| format!("{p}%")),
    ]
}

pub const COLUMNS: [&str; 8] = ["", "Calls", "In", "Out", "Cache rd", "Eff", "Cost", "%"];

/// Every row of the table including the total, as cells.
pub fn table_cells(table: &UsageTable) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = table
        .groups
        .iter()
        .map(|g| stats_cells(&g.key, &g.stats, g.percent))
        .collect();
    out.push(stats_cells("Total", &table.total, None));
    out
}

pub fn footer_line(table: &UsageTable) -> String {
    format!(
        "Premium {} · Staff {} · Saved by cache {} ({} sections reused)",
        money(table.premium_usd),
        money(table.staff_usd),
        money(table.saved_usd),
        table.sections_reused
    )
}

pub const MARKER_LEGEND: &str = "~ estimated   * price assumed   - unavailable";

/// The table as Markdown, for the clipboard.
pub fn to_markdown(table: &UsageTable) -> String {
    let mut out = String::new();
    let head: Vec<String> = COLUMNS
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                let label = table.grouping.label();
                let mut chars = label.chars();
                chars
                    .next()
                    .map(|f| f.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            } else {
                (*c).to_string()
            }
        })
        .collect();
    out.push_str(&format!("| {} |\n", head.join(" | ")));
    out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for cells in table_cells(table) {
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out.push('\n');
    out.push_str(&footer_line(table));
    out.push_str("\n\n");
    out.push_str(MARKER_LEGEND);
    out.push('\n');
    out
}

/// Time-of-day from a stored timestamp (`2026-11-13T10:11:12Z` or
/// `2026-11-13 10:11:12.345`), else the text as is.
fn clock(started_at: &str) -> String {
    let rest = started_at
        .split_once(['T', ' '])
        .map_or(started_at, |(_, time)| time);
    rest.chars().take(8).collect()
}

fn duration(ms: Option<u64>) -> String {
    match ms {
        None => "-".to_string(),
        Some(ms) if ms < 1_000 => format!("{ms}ms"),
        Some(ms) => format!("{:.1}s", ms as f64 / 1_000.0),
    }
}

/// Cells of one call in a group's drill-down: time, task, model, tokens,
/// cost, duration, item.
pub fn call_cells(row: &UsageRow) -> Vec<String> {
    let tokens_cell = if row.cache_hit {
        "reused".to_string()
    } else {
        let in_total = sum_opt([row.input, row.cache_read, row.cache_write].into_iter());
        format!(
            "{} in / {} out",
            tokens(in_total, row.estimated),
            tokens(row.output, row.estimated)
        )
    };
    vec![
        clock(&row.started_at),
        row.task.clone(),
        row.model.clone().unwrap_or_else(|| "-".to_string()),
        tokens_cell,
        cost_cell(row.cost_usd, row.estimated, row.price_assumed),
        duration(row.duration_ms),
        row.item.clone().unwrap_or_else(|| "-".to_string()),
    ]
}

pub const CALL_COLUMNS: [&str; 7] = [
    "Time", "Task", "Model", "Tokens", "Cost", "Duration", "Item",
];

/// The header's one-line total: dollars, or `—` when the ledger has no cost.
pub fn header_total(rows: &[UsageRow]) -> String {
    let all: Vec<&UsageRow> = rows.iter().collect();
    UsageStats::of(&all)
        .cost
        .map_or_else(|| "—".to_string(), money)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn row(agent: &str, task: &str, model: &str, input: u64, output: u64, cost: f64) -> UsageRow {
        UsageRow {
            phase: "prepare".into(),
            task: task.into(),
            agent: agent.into(),
            model: Some(model.into()),
            round: Some(1),
            input: Some(input),
            output: Some(output),
            cache_read: Some(0),
            cache_write: Some(0),
            effective_tokens: Some(input + output * 5),
            cost_usd: Some(cost),
            started_at: "2026-11-13T10:11:12Z".into(),
            ..Default::default()
        }
    }

    fn sample() -> Vec<UsageRow> {
        let mut capture = row("developer", "decision-record", "fable", 1000, 100, 0.24);
        capture.phase = "capture".into();
        capture.estimated = true;
        let mut cached = row("cartographer", "architecture", "haiku", 0, 0, 0.0);
        cached.cache_hit = true;
        cached.saved_usd = Some(0.02);
        cached.item = Some("architecture".into());
        let mut assumed = row("presenter", "question-L1", "mystery", 500, 50, 0.1);
        assumed.price_assumed = true;
        vec![
            row("developer", "question-L3", "fable", 3000, 400, 0.6),
            capture,
            row("presenter", "brief", "sonnet", 800, 80, 0.1),
            assumed,
            cached,
            row("scribe", "unexplained", "haiku", 200, 20, 0.0),
        ]
    }

    #[test]
    fn grouping_cycles_through_all_six_and_back() {
        let mut g = Grouping::Phase;
        let mut seen = vec![g.label()];
        for _ in 0..6 {
            g = g.next();
            seen.push(g.label());
        }
        assert_eq!(
            seen,
            ["phase", "task", "agent", "model", "item", "round", "phase"]
        );
    }

    #[test]
    fn every_grouping_partitions_the_rows() {
        let rows = sample();
        for grouping in Grouping::ALL {
            let table = group_usage(&rows, grouping);
            let calls: usize = table.groups.iter().map(|g| g.stats.calls).sum();
            assert_eq!(calls, rows.len(), "{grouping:?}");
            assert_eq!(table.total.calls, rows.len());
        }
        let by_agent = group_usage(&rows, Grouping::Agent);
        let names: Vec<_> = by_agent.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(names, ["developer", "scribe", "cartographer", "presenter"]);
        let by_phase = group_usage(&rows, Grouping::Phase);
        let names: Vec<_> = by_phase.groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(names, ["capture", "prepare"]);
        let by_item = group_usage(&rows, Grouping::Item);
        assert!(by_item.groups.iter().any(|g| g.key == "(packet-wide)"));
        assert!(by_item.groups.iter().any(|g| g.key == "architecture"));
        let by_round = group_usage(&rows, Grouping::Round);
        assert_eq!(by_round.groups[0].key, "round 1");
        let by_task = group_usage(&rows, Grouping::Task);
        assert_eq!(by_task.groups[0].key, "brief");
    }

    #[test]
    fn effective_total_only_for_a_single_model_group() {
        let rows = sample();
        let by_agent = group_usage(&rows, Grouping::Agent);
        let find = |k: &str| by_agent.groups.iter().find(|g| g.key == k).unwrap();
        // developer rows are all fable.
        assert_eq!(find("developer").stats.effective, Some(5000 + 1500));
        // presenter mixes sonnet and mystery.
        assert_eq!(find("presenter").stats.effective, None);
        // The grand total mixes models, so it has none either.
        assert_eq!(by_agent.total.effective, None);
        let cells = table_cells(&by_agent);
        assert_eq!(cells.last().unwrap()[5], "-");

        let by_model = group_usage(&rows, Grouping::Model);
        assert!(
            by_model
                .groups
                .iter()
                .all(|g| g.stats.effective.is_some() || g.stats.calls == 0)
        );
    }

    #[test]
    fn markers_mark_estimated_assumed_and_unavailable() {
        let rows = sample();
        let by_task = group_usage(&rows, Grouping::Task);
        let cells = |k: &str| {
            let g = by_task.groups.iter().find(|g| g.key == k).unwrap();
            stats_cells(&g.key, &g.stats, g.percent)
        };
        let capture = cells("decision-record");
        assert_eq!(capture[1], "~1");
        assert!(capture[2].starts_with('~'), "{capture:?}");
        assert!(capture[6].starts_with("~$"), "{capture:?}");
        let assumed = cells("question-L1");
        assert!(assumed[6].ends_with('*'), "{assumed:?}");

        let unavailable = UsageRow {
            agent: "presenter".into(),
            task: "brief".into(),
            ..Default::default()
        };
        let table = group_usage(std::slice::from_ref(&unavailable), Grouping::Task);
        let cells = stats_cells("brief", &table.groups[0].stats, table.groups[0].percent);
        assert_eq!(cells[2], "-");
        assert_eq!(cells[3], "-");
        assert_eq!(cells[6], "-");
        assert_eq!(cells[7], "-");
        assert_eq!(header_total(&[unavailable]), "—");
    }

    #[test]
    fn premium_and_staff_split_by_the_developer_agent() {
        let rows = sample();
        let table = group_usage(&rows, Grouping::Agent);
        assert!(
            (table.premium_usd - 0.84).abs() < 1e-9,
            "{}",
            table.premium_usd
        );
        assert!((table.staff_usd - 0.2).abs() < 1e-9, "{}", table.staff_usd);
        assert!((table.saved_usd - 0.02).abs() < 1e-9);
        assert_eq!(table.sections_reused, 1);
        assert_eq!(
            footer_line(&table),
            "Premium $0.84 · Staff $0.20 · Saved by cache $0.02 (1 sections reused)"
        );
        let dev = &table.groups[0];
        assert_eq!(dev.percent, Some(81));
    }

    #[test]
    fn markdown_copy_carries_table_footer_and_legend() {
        let rows = vec![
            row("developer", "question-L3", "fable", 3000, 400, 0.6),
            row("presenter", "brief", "sonnet", 800, 80, 0.2),
        ];
        let md = to_markdown(&group_usage(&rows, Grouping::Agent));
        let lines: Vec<&str> = md.lines().collect();
        assert_eq!(
            lines[0],
            "| Agent | Calls | In | Out | Cache rd | Eff | Cost | % |"
        );
        assert!(lines[1].starts_with("| --- |"));
        assert!(
            lines[2].starts_with("| developer | 1 | 3.0k | 400 |"),
            "{md}"
        );
        assert!(lines[2].ends_with("| $0.60 | 75% |"), "{md}");
        assert!(lines[4].starts_with("| Total | 2 |"), "{md}");
        assert!(
            md.contains("Premium $0.60 · Staff $0.20 · Saved by cache $0.00 (0 sections reused)")
        );
        assert!(md.contains(MARKER_LEGEND));
    }

    #[test]
    fn call_cells_show_time_task_model_tokens_cost_duration_item() {
        let mut r = row("presenter", "question-L1", "sonnet", 800, 80, 0.1);
        r.duration_ms = Some(1_500);
        r.item = Some("d3".into());
        assert_eq!(
            call_cells(&r),
            [
                "10:11:12",
                "question-L1",
                "sonnet",
                "800 in / 80 out",
                "$0.10",
                "1.5s",
                "d3"
            ]
        );
        r.cache_hit = true;
        assert_eq!(call_cells(&r)[3], "reused");
    }

    #[test]
    fn header_total_is_a_dollar_figure_or_a_dash() {
        assert_eq!(header_total(&[]), "—");
        assert_eq!(header_total(&sample()), "$1.04");
    }
}
