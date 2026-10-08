//! Indexed summaries: no ledger snapshot, seed body or comment materialization.
use super::*;
use crate::engine::{self, Activity, Count, CountReq, Filter, Stats, StatsReq};

fn literal(s: &str) -> String {
    format!("\"{}\"", escape_literal(s))
}

fn string_field(predicate: &str, field: &str, default: &str) -> String {
    format!("OPTIONAL {{ ?s <{predicate}> ?raw_{field} . FILTER(isLiteral(?raw_{field}) && sameTerm(?raw_{field}, STR(?raw_{field}))) }} BIND(COALESCE(?raw_{field}, {}) AS ?{field})", literal(default))
}

// Inline expressions retain only the two timestamp bindings: BINDing every
// calendar intermediate would multiply the aggregate's working set.
fn epoch_expression(variable: &str) -> String {
    let date = format!("STRDT(STR({variable}), <{}>)", vocab::XSD_DATE_TIME);
    let month = format!("MONTH({date})");
    let year = format!("(YEAR({date}) - IF({month} <= 2, 1, 0))");
    let era = format!("FLOOR({year} / 400.0)");
    let yoe = format!("({year} - {era} * 400)");
    let mp = format!("({month} + IF({month} <= 2, 9, -3))");
    let doy = format!("(FLOOR((153 * {mp} + 2) / 5.0) + DAY({date}) - 1)");
    let days = format!("((((({era} * 146097 + {yoe} * 365) + FLOOR({yoe} / 4.0)) - FLOOR({yoe} / 100.0)) + {doy}) - 719468)");
    format!(
        "({days} * 86400 + HOURS({date}) * 3600 + MINUTES({date}) * 60 + FLOOR(SECONDS({date})))"
    )
}

impl RemoteBackend {
    pub(super) fn search_summary(&self, req: &engine::SearchReq) -> Result<engine::SearchPage> {
        let needle = literal(&req.query.trim().to_lowercase());
        let hidden = req.filter.status.is_none() && !req.all;
        let visible = if req.filter.status.is_some() || req.all {
            String::new()
        } else {
            let mut guard = format!("FILTER NOT EXISTS {{ ?s <{}> \"closed\" }} FILTER NOT EXISTS {{ ?s <{}> \"tombstone\" }}",term::status(),term::status());
            if !req.deferred {
                guard += &format!(
                    " FILTER NOT EXISTS {{ ?s <{}> \"deferred\" }}",
                    term::status()
                );
            }
            guard
        };
        let mut subjects = BTreeSet::new();
        for graph in self.both_graphs() {
            let pattern = self
                .search_branches(&needle, &visible, false, req.full)
                .join(" UNION ");
            subjects.extend(self.subjects_where(&graph, &pattern, None)?);
        }
        let hidden_closed = if hidden {
            let fields = if req.filter == Filter::default() {
                String::new()
            } else {
                self.filter_fields(&req.filter)?
            };
            let branches = self
                .search_branches(&needle, &fields, true, req.full)
                .join(" UNION ");
            let eph = vocab::ephemeral_graph(&self.graph);
            let pattern = format!("{{ GRAPH <{}> {{ {branches} }} FILTER NOT EXISTS {{ GRAPH <{eph}> {{ ?shadow a <{}> ; <{}> ?id }} }} }} UNION {{ GRAPH <{eph}> {{ {branches} }} }}",self.graph,term::work_item(),term::identifier());
            self.aggregate_number(&format!(
                "SELECT (COUNT(DISTINCT ?id) AS ?n) WHERE {{ {pattern} }}"
            ))?
        } else {
            0
        };
        let (compact, has_ephemeral) =
            self.metadata_snapshot(&subjects, &req.filter, req.sort.as_deref(), false)?;
        let mut page = engine::search_matches_page(req, &compact)?;
        page.hidden_closed = hidden_closed;
        let ids = page
            .page
            .issues
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        let snap = self.page_snapshot(&ids, has_ephemeral)?;
        for seed in &mut page.page.issues {
            *seed = snap
                .seeds
                .get(&seed.id)
                .ok_or_else(|| {
                    SdError::failed("a search hit disappeared before its page was read")
                })?
                .clone();
        }
        page.page.dependent_counts = if req.full {
            self.incoming_counts_scoped(&ids, has_ephemeral)?
        } else {
            BTreeMap::new()
        };
        Ok(page)
    }

    fn search_branches(&self, needle: &str, guard: &str, closed: bool, full: bool) -> Vec<String> {
        let fields = if closed {
            format!(
                "?s <{}> \"closed\" ; <{}> ?id .",
                term::status(),
                term::identifier()
            )
        } else {
            String::new()
        };
        let predicates = if full {
            vec![
                term::identifier(),
                term::name(),
                vocab::RDFS_LABEL.into(),
                term::description(),
            ]
        } else {
            vec![term::name(), vocab::RDFS_LABEL.into()]
        };
        let mut branches = predicates.into_iter().map(|p| {
            let fallback = if p == vocab::RDFS_LABEL { format!("FILTER NOT EXISTS {{ ?s <{}> ?name FILTER(isLiteral(?name) && sameTerm(?name, STR(?name))) }}",term::name()) } else { String::new() };
            format!("{{ {{ ?s <{p}> ?text ; a <{}> . {fields} FILTER(isLiteral(?text) && sameTerm(?text, STR(?text)) && CONTAINS(LCASE(STR(?text)), {needle})) }} {guard} {fallback} }}",term::work_item())
        }).collect::<Vec<_>>();
        if full {
            branches.push(format!("{{ ?comment a <{}> ; <{}> ?text ; <{}> ?s . ?s a <{}> . {fields} FILTER(isLiteral(?text) && sameTerm(?text, STR(?text)) && CONTAINS(LCASE(STR(?text)), {needle})) {guard} }}",term::comment(),term::text(),term::comment_on(),term::work_item()));
        }
        branches
    }

    fn metadata_snapshot(
        &self,
        subjects: &BTreeSet<String>,
        filter: &Filter,
        sort: Option<&str>,
        defer_until_present: bool,
    ) -> Result<(Snapshot, bool)> {
        let mut main = Facts::new();
        let mut eph = Facts::new();
        let mut has_ephemeral = false;
        let mut predicates = vec![vocab::RDF_TYPE.into(), term::identifier(), term::status()];
        if defer_until_present {
            predicates.push(term::defer_until());
        }
        let sort = sort.unwrap_or("priority");
        if sort == "priority"
            || filter.priority.is_some()
            || filter.priority_min.is_some()
            || filter.priority_max.is_some()
        {
            predicates.push(term::priority());
        }
        if sort == "priority" || sort == "created" {
            predicates.push(term::created_at());
        }
        if sort == "updated" {
            predicates.push(term::updated_at());
        }
        if sort == "title" || filter.title_contains.is_some() {
            predicates.extend([term::name(), vocab::RDFS_LABEL.into()]);
        }
        if filter.assignee.is_some() || filter.unassigned {
            predicates.push(term::assigned_to());
        }
        if !filter.labels.is_empty() || !filter.labels_any.is_empty() {
            predicates.push(term::label());
        }
        if filter.issue_type.is_some() {
            predicates.push(term::issue_type());
        }
        if filter.parent.is_some() {
            predicates.push(term::child_of());
        }
        for (wanted, p) in [
            (filter.desc_contains.is_some(), term::description()),
            (filter.notes_contains.is_some(), term::notes()),
            (filter.overdue_at.is_some(), term::due_at()),
        ] {
            if wanted {
                predicates.push(p);
            }
        }
        for (graph, facts) in [
            (self.graph.clone(), &mut main),
            (vocab::ephemeral_graph(&self.graph), &mut eph),
        ] {
            if graph != self.graph {
                let body = json!({"query":format!("ASK {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}")})
                    .to_string();
                let answer = self.post("/query", "application/json", &body, false)?;
                has_ephemeral = ask_answer(&graph, &answer)?;
                if !has_ephemeral {
                    continue;
                }
            }
            let ids = subjects.iter().collect::<Vec<_>>();
            // Bound both payload and parser structural cost (default server cap 4096).
            let per_read = (3600 / (predicates.len() * 4 + 3)).max(1);
            for chunk in ids.chunks(per_read) {
                self.read_metadata(&graph, chunk, &predicates, facts)?;
            }
        }
        Ok((Snapshot::from_graphs(&main, &eph), has_ephemeral))
    }

    fn page_snapshot(&self, ids: &[String], has_ephemeral: bool) -> Result<Snapshot> {
        let mut main = Facts::new();
        let mut eph = Facts::new();
        let subjects = ids.iter().map(|id| vocab::item_iri(id)).collect::<Vec<_>>();
        let refs = subjects.iter().collect::<Vec<_>>();
        // Reuse the ephemeral-graph observation from this page's metadata
        // snapshot, never across commands or backend calls.
        for chunk in refs.chunks(13) {
            self.read_subjects(&self.graph, chunk, None, &mut main)?;
            if has_ephemeral {
                self.read_subjects(&vocab::ephemeral_graph(&self.graph), chunk, None, &mut eph)?;
            }
        }
        Ok(Snapshot::from_graphs(&main, &eph))
    }

    pub(super) fn list_summary(&self, req: &engine::ListReq) -> Result<engine::Page> {
        let q = engine::listing_seed_query(req)?;
        // Start from the selective field index alone. Status/default UNION
        // joins can materialize the whole Action class before this restriction.
        // Metadata decoding and list_matches_page validate type and all filters.
        let pattern = if req.defer_until_present {
            format!("?s <{}> ?defer_candidate .", term::defer_until())
        } else {
            vocab::seed_query_pattern(&q).unwrap_or_else(|| format!("?s a <{}>", term::work_item()))
        };
        let mut subjects = BTreeSet::new();
        if req.filter.ids.is_empty() {
            for graph in self.both_graphs() {
                subjects.extend(self.subjects_where(&graph, &pattern, None)?);
            }
        } else {
            subjects.extend(req.filter.ids.iter().map(|id| vocab::item_iri(id)));
        }
        let (compact, has_ephemeral) = self.metadata_snapshot(
            &subjects,
            &req.filter,
            req.sort.as_deref(),
            req.defer_until_present,
        )?;
        let mut page = engine::list_matches_page(req, &compact)?;
        let ids = page.issues.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        let snap = self.page_snapshot(&ids, has_ephemeral)?;
        for seed in &mut page.issues {
            *seed = snap
                .seeds
                .get(&seed.id)
                .ok_or_else(|| {
                    SdError::failed("a listed seed disappeared before its page was read")
                })?
                .clone();
        }
        page.dependent_counts = self.incoming_counts_scoped(&ids, has_ephemeral)?;
        Ok(page)
    }

    pub(super) fn incoming_counts(&self, ids: &[String]) -> Result<BTreeMap<String, usize>> {
        self.incoming_counts_scoped(ids, true)
    }

    fn incoming_counts_scoped(
        &self,
        ids: &[String],
        has_ephemeral: bool,
    ) -> Result<BTreeMap<String, usize>> {
        let mut counts = ids
            .iter()
            .map(|id| (id.clone(), 0))
            .collect::<BTreeMap<_, _>>();
        let edges = [
            term::blocked_on(),
            term::child_of(),
            term::related_to(),
            term::discovered_from(),
        ];
        let eph = vocab::ephemeral_graph(&self.graph);
        let per_read = if has_ephemeral {
            SUBJECTS_PER_READ / (edges.len() * 2)
        } else {
            8
        };
        for chunk in ids.chunks(per_read) {
            let mut branches = Vec::new();
            for id in chunk {
                for graph in self
                    .both_graphs()
                    .into_iter()
                    .take(if has_ephemeral { 2 } else { 1 })
                {
                    for edge in &edges {
                        let prefer = if has_ephemeral && graph == self.graph {
                            format!("FILTER NOT EXISTS {{ GRAPH <{eph}> {{ ?shadow a <{}> ; <{}> ?source_id }} }}",term::work_item(),term::identifier())
                        } else {
                            String::new()
                        };
                        branches.push(format!("{{ GRAPH <{graph}> {{ ?s <{edge}> <{}> ; a <{}> ; <{}> ?source_id }} {prefer} BIND({} AS ?target) BIND(CONCAT(STR(?source_id), {}) AS ?id) }}",vocab::item_iri(id),term::work_item(),term::identifier(),literal(id),literal(&format!("/{edge}"))));
                    }
                }
            }
            for (id, n) in self.grouped(&branches.join(" UNION "), "target")? {
                counts.insert(id, n);
            }
        }
        Ok(counts)
    }

    fn read_metadata(
        &self,
        graph: &str,
        subjects: &[&String],
        predicates: &[String],
        facts: &mut Facts,
    ) -> Result<()> {
        if subjects.is_empty() {
            return Ok(());
        }
        let branches = subjects
            .iter()
            .map(|s| {
                let properties = predicates
                    .iter()
                    .map(|p| {
                        format!("{{ BIND(<{p}> AS ?p) GRAPH <{graph}> {{ <{s}> <{p}> ?o }} }}")
                    })
                    .collect::<Vec<_>>();
                format!(
                    "{{ BIND(<{s}> AS ?s) {{ {} }} }}",
                    properties.join(" UNION ")
                )
            })
            .collect::<Vec<_>>();
        let (rows, truncated) = self.select_marked(
            &format!(
                "SELECT ?s ?p ?o WHERE {{ {} }} LIMIT {ROW_CAP}",
                branches.join(" UNION ")
            ),
            None,
        )?;
        if truncated || rows.len() >= ROW_CAP {
            if subjects.len() == 1 {
                return Err(SdError::failed("quipu truncated one seed's metadata"));
            }
            let (a, b) = subjects.split_at(subjects.len() / 2);
            self.read_metadata(graph, a, predicates, facts)?;
            return self.read_metadata(graph, b, predicates, facts);
        }
        for mut row in rows {
            if let (Some(Obj::Iri(s)), Some(Obj::Iri(p)), Some(o)) =
                (row.remove("s"), row.remove("p"), row.remove("o"))
            {
                facts.entry(s).or_default().push((p, o));
            }
        }
        Ok(())
    }

    // Ephemeral seeds replace project seeds of the same id, as Snapshot does.
    fn effective_seeds(&self, fields: &str) -> String {
        let eph = vocab::ephemeral_graph(&self.graph);
        let base = format!(
            "?s a <{}> ; <{}> ?id .",
            term::work_item(),
            term::identifier()
        );
        format!("{{ GRAPH <{}> {{ {base} {fields} FILTER(isLiteral(?id) && sameTerm(?id, STR(?id))) }} FILTER NOT EXISTS {{ GRAPH <{eph}> {{ ?shadow a <{}> ; <{}> ?id }} }} }} UNION {{ GRAPH <{eph}> {{ {base} {fields} FILTER(isLiteral(?id) && sameTerm(?id, STR(?id))) }} }}", self.graph, term::work_item(), term::identifier())
    }

    fn status_fields(&self) -> String {
        format!(
            "?s <{}> ?status . FILTER(isLiteral(?status) && sameTerm(?status, STR(?status)))",
            term::status()
        )
    }

    fn effective_normalized(&self, fields: &str) -> String {
        let regular = self.effective_seeds(fields);
        let missing = format!("FILTER NOT EXISTS {{ ?s <{}> ?raw_status FILTER(isLiteral(?raw_status) && sameTerm(?raw_status, STR(?raw_status))) }} BIND(\"open\" AS ?status)",term::status());
        let fallback = self.effective_seeds(&fields.replace(&self.status_fields(), &missing));
        format!("{{ {regular} }} UNION {{ {fallback} }}")
    }

    fn grouped(&self, pattern: &str, key: &str) -> Result<Vec<(String, usize)>> {
        let (rows, truncated) = self.select_marked(&format!("SELECT ?{key} (COUNT(DISTINCT ?id) AS ?n) WHERE {{ {pattern} }} GROUP BY ?{key} LIMIT {ROW_CAP}"), None)?;
        if truncated || rows.len() >= ROW_CAP {
            return Err(SdError::failed(
                "quipu truncated grouped counts; refusing an incomplete summary",
            ));
        }
        let mut groups = BTreeMap::new();
        for row in rows {
            let value = match row.get(key) {
                Some(Obj::Str(s)) => s.clone(),
                Some(Obj::Iri(s)) => vocab::principal_name(s).unwrap_or_else(|| s.clone()),
                Some(Obj::Int(n)) => n.to_string(),
                _ => return Err(SdError::failed("quipu returned an invalid group key")),
            };
            let n = match row.get("n") {
                Some(Obj::Int(n)) => usize::try_from(*n).ok(),
                Some(Obj::Str(n)) => n.parse().ok(),
                _ => None,
            }
            .ok_or_else(|| SdError::failed("quipu returned an invalid grouped count"))?;
            *groups.entry(value).or_default() += n;
        }
        Ok(groups.into_iter().collect())
    }

    fn filter_fields(&self, f: &Filter) -> Result<String> {
        let mut fields = self.status_fields();
        if let Some(status) = &f.status {
            fields += &format!(
                " FILTER(?status = {})",
                literal(&crate::model::parse_status(status)?)
            );
        }
        if let Some(kind) = &f.issue_type {
            fields += &string_field(&term::issue_type(), "kind", "task");
            fields += &format!(
                " FILTER(?kind = {})",
                literal(&crate::model::parse_type(kind)?)
            );
        }
        if f.assignee.is_some() || f.unassigned {
            fields += &format!(" OPTIONAL {{ ?s <{}> ?assignee }}", term::assigned_to());
            if let Some(person) = &f.assignee {
                fields += &format!(
                    " FILTER(?assignee = <{}> || ?assignee = {})",
                    vocab::principal_iri(person),
                    literal(person)
                );
            }
            if f.unassigned {
                fields += " FILTER(!BOUND(?assignee))";
            }
        }
        for label in f.labels.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
            fields += &format!(" ?s <{}> {} .", term::label(), literal(label.trim()));
        }
        let labels_any = f
            .labels_any
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if !labels_any.is_empty() {
            fields += &format!(
                " ?s <{}> ?label_any . FILTER(?label_any IN ({}))",
                term::label(),
                labels_any
                    .iter()
                    .map(|s| literal(s.trim()))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if f.priority.is_some() || f.priority_min.is_some() || f.priority_max.is_some() {
            fields += &format!(" OPTIONAL {{ ?s <{}> ?raw_priority . FILTER(isNumeric(?raw_priority) && ?raw_priority >= 0 && ?raw_priority <= 255) }} BIND(COALESCE(?raw_priority, 2) AS ?priority)", term::priority());
            for (value, operator) in [
                (&f.priority, "="),
                (&f.priority_min, ">="),
                (&f.priority_max, "<="),
            ] {
                if let Some(value) = value {
                    fields += &format!(
                        " FILTER(?priority {operator} {})",
                        crate::model::parse_priority(value)?
                    );
                }
            }
        }
        if let Some(parent) = &f.parent {
            fields += &format!(" ?s <{}> <{}> .", term::child_of(), vocab::item_iri(parent));
        }
        if !f.ids.is_empty() {
            fields += &format!(
                " FILTER(?id IN ({}))",
                f.ids
                    .iter()
                    .map(|s| literal(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for (text, predicate, key) in [
            (&f.title_contains, term::name(), "title"),
            (&f.desc_contains, term::description(), "description"),
            (&f.notes_contains, term::notes(), "notes"),
        ] {
            if let Some(text) = text {
                fields += &format!(
                    " ?s <{predicate}> ?{key} . FILTER(CONTAINS(LCASE(STR(?{key})), {}))",
                    literal(&text.to_lowercase())
                );
            }
        }
        if let Some(now) = &f.overdue_at {
            fields += &format!(" ?s <{}> ?due . FILTER(STR(?due) < {} && ?status != \"closed\" && ?status != \"tombstone\")", term::due_at(), literal(now));
        }
        Ok(fields)
    }

    fn group_field(by: &str) -> Result<String> {
        Ok(match by {
            "status" => String::new(),
            "type" => string_field(&term::issue_type(), "key", "task"),
            "priority" => format!("OPTIONAL {{ ?s <{}> ?raw_key . FILTER(isNumeric(?raw_key) && ?raw_key >= 0 && ?raw_key <= 255) }} BIND(CONCAT(\"P\", STR(COALESCE(?raw_key, 2))) AS ?key)", term::priority()),
            "assignee" => format!("OPTIONAL {{ ?s <{}> ?raw_key }} BIND(COALESCE(?raw_key, \"(unassigned)\") AS ?key)", term::assigned_to()),
            "label" => string_field(&term::label(), "key", "(no labels)"),
            _ => return Err(SdError::usage("unknown --by; expected status, priority, type, assignee or label")),
        })
    }

    pub(super) fn count_summary(&self, req: &CountReq) -> Result<Count> {
        let mut fields = self.filter_fields(&req.filter)?;
        if req.filter.status.is_none() {
            fields += " FILTER(?status != \"tombstone\")";
            if !req.include_closed {
                fields += " FILTER(?status != \"closed\")";
            }
        }
        let total = self.aggregate_number(&format!(
            "SELECT (COUNT(DISTINCT ?id) AS ?n) WHERE {{ {} }}",
            self.effective_normalized(&fields)
        ))?;
        let groups = req
            .by
            .as_deref()
            .map(|by| {
                let key = if by == "status" { "status" } else { "key" };
                self.grouped(
                    &self.effective_normalized(&(fields.clone() + &Self::group_field(by)?)),
                    key,
                )
            })
            .transpose()?;
        Ok(Count { total, groups })
    }

    pub(super) fn stats_summary(&self, ctx: &Ctx, req: StatsReq) -> Result<Stats> {
        let statuses = self.grouped(&self.effective_normalized(&self.status_fields()), "status")?;
        let count = |status: &str| {
            statuses
                .iter()
                .find(|(key, _)| key == status)
                .map_or(0, |(_, n)| *n)
        };
        let mut stats = Stats {
            total: statuses
                .iter()
                .filter(|(key, _)| key != crate::model::TOMBSTONE)
                .map(|(_, n)| n)
                .sum(),
            open: count("open"),
            in_progress: count("in_progress"),
            closed: count("closed"),
            blocked: count("blocked"),
            deferred: count("deferred"),
            tombstones: count(crate::model::TOMBSTONE),
            ..Stats::default()
        };
        let active = self.status_fields() + " FILTER(?status != \"tombstone\")";
        for (wanted, by) in [
            (req.by_type, "type"),
            (req.by_priority, "priority"),
            (req.by_assignee, "assignee"),
            (req.by_label, "label"),
        ] {
            if wanted {
                stats.breakdowns.push((
                    by,
                    self.grouped(
                        &self.effective_normalized(&(active.clone() + &Self::group_field(by)?)),
                        "key",
                    )?,
                ));
            }
        }
        // Compute the mean on the server, without transferring closed-seed
        // facts. Match the core's whole-second Gregorian calendar arithmetic.
        let duration = format!(
            "(({}) - ({})) / 3600.0",
            epoch_expression("?closed"),
            epoch_expression("?created")
        );
        let pattern = format!(
            "?s <{}> \"closed\" ; a <{}> ; <{}> ?id ; <{}> ?created ; <{}> ?closed .",
            term::status(),
            term::work_item(),
            term::identifier(),
            term::created_at(),
            term::closed_at()
        );
        let eph = vocab::ephemeral_graph(&self.graph);
        let graphs = format!("{{ GRAPH <{}> {{ {pattern} }} FILTER NOT EXISTS {{ GRAPH <{eph}> {{ ?shadow a <{}> ; <{}> ?id }} }} }} UNION {{ GRAPH <{eph}> {{ {pattern} }} }}",self.graph,term::work_item(),term::identifier());
        let (rows, truncated) = self.select_marked(
            &format!(
                "SELECT (AVG({duration}) AS ?mean) (COUNT({duration}) AS ?n) WHERE {{ {graphs} }}"
            ),
            None,
        )?;
        if truncated || rows.len() != 1 {
            return Err(SdError::failed(
                "quipu returned an incomplete lead-time aggregate",
            ));
        }
        let n = rows[0]
            .get("n")
            .and_then(|v| match v {
                Obj::Int(n) => usize::try_from(*n).ok(),
                Obj::Str(n) => n.parse().ok(),
                _ => None,
            })
            .ok_or_else(|| SdError::failed("quipu returned an invalid lead-time count"))?;
        stats.average_lead_time_hours = if n == 0 {
            0.0
        } else {
            rows[0]
                .get("mean")
                .and_then(|v| match v {
                    Obj::Int(n) => Some(*n as f64),
                    Obj::Str(s) | Obj::Typed { lexical: s, .. } => s.parse::<f64>().ok(),
                    _ => None,
                })
                .filter(|n| n.is_finite())
                .ok_or_else(|| SdError::failed("quipu returned an invalid lead-time mean"))?
        };
        let mut ready = vocab::ready_query(&self.graph)
            .replace("SELECT ?id", "SELECT (COUNT(DISTINCT ?id) AS ?n)");
        let tail = "OPTIONAL { ?item <".to_string()
            + &term::defer_until()
            + "> ?defer } FILTER(!BOUND(?defer) || STR(?defer) <= "
            + &literal(&ctx.now)
            + ")";
        let position = ready.rfind("}} ").or_else(|| ready.rfind("}}"));
        // The public query ends with the GRAPH and WHERE braces on separate lines.
        let position = position.or_else(|| ready.rfind("} }"));
        if let Some(position) = position {
            ready.insert_str(position, &tail);
        } else {
            return Err(SdError::failed("cannot construct the ready aggregate"));
        }
        stats.ready = self.aggregate_number(&ready)?;
        let fields = format!("?s <{}> \"epic\" . {active} FILTER(?status != \"closed\") FILTER EXISTS {{ ?child <{}> ?s ; <{}> \"closed\" . ?s a <{}> }} FILTER NOT EXISTS {{ ?child <{}> ?s . FILTER NOT EXISTS {{ ?child <{}> \"closed\" }} FILTER NOT EXISTS {{ ?child <{}> \"tombstone\" }} }}", term::issue_type(), term::child_of(), term::status(), term::work_item(),term::child_of(), term::status(),term::status());
        stats.epics_eligible_for_closure = self.aggregate_number(&format!(
            "SELECT (COUNT(DISTINCT ?id) AS ?n) WHERE {{ {} }}",
            self.effective_normalized(&fields)
        ))?;
        if let Some(hours) = req.activity_hours {
            let since = engine::parse_since(&format!("{hours}h"), &ctx.now)?;
            let time_count = |predicate: String, extra: &str| {
                let fields = format!(
                    "?s <{predicate}> ?time . {active} FILTER(STR(?time) >= {}) {extra}",
                    literal(&since)
                );
                self.aggregate_number(&format!(
                    "SELECT (COUNT(DISTINCT ?id) AS ?n) WHERE {{ {} }}",
                    self.effective_normalized(&fields)
                ))
            };
            let updated_extra = format!("FILTER NOT EXISTS {{ ?s <{}> ?created FILTER(STR(?created) >= {}) }} FILTER NOT EXISTS {{ ?s <{}> ?closed FILTER(STR(?closed) >= {}) }}", term::created_at(), literal(&since), term::closed_at(), literal(&since));
            stats.activity = Some(Activity {
                hours,
                created: time_count(term::created_at(), "")?,
                closed: time_count(term::closed_at(), "")?,
                updated: time_count(term::updated_at(), &updated_extra)?,
                touched: time_count(term::updated_at(), "")?,
            });
        }
        Ok(stats)
    }
}
