//! create -> dep add -> ready -> close -> ready on an in-memory store, using
//! only the core. It builds natively and for wasm32-unknown-unknown
//! (`just wasm` builds it and reports the linked module size), so it doubles
//! as a size probe for everything the core pulls in.

use seeds::backend::Ctx;
use seeds::engine::{self, CreateReq, ReadyReq};
use seeds::quipu_backend::QuipuBackend;

fn ready(b: &QuipuBackend, ctx: &Ctx) -> Vec<String> {
    engine::ready(b, ctx, &ReadyReq::default(), None)
        .expect("ready")
        .issues
        .into_iter()
        .map(|s| s.id)
        .collect()
}

fn main() {
    let mut b = QuipuBackend::in_memory("https://seeds.local/project/sd").expect("store");
    let ctx = Ctx {
        now: "2026-09-30T00:00:00Z".into(),
        actor: "demo".into(),
        prefix: "sd".into(),
    };
    let mk = |b: &mut QuipuBackend, t: &str| {
        engine::create(
            b,
            &ctx,
            &CreateReq {
                title: t.into(),
                ..Default::default()
            },
        )
        .expect("create")
        .0
        .id
    };
    let parser = mk(&mut b, "Write the parser");
    let grammar = mk(&mut b, "Design the grammar");
    engine::dep_add(&mut b, &ctx, &parser, &grammar, "blocks").expect("dep add");
    assert_eq!(ready(&b, &ctx), vec![grammar.clone()]);
    engine::close(
        &mut b,
        &ctx,
        std::slice::from_ref(&grammar),
        Some("done"),
        false,
    )
    .expect("close");
    assert_eq!(ready(&b, &ctx), vec![parser.clone()]);
    println!("ready after closing {grammar}: {parser}");
}
