//! Rendering: the bd-shaped `--json` output and the human text.
//!
//! The JSON shapes follow br's, key for key, with two additions: every seed
//! object carries `revision` (the compare-and-set token) and `dependency_count`,
//! and every write's output carries the transaction id as `tx` (on the object,
//! or on each seed of a bare array) so a script can pass it to `--at`. Every
//! documented key is always present; absent values are `null`, never a missing
//! key. `tests/json_schema.rs` pins the key sets.

use serde_json::{json, Value as Json};

use crate::engine::{BlockedPage, Count, DepChange, DepRow, Page, SearchPage, SeedView, Stats};
use crate::model::{Comment, Seed};

/// `show --json`: an array of full seed objects.
pub fn show_json(views: &[SeedView]) -> Json {
    Json::Array(views.iter().map(view_json).collect())
}

fn view_json(v: &SeedView) -> Json {
    let mut o = v.seed.to_json();
    let edge = |id: &str, ty: &str, s: &Option<Seed>| {
        json!({
            "id": id,
            "title": s.as_ref().map(|s| s.title.clone()),
            "status": s.as_ref().map(|s| s.status.clone()),
            "priority": s.as_ref().map(|s| s.priority),
            "dependency_type": ty,
        })
    };
    o["dependencies"] = v
        .dependencies
        .iter()
        .map(|(id, ty, s)| edge(id, ty, s))
        .collect();
    o["dependents"] = v
        .dependents
        .iter()
        .map(|(id, ty, s)| edge(id, ty, s))
        .collect();
    o["comments"] = v.comments.iter().map(Comment::to_json).collect();
    o
}

/// `list --json`: br's paged envelope.
pub fn list_json(p: &Page) -> Json {
    let issue = |s: &Seed| {
        let mut o = s.to_json();
        o["dependent_count"] = json!(p.dependent_counts.get(&s.id).copied().unwrap_or(0));
        o
    };
    json!({
        "issues": p.issues.iter().map(issue).collect::<Vec<_>>(),
        "total": p.total,
        "limit": p.limit,
        "offset": p.offset,
        "has_more": p.has_more,
    })
}

/// `search --json`: br's envelope, `{issues, hidden_closed_count, limit, offset,
/// has_more}`, plus `total`.
pub fn search_json(r: &SearchPage) -> Json {
    let mut o = list_json(&r.page);
    o["hidden_closed_count"] = json!(r.hidden_closed);
    o["search_scope"] = json!(search_scope(r));
    o["search_notice"] = json!(if r.full {
        format!("{} full-field matches", r.page.total)
    } else {
        format!("{} title matches", r.page.total)
    });
    o
}

/// `search` as text: the hits, then how many closed ones were hidden.
pub fn search_text(r: &SearchPage, query: &str, layout: Layout) -> String {
    let mut out = if !r.full && r.page.total == 0 {
        "0 title matches".into()
    } else {
        page_text_layout(&r.page, "matching", layout)
    };
    if r.hidden_closed > 0 {
        out.push_str(&format!(
            "\n({} closed seed{} also match {query:?}; --all shows them)",
            r.hidden_closed,
            if r.hidden_closed == 1 { "" } else { "s" }
        ));
    }
    out.push('\n');
    out.push_str(search_scope(r));
    out
}

/// Scope travels with every result, so a title-only zero cannot claim absence
/// from the descriptions or comments that were not queried.
pub fn search_scope(r: &SearchPage) -> &'static str {
    if r.full {
        "searched: IDs, titles, descriptions and comments (--full)"
    } else {
        "searched: titles only (IDs/descriptions/comments NOT searched by the query; full: sd search --full)"
    }
}

/// Stamp a write's transaction on its `--json` output: on an object, or on each
/// object of a bare array (br's shape for update/close stays an array).
pub fn with_tx(mut value: Json, tx: Option<u64>) -> Json {
    let tx = json!(tx);
    match &mut value {
        Json::Object(o) => {
            o.insert("tx".into(), tx);
        }
        Json::Array(items) => {
            for item in items.iter_mut().filter_map(Json::as_object_mut) {
                item.insert("tx".into(), tx.clone());
            }
        }
        _ => {}
    }
    value
}

/// `stats --json`: br's `{summary, breakdowns?}`. seeds has no drafts or
/// pins, so those counts are always 0.
pub fn stats_json(st: &Stats) -> Json {
    let mut o = json!({"summary": {
        "total_issues": st.total,
        "open_issues": st.open,
        "in_progress_issues": st.in_progress,
        "closed_issues": st.closed,
        "blocked_issues": st.blocked,
        "deferred_issues": st.deferred,
        "draft_issues": 0,
        "ready_issues": st.ready,
        "tombstone_issues": st.tombstones,
        "pinned_issues": 0,
        "epics_eligible_for_closure": st.epics_eligible_for_closure,
        "average_lead_time_hours": st.average_lead_time_hours,
    }});
    if !st.breakdowns.is_empty() {
        o["breakdowns"] = st
            .breakdowns
            .iter()
            .map(|(dim, counts)| {
                json!({"dimension": dim, "counts": counts.iter()
                    .map(|(k, n)| json!({"key": k, "count": n})).collect::<Vec<_>>()})
            })
            .collect();
    }
    if let Some(a) = &st.activity {
        // br's keys. commit_count and issues_reopened are null, not 0: sd
        // cannot know them, and a 0 would claim it does.
        o["recent_activity"] = json!({
            "hours_tracked": a.hours,
            "commit_count": null,
            "issues_created": a.created,
            "issues_closed": a.closed,
            "issues_updated": a.updated,
            "issues_reopened": null,
            "total_changes": a.touched,
        });
    }
    o
}

/// `stats` as text.
pub fn stats_text(st: &Stats) -> String {
    let mut lines = vec![
        format!("total {}", st.total),
        format!(
            "  open {} · in progress {} · blocked {} · deferred {} · closed {}",
            st.open, st.in_progress, st.blocked, st.deferred, st.closed
        ),
        format!("  ready now {}", st.ready),
        format!("  epics ready to close {}", st.epics_eligible_for_closure),
        format!("  average lead time {:.1} h", st.average_lead_time_hours),
    ];
    for (dim, counts) in &st.breakdowns {
        lines.push(format!("by {dim}:"));
        lines.extend(counts.iter().map(|(k, n)| format!("  {k}: {n}")));
    }
    if let Some(a) = &st.activity {
        lines.push(format!("recent activity (last {} hours):", a.hours));
        lines.push(format!(
            "  created {} · closed {} · updated {} · seeds touched {}",
            a.created, a.closed, a.updated, a.touched
        ));
        lines.push("  reopened and commits: not tracked by sd".into());
    }
    lines.join("\n")
}

/// `blocked --json`: the list envelope; each seed also carries br's
/// `blocked_by` (open blocker ids) and `blocked_by_count`.
pub fn blocked_json(b: &BlockedPage) -> Json {
    let mut o = list_json(&b.page);
    if let Some(issues) = o["issues"].as_array_mut() {
        for issue in issues {
            let by = issue["id"]
                .as_str()
                .and_then(|id| b.blocked_by.get(id))
                .cloned()
                .unwrap_or_default();
            issue["blocked_by_count"] = json!(by.len());
            issue["blocked_by"] = json!(by);
        }
    }
    o
}

/// `blocked` as text: each seed and what blocks it; says when cut short.
pub fn blocked_text(b: &BlockedPage, detailed: bool) -> String {
    let p = &b.page;
    if p.issues.is_empty() {
        return "no blocked seeds".to_string();
    }
    let mut lines = vec![format!("blocked seeds ({}):", p.total)];
    for s in &p.issues {
        let by = b.blocked_by.get(&s.id).cloned().unwrap_or_default();
        lines.push(seed_line(s));
        if detailed {
            // br's --detailed: each blocker with its title, priority and status.
            lines.push("  blocked by:".into());
            for id in &by {
                lines.push(match b.blockers.get(id) {
                    Some(x) => format!("    • {id}: {} [P{}] [{}]", x.title, x.priority, x.status),
                    None => format!("    • {id}"),
                });
            }
        } else {
            lines.push(format!("  blocked by {} open: {}", by.len(), by.join(", ")));
        }
    }
    if p.has_more {
        lines.push(format!(
            "showing {} of {} (--limit 0 for all)",
            p.issues.len(),
            p.total
        ));
    }
    lines.join("\n")
}

/// `ready --json`, `update --json`, `close --json`, `create --json` (one
/// element): a bare array of seed objects, as br prints them.
pub fn seeds_json(seeds: &[Seed]) -> Json {
    Json::Array(seeds.iter().map(Seed::to_json).collect())
}

/// `count --json`: `{"count": n}`, or br's grouped shape with `--by`.
pub fn count_json(c: &Count) -> Json {
    match &c.groups {
        None => json!({ "count": c.total }),
        Some(g) => json!({
            "total": c.total,
            "groups": g.iter().map(|(k, n)| json!({"group": k, "count": n})).collect::<Vec<_>>(),
        }),
    }
}

/// `dep add|remove --json`.
pub fn dep_change_json(d: &DepChange) -> Json {
    json!({
        "status": "ok",
        "issue_id": d.issue_id,
        "depends_on_id": d.depends_on_id,
        "type": d.dep_type,
        "action": d.action,
        "tx": d.tx,
    })
}

/// `dep list --json`.
pub fn dep_list_json(rows: &[DepRow]) -> Json {
    Json::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "issue_id": r.issue_id,
                    "depends_on_id": r.depends_on_id,
                    "type": r.dep_type,
                    "title": r.other.as_ref().map(|s| s.title.clone()),
                    "status": r.other.as_ref().map(|s| s.status.clone()),
                    "priority": r.other.as_ref().map(|s| s.priority),
                })
            })
            .collect(),
    )
}

/// `comments list --json`.
pub fn comments_json(cs: &[Comment]) -> Json {
    Json::Array(cs.iter().map(Comment::to_json).collect())
}

/// An error, as `--json` callers receive it (on stdout, like br).
pub fn error_json(kind: &str, message: &str) -> Json {
    json!({ "error": { "code": kind, "message": message } })
}

// ---------------------------------------------------------------- text

fn status_glyph(s: &Seed) -> &'static str {
    match s.status.as_str() {
        "closed" => "✓",
        "in_progress" => "◐",
        "blocked" => "●",
        "deferred" => "❄",
        _ => "○",
    }
}

/// One line per seed.
pub fn seed_line(s: &Seed) -> String {
    let mut line = format!(
        "{} {} [P{}] [{}] {}",
        status_glyph(s),
        s.id,
        s.priority,
        s.issue_type,
        s.title
    );
    if let Some(a) = &s.assignee {
        line.push_str(&format!(" @{a}"));
    }
    if !s.labels.is_empty() {
        let labels: Vec<&str> = s.labels.iter().map(String::as_str).collect();
        line.push_str(&format!(" #{}", labels.join(" #")));
    }
    line
}

/// How `list`/`search` lay out text (br's `--long`, `--pretty`, `--tree`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// One line per seed.
    #[default]
    Line,
    /// Each line, then its fields indented, then a blank line.
    Long,
    /// Each line, then its fields with tree connectors, then a blank line.
    Pretty,
    /// Children nested under their parents with tree connectors.
    Tree,
}

/// br's `--long`/`--pretty` field block: status, priority, type, assignee (if
/// any), and for `--long` created and updated (br's `--pretty` has no dates).
fn seed_fields(s: &Seed, dates: bool) -> Vec<String> {
    let mut f = vec![
        format!("Status: {}", s.status),
        format!("Priority: P{}", s.priority),
        format!("Type: {}", s.issue_type),
    ];
    if let Some(a) = &s.assignee {
        f.push(format!("Assignee: {a}"));
    }
    if dates {
        f.push(format!("Created: {}", s.created_at));
        f.push(format!("Updated: {}", s.updated_at));
    }
    f
}

/// Seeds nested under their parents, in the given order. A seed whose parent
/// is not in the list is a root. A parent loop (possible only in a damaged
/// ledger) cannot hide or repeat a seed: anything no root reaches is printed
/// as a root too.
fn tree_lines(seeds: &[Seed]) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let ids: BTreeSet<&str> = seeds.iter().map(|s| s.id.as_str()).collect();
    let mut children: BTreeMap<&str, Vec<&Seed>> = BTreeMap::new();
    for s in seeds {
        if let Some(p) = s
            .parent
            .as_deref()
            .filter(|p| ids.contains(p) && *p != s.id)
        {
            children.entry(p).or_default().push(s);
        }
    }
    fn walk<'a>(
        s: &'a Seed,
        prefix: &str,
        connector: &str,
        children: &BTreeMap<&str, Vec<&'a Seed>>,
        seen: &mut BTreeSet<&'a str>,
        out: &mut Vec<String>,
    ) {
        if !seen.insert(s.id.as_str()) {
            return;
        }
        out.push(format!("{prefix}{connector}{}", seed_line(s)));
        let kids = children
            .get(s.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let next = match connector {
            "├── " => format!("{prefix}│   "),
            "└── " => format!("{prefix}    "),
            _ => prefix.to_string(),
        };
        for (i, k) in kids.iter().enumerate() {
            let c = if i + 1 == kids.len() {
                "└── "
            } else {
                "├── "
            };
            walk(k, &next, c, children, seen, out);
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let roots = seeds.iter().filter(|s| {
        s.parent
            .as_deref()
            .is_none_or(|p| !ids.contains(p) || p == s.id)
    });
    for s in roots {
        walk(s, "", "", &children, &mut seen, &mut out);
    }
    for s in seeds {
        walk(s, "", "", &children, &mut seen, &mut out);
    }
    out
}

/// Seeds in a [`Layout`].
pub fn seeds_layout(seeds: &[Seed], layout: Layout) -> Vec<String> {
    match layout {
        Layout::Line => seeds.iter().map(seed_line).collect(),
        Layout::Tree => tree_lines(seeds),
        Layout::Long | Layout::Pretty => {
            let mut out = Vec::new();
            for (i, s) in seeds.iter().enumerate() {
                if i > 0 {
                    out.push(String::new());
                }
                out.push(seed_line(s));
                let f = seed_fields(s, layout == Layout::Long);
                for (j, line) in f.iter().enumerate() {
                    let lead = match layout {
                        Layout::Long => "  ",
                        _ if j + 1 == f.len() => "└── ",
                        _ => "├── ",
                    };
                    out.push(format!("{lead}{line}"));
                }
            }
            out
        }
    }
}

/// A page of seeds, with a trailer that says when it was cut short.
pub fn page_text(p: &Page, what: &str) -> String {
    page_text_layout(p, what, Layout::Line)
}

/// [`page_text`] in a [`Layout`].
pub fn page_text_layout(p: &Page, what: &str, layout: Layout) -> String {
    let mut out: Vec<String> = seeds_layout(&p.issues, layout);
    if p.issues.is_empty() {
        out.push(format!("no {what} seeds"));
    }
    if p.has_more {
        out.push(format!(
            "(showing {} of {}; --limit 0 shows all)",
            p.issues.len(),
            p.total
        ));
    }
    out.join("\n")
}

/// The long form `show` prints.
pub fn view_text(v: &SeedView) -> String {
    let s = &v.seed;
    let mut out = vec![
        seed_line(s),
        format!(
            "  status {} · created {} · updated {} · revision {}",
            s.status, s.created_at, s.updated_at, s.revision
        ),
    ];
    if let Some(c) = &s.closed_at {
        out.push(format!(
            "  closed {c}: {}",
            s.close_reason.as_deref().unwrap_or("(no reason)")
        ));
    }
    if let Some(d) = &s.defer_until {
        out.push(format!("  deferred until {d}"));
    }
    if let Some(d) = &s.due_at {
        out.push(format!("  due {d}"));
    }
    if let Some(m) = s.estimated_minutes {
        out.push(format!("  estimate {m}m"));
    }
    if let Some(d) = &s.description {
        out.push(String::new());
        out.extend(d.lines().map(|l| format!("  {l}")));
    }
    for (label, text) in [
        ("notes", &s.notes),
        ("design", &s.design),
        ("acceptance criteria", &s.acceptance_criteria),
    ] {
        if let Some(t) = text {
            out.push(String::new());
            out.push(format!("  {label}:"));
            out.extend(t.lines().map(|l| format!("    {l}")));
        }
    }
    if let Some(e) = &s.external_ref {
        out.push(format!("  external ref: {e}"));
    }
    if let Some(c) = &s.agent_context {
        out.push(format!("  agent context: {c}"));
    }
    if s.ephemeral {
        out.push("  ephemeral: not in the shared ledger, never ready".into());
    }
    if !v.dependencies.is_empty() {
        out.push(String::new());
        out.push("  depends on:".into());
        for (id, ty, t) in &v.dependencies {
            out.push(edge_line(id, ty, t));
        }
    }
    if !v.dependents.is_empty() {
        out.push(String::new());
        out.push("  depended on by:".into());
        for (id, ty, t) in &v.dependents {
            out.push(edge_line(id, ty, t));
        }
    }
    if !v.comments.is_empty() {
        out.push(String::new());
        out.push("  comments:".into());
        for c in &v.comments {
            out.push(format!(
                "    [{}] {} ({}):",
                c.index, c.author, c.created_at
            ));
            out.extend(c.text.lines().map(|l| format!("      {l}")));
        }
    }
    out.join("\n")
}

fn edge_line(id: &str, ty: &str, s: &Option<Seed>) -> String {
    match s {
        Some(s) => format!("    {id} ({ty}) [{}] {}", s.status, s.title),
        None => format!("    {id} ({ty}) (not in this project)"),
    }
}

/// `count` in text.
pub fn count_text(c: &Count) -> String {
    match &c.groups {
        None => c.total.to_string(),
        Some(g) => {
            let mut out: Vec<String> = g.iter().map(|(k, n)| format!("{k}: {n}")).collect();
            out.push(format!("total: {}", c.total));
            out.join("\n")
        }
    }
}

/// The `--format csv` columns sd can fill: br's list.
pub const CSV_FIELDS: &[&str] = &[
    "id",
    "title",
    "description",
    "status",
    "priority",
    "issue_type",
    "assignee",
    "owner",
    "created_at",
    "updated_at",
    "closed_at",
    "due_at",
    "defer_until",
    "notes",
    "external_ref",
];

/// br's default `--format csv` columns.
pub const CSV_DEFAULT: &str = "id,title,status,priority,issue_type,assignee,created_at,updated_at";

/// Parse a `--fields` list, refusing a column sd cannot fill rather than
/// dropping it (br drops unknown columns silently).
pub fn csv_fields(spec: &str) -> crate::error::Result<Vec<String>> {
    let fields: Vec<String> = spec.split(',').map(|f| f.trim().to_string()).collect();
    for f in &fields {
        if !CSV_FIELDS.contains(&f.as_str()) {
            return Err(crate::error::SdError::usage(format!(
                "--fields: sd has no column {f:?}; expected some of {}",
                CSV_FIELDS.join(", ")
            )));
        }
    }
    Ok(fields)
}

/// Seeds as RFC 4180 CSV: a header row, then one row per seed. A value with a
/// comma, quote or newline is quoted, and its quotes are doubled.
pub fn seeds_csv(seeds: &[Seed], fields: &[String]) -> String {
    fn cell(v: &str) -> String {
        if v.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_string()
        }
    }
    let mut out = vec![fields.join(",")];
    for s in seeds {
        let row: Vec<String> = fields
            .iter()
            .map(|f| {
                let v = match f.as_str() {
                    "id" => s.id.clone(),
                    "title" => s.title.clone(),
                    "description" => s.description.clone().unwrap_or_default(),
                    "status" => s.status.clone(),
                    "priority" => s.priority.to_string(),
                    "issue_type" => s.issue_type.clone(),
                    "assignee" => s.assignee.clone().unwrap_or_default(),
                    "owner" => s.owner.clone().unwrap_or_default(),
                    "created_at" => s.created_at.clone(),
                    "updated_at" => s.updated_at.clone(),
                    "closed_at" => s.closed_at.clone().unwrap_or_default(),
                    "defer_until" => s.defer_until.clone().unwrap_or_default(),
                    "notes" => s.notes.clone().unwrap_or_default(),
                    "external_ref" => s.external_ref.clone().unwrap_or_default(),
                    "due_at" => s.due_at.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                cell(&v)
            })
            .collect();
        out.push(row.join(","));
    }
    out.join("\n")
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn seed(id: &str, parent: Option<&str>) -> Seed {
        Seed {
            id: id.into(),
            title: id.into(),
            status: "open".into(),
            priority: 2,
            issue_type: "task".into(),
            parent: parent.map(str::to_string),
            ..Seed::default()
        }
    }

    #[test]
    fn a_parent_loop_prints_every_seed_exactly_once() {
        // x and y are each other's parent (a damaged ledger): no root reaches
        // them, and a naive walk would drop them or recurse forever.
        let seeds = [seed("r", None), seed("x", Some("y")), seed("y", Some("x"))];
        let lines = tree_lines(&seeds);
        for id in ["r", "x", "y"] {
            let n = lines
                .iter()
                .filter(|l| l.contains(&format!(" {id} [P2]")))
                .count();
            assert_eq!(n, 1, "{id} in {lines:?}");
        }
    }

    #[test]
    fn nested_children_get_brs_connectors() {
        let seeds = [
            seed("p", None),
            seed("p.1", Some("p")),
            seed("p.1.1", Some("p.1")),
            seed("p.2", Some("p")),
        ];
        let lines: Vec<String> = tree_lines(&seeds)
            .iter()
            .map(|l| l.replace(" [P2] [task]", "").replace("○ ", ""))
            .collect();
        assert_eq!(
            lines,
            ["p p", "├── p.1 p.1", "│   └── p.1.1 p.1.1", "└── p.2 p.2"]
        );
    }
}
