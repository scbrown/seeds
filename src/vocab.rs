//! The vocabulary a seed is written in.
//!
//! Where camayoc already names a thing, seeds uses camayoc's term (camayoc
//! publishes its vocabulary in the `aegis:` namespace below; that string is an
//! RDF namespace, not a host). Where a tracker needs something camayoc does not
//! model (status, priority, labels, the compare-and-set revision), seeds mints
//! it in its own `seeds:` namespace. See `docs/book/src/storage.md`.

/// camayoc's vocabulary namespace (`ontology/core.ttl` in scbrown/camayoc).
pub const AEGIS: &str = "http://aegis.gastown.local/ontology/";
/// seeds' own vocabulary namespace.
pub const SEEDS: &str = "https://seeds.local/ontology/";
/// Where seed, comment and principal IRIs live.
pub const SEEDS_BASE: &str = "https://seeds.local/";

/// `rdf:type`.
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// `rdfs:label`: a seed's title.
pub const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

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

/// Predicates and classes, by role. Kept as functions so the IRIs are built in
/// one place.
pub mod term {
    use super::{aegis, seeds};

    /// `aegis:WorkItem`, camayoc's class for a unit of intended work.
    pub fn work_item() -> String {
        aegis("WorkItem")
    }
    /// `aegis:sourceKind`; seeds writes `declared` (an agent or person said so).
    pub fn source_kind() -> String {
        aegis("sourceKind")
    }
    /// `aegis:identifier`: the seed id, e.g. `sd-a3f`.
    pub fn identifier() -> String {
        aegis("identifier")
    }
    /// `aegis:createdAt`.
    pub fn created_at() -> String {
        aegis("createdAt")
    }
    /// `aegis:closedAt`.
    pub fn closed_at() -> String {
        aegis("closedAt")
    }
    /// `aegis:outcome`; `done` on close, absent while open.
    pub fn outcome() -> String {
        aegis("outcome")
    }
    /// `aegis:assignedTo`, pointing at a principal IRI.
    pub fn assigned_to() -> String {
        aegis("assignedTo")
    }
    /// `aegis:blockedOn`: the `blocks` dependency.
    pub fn blocked_on() -> String {
        aegis("blockedOn")
    }
    /// `seeds:status`.
    pub fn status() -> String {
        seeds("status")
    }
    /// `seeds:priority`, 0..=4.
    pub fn priority() -> String {
        seeds("priority")
    }
    /// `seeds:issueType`.
    pub fn issue_type() -> String {
        seeds("issueType")
    }
    /// `seeds:description`.
    pub fn description() -> String {
        seeds("description")
    }
    /// `seeds:notes`.
    pub fn notes() -> String {
        seeds("notes")
    }
    /// `seeds:label`, one fact per label.
    pub fn label() -> String {
        seeds("label")
    }
    /// `seeds:updatedAt`.
    pub fn updated_at() -> String {
        seeds("updatedAt")
    }
    /// `seeds:createdBy`.
    pub fn created_by() -> String {
        seeds("createdBy")
    }
    /// `seeds:closeReason`.
    pub fn close_reason() -> String {
        seeds("closeReason")
    }
    /// `seeds:deferUntil`.
    pub fn defer_until() -> String {
        seeds("deferUntil")
    }
    /// `seeds:revision`, the compare-and-set token.
    pub fn revision() -> String {
        seeds("revision")
    }
    /// `seeds:relatedTo`: a `related` dependency.
    pub fn related_to() -> String {
        seeds("relatedTo")
    }
    /// `seeds:childOf`: a `parent-child` dependency (this seed is the child).
    pub fn child_of() -> String {
        seeds("childOf")
    }
    /// `seeds:discoveredFrom`: a `discovered-from` dependency.
    pub fn discovered_from() -> String {
        seeds("discoveredFrom")
    }
    /// `seeds:workflowRun`: the shuttle run (`urn:shuttle:run:<id>`) that
    /// created or drives the seed. Neither camayoc nor shuttle has a term
    /// linking a WorkItem to a WorkflowRun yet; this is seeds' stopgap and a
    /// proposal for camayoc, not a parallel vocabulary for runs themselves
    /// (runs, definitions and transitions stay in shuttle's `aegis:` terms).
    pub fn workflow_run() -> String {
        seeds("workflowRun")
    }
    /// `seeds:Comment`.
    pub fn comment() -> String {
        seeds("Comment")
    }
    /// `seeds:commentOn`: the seed a comment belongs to.
    pub fn comment_on() -> String {
        seeds("commentOn")
    }
    /// `seeds:commentIndex`: 1-based position among the seed's comments.
    pub fn comment_index() -> String {
        seeds("commentIndex")
    }
    /// `seeds:author`.
    pub fn author() -> String {
        seeds("author")
    }
    /// `seeds:text`.
    pub fn text() -> String {
        seeds("text")
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
        "PREFIX aegis: <{AEGIS}>\n\
         PREFIX seeds: <{SEEDS}>\n\
         SELECT ?id WHERE {{ GRAPH <{graph_iri}> {{\n\
         \x20 ?item a aegis:WorkItem ; aegis:identifier ?id ; seeds:status \"open\" .\n\
         \x20 FILTER NOT EXISTS {{ ?item aegis:blockedOn ?blocker . ?blocker seeds:status ?bs . FILTER(?bs != \"closed\") }}\n\
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
