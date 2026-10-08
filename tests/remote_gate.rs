//! Producer facts, rather than hand-written fixtures, prove admission parity.
#![cfg(all(feature = "shacl", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used)]

use seeds::backend::{write_record, Backend, Claims, Ctx, SeedWrite, WriteBatch};
use seeds::engine::{self, CreateReq};
use seeds::quipu_backend::QuipuBackend;
use seeds::validate::push_ntriples;

#[test]
fn remote_gate_accepts_producer_and_rejects_missing_or_invalid_fields() {
    let shapes = include_str!("../shapes/quipu-gate.shapes.ttl");
    let ctx = Ctx {
        now: "2026-10-08T00:00:00Z".into(),
        actor: "tester".into(),
        prefix: "sd".into(),
        claims: Claims {
            agent_name: Some("tester".into()),
            ..Default::default()
        },
    };
    let mut backend = QuipuBackend::in_memory("urn:test:remote-gate").unwrap();
    let seed = engine::create(
        &mut backend,
        &ctx,
        &CreateReq {
            title: "gate probe".into(),
            ..Default::default()
        },
    )
    .unwrap()
    .0;
    engine::comment_add(&mut backend, &ctx, &seed.id, "comment probe", None).unwrap();
    let snapshot = backend.snapshot(None).unwrap();
    let mut batch = WriteBatch::default();
    batch.seeds.push(SeedWrite {
        seed: seed.clone(),
        expected_revision: None,
    });
    let mut records = write_record("urn:test:write", &batch, &ctx);
    records.push(("urn:test:seed".into(), seed.facts()));
    for (index, comment) in snapshot.comments.iter().enumerate() {
        records.push((format!("urn:test:comment:{index}"), comment.facts()));
    }
    let render = |records: &Vec<(String, Vec<seeds::model::Fact>)>| {
        let mut data = String::new();
        for (subject, facts) in records {
            push_ntriples(&mut data, subject, facts);
        }
        data
    };
    assert!(
        quipu::validate_shapes(shapes, &render(&records))
            .unwrap()
            .conforms
    );
    for predicate in [
        "https://seeds.local/ontology/actor",
        "https://schema.org/identifier",
        "https://schema.org/text",
        "http://aegis.gastown.local/ontology/sourceKind",
    ] {
        let mut missing = records.clone();
        for (_, facts) in &mut missing {
            facts.retain(|(p, _)| p != predicate);
        }
        assert!(
            !quipu::validate_shapes(shapes, &render(&missing))
                .unwrap()
                .conforms,
            "{predicate}"
        );
    }
    let mut invalid = records.clone();
    for (_, facts) in &mut invalid {
        for (p, o) in facts {
            if p == "https://seeds.local/ontology/priority" {
                *o = seeds::model::Obj::Int(5);
            }
        }
    }
    assert!(
        !quipu::validate_shapes(shapes, &render(&invalid))
            .unwrap()
            .conforms
    );
}
