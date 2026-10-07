//! The vocabulary a seed is written in.
//!
//! schema.org first, then other public W3C vocabularies, then Quechua for the
//! governance terms camayoc's gate reads, and seeds' own `seeds:` namespace
//! only for tracker mechanics no public vocabulary names (status, priority,
//! type, the body sections). The mapping is ruled in aegis-bqgdr3 (table
//! v1.1); see `docs/book/src/storage.md`. Item, comment, principal and graph
//! IRIs are seeds' own and did not change with the vocabulary.

/// camayoc's vocabulary namespace. seeds' items no longer use it; the
/// provenance side graph's attribution claims still carry its `sourceKind`.
pub const AEGIS: &str = "http://aegis.gastown.local/ontology/";
/// seeds' own vocabulary namespace.
pub const SEEDS: &str = "https://seeds.local/ontology/";
/// schema.org.
pub const SCHEMA: &str = "https://schema.org/";
/// Quechua, the governance vocabulary (`sourceKind`, `outcome`, `blockedOn`).
pub const QUECHUA: &str = "https://scbrown.github.io/quechua/ns#";
/// W3C RDF Calendar (`ical:due`).
pub const ICAL: &str = "http://www.w3.org/2002/12/cal/ical#";
/// Dublin Core terms (`dcterms:relation`).
pub const DCTERMS: &str = "http://purl.org/dc/terms/";
/// W3C PROV-O (`prov:wasDerivedFrom`).
pub const PROV: &str = "http://www.w3.org/ns/prov#";
/// Where seed, comment and principal IRIs live.
pub const SEEDS_BASE: &str = "https://seeds.local/";

/// `rdf:type`.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// `rdfs:label`: a seed's title, written beside `schema:name` with the same
/// value (quipu's label floor and `/search` read it; the shapes hold the two
/// equal).
pub const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
/// `xsd:date`: a due or defer value written as a bare `YYYY-MM-DD`.
pub const XSD_DATE: &str = "http://www.w3.org/2001/XMLSchema#date";
/// `xsd:dateTime`: every instant a seed or comment carries.
pub const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
/// `xsd:duration`: the time estimate, in canonical form (`PT1H30M`).
pub const XSD_DURATION: &str = "http://www.w3.org/2001/XMLSchema#duration";

/// The shapes every native write is validated against.
pub const SHAPES_TURTLE: &str = include_str!("../shapes/seeds.shapes.ttl");

/// A term in camayoc's namespace.
pub fn aegis(local: &str) -> String {
    format!("{AEGIS}{local}")
}

/// A term in the seeds namespace.
pub fn seeds(local: &str) -> String {
    format!("{SEEDS}{local}")
}

/// A term in schema.org.
pub fn schema(local: &str) -> String {
    format!("{SCHEMA}{local}")
}

/// A term in the Quechua governance namespace.
pub fn quechua(local: &str) -> String {
    format!("{QUECHUA}{local}")
}

/// Predicates and classes, by role. Kept as functions so the IRIs are built in
/// one place.
pub mod term {
    use super::{quechua, schema, seeds, DCTERMS, ICAL, PROV};

    /// `schema:Action`, the class of a seed (a unit of intended work).
    pub fn work_item() -> String {
        schema("Action")
    }
    /// `quechua:sourceKind`; seeds writes `declared` (an agent or person said
    /// so).
    pub fn source_kind() -> String {
        quechua("sourceKind")
    }
    /// `schema:identifier`: the seed id, e.g. `sd-a3f`.
    pub fn identifier() -> String {
        schema("identifier")
    }
    /// `schema:name`: the title. Written with `rdfs:label` beside it, same
    /// value.
    pub fn name() -> String {
        schema("name")
    }
    /// `schema:dateCreated`, an `xsd:dateTime` (on seeds and comments).
    pub fn created_at() -> String {
        schema("dateCreated")
    }
    /// `schema:endTime`: when the seed was closed, an `xsd:dateTime`.
    pub fn closed_at() -> String {
        schema("endTime")
    }
    /// `quechua:outcome`; `done` on close, absent while open. camayoc's
    /// governed close classification, kept apart from the free-text
    /// `schema:result`.
    pub fn outcome() -> String {
        quechua("outcome")
    }
    /// `schema:agent`, pointing at a principal IRI: the assignee.
    pub fn assigned_to() -> String {
        schema("agent")
    }
    /// `quechua:blockedOn`: the `blocks` dependency.
    pub fn blocked_on() -> String {
        quechua("blockedOn")
    }
    /// `seeds:status`: the precise tracker status, one of seven. The field the
    /// claim compare-and-set reads.
    pub fn status() -> String {
        seeds("status")
    }
    /// `schema:actionStatus`: the coarse public status, DERIVED from status
    /// and outcome in the same write (see `crate::model::action_status`).
    pub fn action_status() -> String {
        schema("actionStatus")
    }
    /// `seeds:priority`, 0..=4.
    pub fn priority() -> String {
        seeds("priority")
    }
    /// `seeds:issueType`.
    pub fn issue_type() -> String {
        seeds("issueType")
    }
    /// `schema:description`.
    pub fn description() -> String {
        schema("description")
    }
    /// `seeds:notes`.
    pub fn notes() -> String {
        seeds("notes")
    }
    /// `seeds:design`: design notes (br's `design`).
    pub fn design() -> String {
        seeds("design")
    }
    /// `seeds:agentContext`: governing instructions for an agent working the
    /// seed (br's `agent_context`), a compact JSON text.
    pub fn agent_context() -> String {
        seeds("agentContext")
    }
    /// `seeds:acceptanceCriteria`: br's `acceptance_criteria`, often a
    /// `- [ ]` checklist.
    pub fn acceptance_criteria() -> String {
        seeds("acceptanceCriteria")
    }
    /// `seeds:externalRef`: a reference to the same work elsewhere (br's
    /// `external_ref`), a plain string. Not `schema:sameAs`/`url`: some
    /// values are not URLs.
    pub fn external_ref() -> String {
        seeds("externalRef")
    }
    /// `ical:due`: when the work is due (br's `due_at`), an `xsd:date` or an
    /// `xsd:dateTime`.
    pub fn due_at() -> String {
        format!("{ICAL}due")
    }
    /// `schema:timeRequired`: a time estimate (br's `estimated_minutes`), an
    /// `xsd:duration` of that many minutes in canonical form (`PT1H30M`).
    pub fn estimated_minutes() -> String {
        schema("timeRequired")
    }

    /// `schema:accountablePerson`: who owns the work (br's `owner`), a
    /// principal IRI.
    pub fn owner() -> String {
        schema("accountablePerson")
    }
    /// `schema:keywords`, one fact per label.
    pub fn label() -> String {
        schema("keywords")
    }
    /// `schema:dateModified`, an `xsd:dateTime`.
    pub fn updated_at() -> String {
        schema("dateModified")
    }
    /// `schema:creator`, a principal IRI.
    pub fn created_by() -> String {
        schema("creator")
    }
    /// `schema:result`: the free-text close reason.
    pub fn close_reason() -> String {
        schema("result")
    }
    /// `schema:scheduledTime`: hidden from ready until then, an `xsd:date`
    /// or an `xsd:dateTime`.
    pub fn defer_until() -> String {
        schema("scheduledTime")
    }
    /// `schema:version`, the compare-and-set token.
    pub fn revision() -> String {
        schema("version")
    }
    /// `dcterms:relation`: a `related` dependency.
    pub fn related_to() -> String {
        format!("{DCTERMS}relation")
    }
    /// `schema:isPartOf`: a `parent-child` dependency (this seed is the
    /// child).
    pub fn child_of() -> String {
        schema("isPartOf")
    }
    /// `prov:wasDerivedFrom`: a `discovered-from` dependency.
    pub fn discovered_from() -> String {
        format!("{PROV}wasDerivedFrom")
    }
    /// `seeds:workflowRun`: the shuttle run (`urn:shuttle:run:<id>`) that
    /// created or drives the seed. Neither camayoc nor shuttle has a term
    /// linking a work item to a WorkflowRun yet; this is seeds' stopgap and a
    /// proposal for camayoc, not a parallel vocabulary for runs themselves
    /// (runs, definitions and transitions stay in shuttle's terms).
    pub fn workflow_run() -> String {
        seeds("workflowRun")
    }
    /// `schema:Comment`.
    pub fn comment() -> String {
        schema("Comment")
    }
    /// `schema:parentItem`: the seed a comment belongs to.
    pub fn comment_on() -> String {
        schema("parentItem")
    }
    /// `schema:position`: 1-based position among the seed's comments.
    pub fn comment_index() -> String {
        schema("position")
    }
    /// `schema:author`, a principal IRI.
    pub fn author() -> String {
        schema("author")
    }
    /// `schema:text`.
    pub fn text() -> String {
        schema("text")
    }
}

/// The IRI of a seed.
pub fn item_iri(id: &str) -> String {
    format!("{SEEDS_BASE}item/{}", encode(id))
}

/// The seed id an item IRI names, if it is one.
pub fn item_id(iri: &str) -> Option<String> {
    iri.strip_prefix(&format!("{SEEDS_BASE}item/"))
        .filter(|rest| !rest.contains('/'))
        .map(decode)
}

/// The IRI of a seed's `index`th comment.
pub fn comment_iri(id: &str, index: u64) -> String {
    format!("{SEEDS_BASE}item/{}/comment/{index}", encode(id))
}

/// The shuttle namespace for run IRIs (shuttle's default `SHUTTLE_ENTITY_NS`).
pub const SHUTTLE_RUN_PREFIX: &str = "urn:shuttle:run:";

/// A shuttle run reference as an IRI: a full IRI is kept, a bare run id
/// becomes `urn:shuttle:run:<id>`.
pub fn run_iri(run: &str) -> String {
    let r = run.trim();
    if r.contains(':') {
        r.to_string()
    } else {
        format!("{SHUTTLE_RUN_PREFIX}{r}")
    }
}

/// The IRI of a principal (an assignee).
pub fn principal_iri(name: &str) -> String {
    format!("{SEEDS_BASE}principal/{}", encode(name))
}

/// The principal name a principal IRI names, if it is one.
pub fn principal_name(iri: &str) -> Option<String> {
    iri.strip_prefix(&format!("{SEEDS_BASE}principal/"))
        .map(decode)
}

/// The default named graph (project) IRI for an id prefix.
pub fn project_graph_iri(prefix: &str) -> String {
    format!("{SEEDS_BASE}project/{}", encode(prefix))
}

/// The ready definition, as SPARQL over one project graph: open seeds with no
/// `blockedOn` target whose status is anything but `closed`. This is the
/// query camayoc is meant to carry as a stored query; `sd ready` runs it
/// verbatim and then applies the caller's filters and the defer date.
pub fn ready_query(graph_iri: &str) -> String {
    format!(
        "PREFIX schema: <{SCHEMA}>\n\
         PREFIX quechua: <{QUECHUA}>\n\
         PREFIX seeds: <{SEEDS}>\n\
         SELECT ?id WHERE {{ GRAPH <{graph_iri}> {{\n\
         \x20 ?item a schema:Action ; schema:identifier ?id ; seeds:status \"open\" .\n\
         \x20 FILTER NOT EXISTS {{ ?item quechua:blockedOn ?blocker . ?blocker seeds:status ?bs . FILTER(?bs != \"closed\" && ?bs != \"tombstone\") }}\n\
         }} }}"
    )
}

/// Percent-encode anything outside the unreserved set, so an id or a name
/// can sit in an IRI path segment and come back out unchanged.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Inverse of [`encode`].
pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The side graph holding a project's ephemeral seeds (br's `--ephemeral`):
/// read by every snapshot, never shared, so a pendant or ledger carries only
/// the project graph. A seed stays in the graph it was created in.
pub fn ephemeral_graph(graph: &str) -> String {
    if graph.contains('#') {
        format!("{graph}-seeds-ephemeral")
    } else {
        format!("{graph}#seeds-ephemeral")
    }
}

/// The side graph holding who wrote what (`seeds:Write` records and their
/// attribution claims). Snapshots and exports never read it.
pub fn provenance_graph(graph: &str) -> String {
    if graph.contains('#') {
        format!("{graph}-seeds-provenance")
    } else {
        format!("{graph}#seeds-provenance")
    }
}

/// The attribution claims on writes to one seed, from its project's
/// provenance graph: each `?version` (`<id>@<revision>`) a claiming write
/// produced, and whichever of `?agent ?harness ?model ?session` it made. The caller
/// keeps only versions of this seed (one write can produce several).
pub fn claims_query(graph_iri: &str, id: &str) -> String {
    format!(
        "PREFIX seeds: <{SEEDS}>\n\
         SELECT ?version ?agent ?harness ?model ?session WHERE {{ GRAPH <{pg}> {{\n\
         \x20 ?w seeds:wrote <{item}> ; seeds:version ?version ; seeds:claimed ?c .\n\
         \x20 OPTIONAL {{ ?c seeds:agentName ?agent }}\n\
         \x20 OPTIONAL {{ ?c seeds:harness ?harness }}\n\
         \x20 OPTIONAL {{ ?c seeds:model ?model }}\n\
         \x20 OPTIONAL {{ ?c seeds:session ?session }}\n\
         }} }}",
        pg = provenance_graph(graph_iri),
        item = item_iri(id),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iris_round_trip() {
        for id in ["sd-a3f", "sd-a3f.1", "odd id/with%stuff"] {
            assert_eq!(item_id(&item_iri(id)).as_deref(), Some(id));
        }
        assert_eq!(
            principal_name(&principal_iri("ian malcolm")).as_deref(),
            Some("ian malcolm")
        );
        assert_eq!(item_id(&comment_iri("sd-a", 1)), None);
    }
}
