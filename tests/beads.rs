//! Round-trip and conflict evidence for the JSONL bridge.
use seeds::{
    backend::{Backend, Ctx},
    beads::{self, Records},
    model::Snapshot,
    quipu_backend::QuipuBackend,
    sync,
};
use serde_json::json;
fn records() -> Records {
    beads::parse(&json!({"id":"br-a","title":"original","status":"hooked","priority":1,"issue_type":"decision","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","owner":null,"labels":["z","a"],"opaque":{"nested":[1,null,"雪"]},"comments":[{"id":903,"issue_id":"br-a","author":"writer","text":"hello","created_at":"2026-01-01T00:00:00Z","future":true}],"dependencies":[{"issue_id":"br-a","depends_on_id":"br-a","type":"relates-to","metadata":"{}","created_by":"writer"}]}).to_string()).unwrap()
}
fn context() -> Ctx {
    Ctx {
        now: "2026-02-01T00:00:00Z".into(),
        actor: "bridge".into(),
        prefix: "sd".into(),
        claims: Default::default(),
    }
}
#[test]
fn full_fidelity_through_real_store_and_edited_projection() {
    let original = records();
    let snap = beads::decode(&original).unwrap();
    let mut b = QuipuBackend::in_memory("https://seeds.local/project/test").unwrap();
    let (batch, _) = sync::plan(&Snapshot::default(), &snap, "beads-sync");
    b.commit(&batch, &context()).unwrap();
    let mut loaded = b.snapshot(None).unwrap();
    assert_eq!(beads::encode(&loaded).unwrap(), original);
    let s = loaded.seeds.get_mut("br-a").unwrap();
    s.title = "changed".into();
    s.related.clear();
    s.labels.insert("new".into());
    let changed = beads::encode(&loaded).unwrap();
    assert_eq!(changed["br-a"]["title"], "changed");
    assert_eq!(changed["br-a"]["opaque"], original["br-a"]["opaque"]);
    assert_eq!(changed["br-a"]["comments"], original["br-a"]["comments"]);
    assert_eq!(changed["br-a"]["dependencies"], json!([]));
    let paths: Vec<_> = beads::diff(&original, &changed)
        .iter()
        .map(|v| v["field"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        paths,
        vec![
            "/dependencies/0",
            "/labels/0",
            "/labels/1",
            "/labels/2",
            "/title"
        ]
    );
}
#[test]
fn scalar_conflicts_and_multivalued_union() {
    let base = records();
    let mut l = base.clone();
    let mut r = base.clone();
    l.get_mut("br-a").unwrap()["priority"] = json!(0);
    r.get_mut("br-a").unwrap()["priority"] = json!(3);
    assert!(beads::merge(&base, &l, &r)
        .unwrap_err()
        .message
        .contains("br-a/priority"));
    r = base.clone();
    r.get_mut("br-a").unwrap()["title"] = json!("peer");
    let m = beads::merge(&base, &l, &r).unwrap();
    assert_eq!(m["br-a"]["priority"], 0);
    assert_eq!(m["br-a"]["title"], "peer");
    l = base.clone();
    r = base.clone();
    l.get_mut("br-a").unwrap()["labels"] = json!(["left"]);
    r.get_mut("br-a").unwrap()["labels"] = json!(["right"]);
    assert_eq!(
        beads::merge(&base, &l, &r).unwrap()["br-a"]["labels"],
        json!(["left", "right"])
    );
    assert_eq!(beads::merge(&base, &m, &m).unwrap(), m);
}
#[test]
fn duplicates_and_absent_null_are_not_silent() {
    let text = beads::render(&records());
    assert!(beads::parse(&(text.clone() + &text)).is_err());
    let a = records();
    let mut b = a.clone();
    b.get_mut("br-a")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("owner");
    let d = beads::diff(&a, &b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["field"], "/owner");
    assert_eq!(d[0]["after_present"], false);
}

#[test]
fn native_facts_survive_reverse_jsonl_roundtrip() {
    use seeds::model::{Comment, Obj, Seed};
    let mut s = Seed {
        id: "sd-new".into(),
        title: "native".into(),
        status: "open".into(),
        priority: 2,
        issue_type: "task".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        revision: 7,
        ..Seed::default()
    };
    s.extra.insert((
        "https://example.org/future".into(),
        Obj::Lang {
            lexical: "hello".into(),
            lang: "en".into(),
        },
    ));
    let mut snap = Snapshot::default();
    snap.seeds.insert(s.id.clone(), s.clone());
    let c = Comment {
        seed: s.id.clone(),
        index: 7,
        author: "a".into(),
        text: "x".into(),
        created_at: s.created_at.clone(),
        extra: s.extra.clone(),
    };
    snap.comments.push(c.clone());
    let before = beads::encode(&snap).unwrap();
    let after = beads::decode(&beads::parse(&beads::render(&before)).unwrap()).unwrap();
    assert_eq!(beads::encode(&after).unwrap(), before);
    let mut loaded = after.seeds[&s.id].clone();
    loaded
        .extra
        .retain(|(p, _)| *p != seeds::vocab::seeds("beadsJson"));
    assert_eq!(loaded, s);
    assert_eq!(after.comments[0], c);
}

#[test]
fn inferred_closed_outcome_does_not_add_a_json_field() {
    let mut r = records();
    r.get_mut("br-a").unwrap()["status"] = json!("closed");
    for outcome in [None, Some(serde_json::Value::Null)] {
        if let Some(v) = outcome {
            r.get_mut("br-a").unwrap()["outcome"] = v;
        }
        let snap = beads::decode(&r).unwrap();
        let mut b = QuipuBackend::in_memory("https://seeds.local/project/closed").unwrap();
        let (batch, _) = sync::plan(&Snapshot::default(), &snap, "beads-sync");
        b.commit(&batch, &context()).unwrap();
        assert_eq!(beads::encode(&b.snapshot(None).unwrap()).unwrap(), r);
    }
}

#[test]
fn concurrent_update_times_compare_fractional_seconds_numerically() {
    let base = records();
    let mut l = base.clone();
    let mut r = base.clone();
    l.get_mut("br-a").unwrap()["updated_at"] = json!("2026-02-01T00:00:00.5Z");
    r.get_mut("br-a").unwrap()["updated_at"] = json!("2026-02-01T00:00:00Z");
    assert_eq!(
        beads::merge(&base, &l, &r).unwrap()["br-a"]["updated_at"],
        "2026-02-01T00:00:00.5Z"
    );
}

#[test]
fn native_export_uses_br_defaults_without_changing_imported_nulls() {
    use seeds::model::Seed;
    let seed = Seed {
        id: "sd-native".into(),
        title: "native".into(),
        status: "open".into(),
        priority: 2,
        issue_type: "task".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        revision: 1,
        ..Seed::default()
    };
    let mut snap = Snapshot::default();
    snap.seeds.insert(seed.id.clone(), seed);
    let encoded = beads::encode(&snap).unwrap();
    let row = &encoded["sd-native"];
    for key in [
        "owner",
        "description",
        "created_by",
        "labels",
        "dependencies",
        "comments",
    ] {
        assert!(row.get(key).is_none(), "{key}");
    }
    assert_eq!(row["source_repo"], ".");
    assert_eq!(row["compaction_level"], 0);
    assert_eq!(row["original_size"], 0);
    assert_eq!(
        beads::encode(&beads::decode(&encoded).unwrap()).unwrap(),
        encoded
    );

    let mut imported = encoded;
    imported.get_mut("sd-native").unwrap()["owner"] = json!(null);
    imported.get_mut("sd-native").unwrap()["labels"] = json!([]);
    assert_eq!(
        beads::encode(&beads::decode(&imported).unwrap()).unwrap(),
        imported
    );
}
