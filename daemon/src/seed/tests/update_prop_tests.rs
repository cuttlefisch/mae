//! The update as a property: for a random history of a synthetic repository
//! (pages added, edited, moved, removed, restored, retitled, retagged), seeding
//! the first revision and updating through the rest gives exactly what a FRESH
//! seed of the last revision gives, node for node and row for row, while the
//! KB keeps its id, owner and every member added along the way, and each node
//! keeps a version for every revision that changed it.

use std::collections::BTreeMap;

use mae_mcp::identity::Identity;
use mae_sync::kb::Role;
use proptest::prelude::*;

use super::apply_tests::{seeded, KB, T};
use super::git_tests::Repo;
use super::page;
use super::update_tests::{fresh, rows, snapshot, update_to};
use crate::collab_handler::{operator_change_membership, operator_list_members, OperatorMemberOp};

#[derive(Debug, Clone, PartialEq)]
struct Page {
    path: String,
    title: Option<String>,
    tags: Option<Vec<String>>,
    body: String,
}

/// The pages of one revision, by `:ID:`, and the ones removed so far.
#[derive(Debug, Default)]
struct Model {
    live: BTreeMap<String, Page>,
    gone: BTreeMap<String, Page>,
    counter: usize,
}

impl Model {
    fn new() -> Self {
        let mut m = Model::default();
        for w in ["uno", "dos"] {
            m.add(w);
        }
        m
    }

    fn add(&mut self, word: &str) {
        self.counter += 1;
        let id = format!("0b000000-0000-4000-8000-{:012}", self.counter);
        let page = Page {
            path: format!("d{}/{word}-{}.org", self.counter % 3, self.counter),
            title: Some(format!("Página {word}")),
            tags: None,
            body: format!("Texto {word} ñ.\n* Sección\n"),
        };
        self.live.insert(id, page);
    }

    /// Apply one generated operation; one that does not apply is skipped.
    fn apply(&mut self, (kind, at, word): &(u8, prop::sample::Index, String)) {
        if kind % 7 == 0 {
            return self.add(word);
        }
        if kind % 7 == 6 {
            return self.restore(at, word);
        }
        let ids: Vec<String> = self.live.keys().cloned().collect();
        let id = &ids[at.index(ids.len())];
        self.counter += 1;
        let n = self.counter;
        if kind % 7 == 3 {
            if self.live.len() > 1 {
                let p = self.live.remove(id).unwrap();
                self.gone.insert(id.clone(), p);
            }
            return;
        }
        let p = self.live.get_mut(id).unwrap();
        match kind % 7 {
            1 => p.body = format!("{word}\n{}", p.body),
            2 => p.path = format!("movido/{word}-{n}.org"),
            4 => p.title = (!word.len().is_multiple_of(4)).then(|| word.clone()),
            _ => {
                p.tags =
                    (!word.len().is_multiple_of(3)).then(|| vec![word.clone(), format!("t{n}")])
            }
        }
    }

    /// A removed page returns under its own id, at a new path, perhaps edited.
    fn restore(&mut self, at: &prop::sample::Index, word: &str) {
        let ids: Vec<String> = self.gone.keys().cloned().collect();
        if ids.is_empty() {
            return;
        }
        let id = ids[at.index(ids.len())].clone();
        let mut p = self.gone.remove(&id).unwrap();
        self.counter += 1;
        p.path = format!("vuelta/{word}-{}.org", self.counter);
        if word.len().is_multiple_of(2) {
            p.body.push_str(&format!("{word}\n"));
        }
        self.live.insert(id, p);
    }

    /// Write this revision's working copy: exactly the live pages.
    fn write(&self, r: &Repo) {
        for dir in ["d0", "d1", "d2", "movido", "vuelta"] {
            let _ = std::fs::remove_dir_all(r.path().join(dir));
        }
        for (id, p) in &self.live {
            let tags: Option<Vec<&str>> = p
                .tags
                .as_ref()
                .map(|t| t.iter().map(String::as_str).collect());
            let text = page(
                &[("ID", id), ("ORDEN", "x")],
                p.title.as_deref(),
                tags.as_deref(),
                &p.body,
            );
            r.write(&p.path, &text);
        }
    }
}

fn op() -> impl Strategy<Value = (u8, prop::sample::Index, String)> {
    (any::<u8>(), any::<prop::sample::Index>(), "[a-z]{1,7}")
}

async fn run(revisions: Vec<Vec<(u8, prop::sample::Index, String)>>) {
    let r = Repo::new();
    let mut model = Model::new();
    model.write(&r);
    r.commit(T);
    r.git(&["tag", "v1"]);
    let (store, owner, _) = seeded(&r).await;
    let mut members = Vec::new();
    let mut changes: BTreeMap<String, usize> = BTreeMap::new();
    let mut before = snapshot(&store).await;
    let mut last = "v1".to_string();
    for (i, ops) in revisions.iter().enumerate() {
        for o in ops {
            model.apply(o);
        }
        model.write(&r);
        // A revision whose operations all were skipped changes nothing.
        r.git(&["add", "-A"]);
        let t = T + 100 * (i as i64 + 1);
        r.git_with_time(&["commit", "-q", "--allow-empty", "-m", "c"], t);
        last = format!("v{}", i + 2);
        r.git(&["tag", &last]);
        let m = Identity::generate("member").fingerprint();
        let add = OperatorMemberOp::Add {
            member: &m,
            role: Role::Viewer,
            label: "m",
        };
        operator_change_membership(&store, &owner, KB, add)
            .await
            .unwrap();
        members.push(m);
        update_to(&store, &owner, &r, &last, false).await.unwrap();
        let after = snapshot(&store).await;
        for (id, old) in &before {
            if after.get(id).is_some_and(|new| new != old) {
                *changes.entry(id.clone()).or_default() += 1;
            }
        }
        before = after;
    }
    let oracle = fresh(&r, &last).await;
    assert_eq!(snapshot(&store).await, snapshot(&oracle).await);
    assert_eq!(rows(&store).await, rows(&oracle).await);
    let (record, _) = store.seed_record(KB).await.unwrap().unwrap();
    assert_eq!(record.kb_id, KB);
    assert_eq!(Some(record.plan_hash), r.plan(&last).plan_hash());
    let (got_owner, listed) = operator_list_members(&store, KB).await.unwrap();
    assert_eq!(got_owner, owner.fingerprint());
    for m in &members {
        assert!(listed.iter().any(|(fp, _)| fp == m), "member {m} lost");
    }
    for (id, n) in changes {
        let versions = store.history_list(KB, &id).await.unwrap().len();
        assert!(versions >= n, "{id}: {versions} versions for {n} changes");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

    /// Kills: any field the update does not carry (a fresh seed has it), a
    /// removal or restoration not reflected in the manifest, rows left at the
    /// old plan, a lost member, a change that leaves no version.
    #[test]
    fn updating_through_a_history_equals_a_fresh_seed_of_its_end(
        revisions in prop::collection::vec(prop::collection::vec(op(), 1..4), 1..5)
    ) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run(revisions));
    }
}
