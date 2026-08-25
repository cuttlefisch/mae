//! Embeddings / vector search: per-node+model vectors in a fixed-width column
//! whose dimension is pinned lazily to the configured embedding model, brute-force
//! cosine k-NN over it, and GraphRAG (vector hits expanded one hop through the
//! typed link graph). No ANN index -- see `vector_search_for_model`.

use super::util::{btree_params, cozo_err, dv_str};
use super::*;

impl CozoKbStore {
    /// The `instance_meta` key recording the width `embeddings` was created at.
    /// Absent means "legacy or never created" -- see `ensure_embeddings_relation`.
    const EMBEDDINGS_DIM_KEY: &'static str = "embeddings_dim";

    /// ADR-061 Phase F / D2: make `embeddings` exist at exactly `dim` wide,
    /// creating it on first use and RE-creating it if the recorded width differs.
    ///
    /// The width cannot be chosen at schema time: it is a property of whichever
    /// model `ai_embedding_model` names, which `mae-kb` has no access to and which
    /// the user may change. Cozo's fixed-width vector type is fixed at relation
    /// creation, so the only honest options are "pin the store to a dimension" or
    /// "give up fixed width". D2 measured the cost of giving it up -- materializing
    /// 8,000 x 768 values out of a `[Float]` column takes **507ms** versus **26ms**
    /// from `<F32; 768>`, a 19x difference that is the whole reason semantic search
    /// was slow -- so the store pins, and re-pins when the model changes.
    ///
    /// Re-creating is CHEAP AND LOSSLESS in network terms, which is what makes this
    /// safe rather than merely expedient: `embedding_cache` still holds every vector
    /// ever computed, keyed by `(content_hash, model, chunk_version)`, so rebuilding
    /// `embeddings` costs a local re-scan and **no** re-embedding calls.
    fn ensure_embeddings_relation(&self, dim: usize) -> Result<(), KbStoreError> {
        if self.get_meta(Self::EMBEDDINGS_DIM_KEY).ok().flatten() == Some(dim.to_string()) {
            return Ok(());
        }
        // A pre-D2 store carries a `<F32; 384>` `embeddings` plus its
        // `embeddings:semantic` HNSW index. Nothing ever wrote that relation, so
        // there are no contents to migrate -- remove and re-create at the real
        // width. Both removes are best-effort: "does not exist" is the common case.
        let _ = self.run_mut_params("::index drop embeddings:semantic", Default::default());
        let _ = self.run_mut_params("::hnsw drop embeddings:semantic", Default::default());
        let _ = self.run_mut_params("::remove embeddings", Default::default());
        self.run_mut_params(
            &format!(
                r#":create embeddings {{
                    id: String,
                    model: String
                    =>
                    content_hash: String,
                    vec: <F32; {dim}>
                }}"#
            ),
            Default::default(),
        )
        .map_err(cozo_err)?;
        self.set_meta(Self::EMBEDDINGS_DIM_KEY, &dim.to_string())?;
        Ok(())
    }

    /// Store a searchable embedding for a node under a model pin.
    ///
    /// `content_hash` is the `body_hash` of the body this vector was computed
    /// from, carried so a reader can tell a fresh hit from one whose node has been
    /// edited since the last enrichment sweep. It is a non-key column: one node has
    /// one vector per model, and a re-embed replaces it.
    pub fn store_embedding(
        &self,
        id: &str,
        model: &str,
        content_hash: &str,
        vec: &[f32],
    ) -> Result<(), KbStoreError> {
        if vec.is_empty() {
            return Err(KbStoreError::Storage(
                "refusing to store a zero-length embedding".to_string(),
            ));
        }
        self.ensure_embeddings_relation(vec.len())?;
        let arr = ndarray::Array1::from(vec.to_vec());
        self.run_mut_params(
            "?[id, model, content_hash, vec] <- [[$id, $model, $content_hash, $vec]] \
             :put embeddings {id, model => content_hash, vec}",
            btree_params([
                ("id", dv_str(id)),
                ("model", dv_str(model)),
                ("content_hash", dv_str(content_hash)),
                ("vec", DataValue::Vec(Vector::F32(arr))),
            ]),
        )
        .map_err(|e| match Self::is_busy(&e) {
            true => cozo_err(e),
            // The bare cozo message for a width mismatch is
            // "when executing against relation 'embeddings'", which names neither
            // the dimension nor the model -- useless to whoever has to fix it.
            false => KbStoreError::Storage(format!(
                "CozoDB: storing a {}-dim embedding for model '{model}' failed \
                 (store is pinned to {} dims): {e}",
                vec.len(),
                self.get_meta(Self::EMBEDDINGS_DIM_KEY)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unset".to_string()),
            )),
        })?;
        Ok(())
    }

    /// Every stored vector for a model pin, in one query.
    ///
    /// The bulk shape is the point. The previous live path
    /// (`enrichment::search_cached_embeddings`) issued **2N** Datalog queries --
    /// `get_node` then `get_cached_embedding`, per node -- to answer one search,
    /// measured at 1,287ms over 8,000 nodes. Neither the cosine arithmetic nor the
    /// `crdt_doc` column was the cost (swapping in `get_node_light` changed
    /// nothing); it was per-query overhead, ~80us x 16,000.
    pub fn embeddings_for_model(
        &self,
        model: &str,
    ) -> Result<Vec<(String, String, Vec<f32>)>, KbStoreError> {
        if self
            .get_meta(Self::EMBEDDINGS_DIM_KEY)
            .ok()
            .flatten()
            .is_none()
        {
            // Never written under this store: no relation to scan, not an error.
            return Ok(Vec::new());
        }
        let result = self
            .run_immut_params(
                "?[id, content_hash, vec] := *embeddings{id, model: $model, content_hash, vec}",
                btree_params([("model", dv_str(model))]),
            )
            .map_err(cozo_err)?;
        let mut out = Vec::with_capacity(result.rows.len());
        for row in result.rows.iter() {
            let (Some(id), Some(hash), Some(v)) = (row.first(), row.get(1), row.get(2)) else {
                continue;
            };
            let (Some(id), Some(hash)) = (id.get_str(), hash.get_str()) else {
                continue;
            };
            let DataValue::Vec(Vector::F32(arr)) = v else {
                continue;
            };
            out.push((id.to_string(), hash.to_string(), arr.to_vec()));
        }
        Ok(out)
    }

    /// Brute-force cosine k-NN over the stored per-node embeddings for `model`.
    ///
    /// Deliberately brute force rather than an ANN index. A KB mutates on every
    /// edit, and HNSW's deletion story is awkward while its graph overhead is
    /// `M * 8-10` bytes per element (~128-160B at `m: 16`) -- around 3x a
    /// binary-quantized vector. One fixed-width scan measures 26ms at 8,000 nodes,
    /// which is the budget this needs to fit.
    pub fn vector_search_for_model(
        &self,
        model: &str,
        vec: &[f32],
        k: usize,
    ) -> Result<Vec<VectorHit>, KbStoreError> {
        let mut hits: Vec<VectorHit> = self
            .embeddings_for_model(model)?
            .into_iter()
            .filter_map(|(id, _hash, v)| {
                crate::enrichment::cosine_distance(vec, &v)
                    .map(|distance| VectorHit { id, distance })
            })
            .collect();
        hits.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// ADR-061 Phase B: look up a previously-computed embedding by its
    /// content-addressed cache key. A hit means the embedding provider does
    /// NOT need to be called again for this exact (content, model, chunking)
    /// combination -- the whole point of this cache existing. Persisted to
    /// disk via the same `embedding_cache` CozoDB relation every other piece
    /// of KB state lives in, so a hit survives a daemon restart (verified by
    /// a dedicated test that closes and reopens the store, not just an
    /// in-process check).
    pub fn get_cached_embedding(
        &self,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[vec] := *embedding_cache{content_hash: $content_hash, model: $model, chunk_version: $chunk_version, vec}",
                btree_params([
                    ("content_hash", dv_str(content_hash)),
                    ("model", dv_str(model)),
                    ("chunk_version", DataValue::from(chunk_version)),
                ]),
            )
            .map_err(cozo_err)?;
        let Some(row) = result.rows.first() else {
            return Ok(None);
        };
        let Some(DataValue::List(items)) = row.first() else {
            return Ok(None);
        };
        let vec: Option<Vec<f32>> = items
            .iter()
            .map(|v| v.get_float().map(|f| f as f32))
            .collect();
        Ok(vec)
    }

    /// ADR-061 Phase B: record a freshly-computed embedding under its
    /// content-addressed cache key. Deliberately a plain `[Float]` list
    /// column (not the fixed `<F32; 384>` HNSW-typed column `embeddings`
    /// uses) -- this cache is never similarity-searched, only looked up by
    /// exact key, so it is NOT locked to any one embedding model's output
    /// dimension. Distinct entries for a bumped `model`/`chunk_version`
    /// simply live under a different key; nothing here overwrites or prunes
    /// an old entry.
    pub fn put_cached_embedding(
        &self,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
        vec: &[f32],
    ) -> Result<(), KbStoreError> {
        let vec_list = DataValue::List(vec.iter().map(|f| DataValue::from(*f as f64)).collect());
        self.run_mut_params(
            "?[content_hash, model, chunk_version, vec] <- [[$content_hash, $model, $chunk_version, $vec]] \
             :put embedding_cache {content_hash, model, chunk_version => vec}",
            btree_params([
                ("content_hash", dv_str(content_hash)),
                ("model", dv_str(model)),
                ("chunk_version", DataValue::from(chunk_version)),
                ("vec", vec_list),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// GraphRAG search: vector nearest neighbors expanded by 1 hop of graph links.
    ///
    /// Returns vector hits with their distance scores plus graph-adjacent nodes
    /// with score 0.0 (no vector distance -- included via structural proximity).
    ///
    /// D2: the seed set now comes from `vector_search_for_model` rather than the
    /// `embeddings:semantic` HNSW index, which no longer exists. Expansion is still
    /// one Datalog query, so this stays two queries total regardless of `k`.
    pub fn graphrag_search(
        &self,
        model: &str,
        vec: &[f32],
        k: usize,
    ) -> Result<Vec<VectorHit>, KbStoreError> {
        let seeds = self.vector_search_for_model(model, vec, k)?;
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let seed_rows = DataValue::List(
            seeds
                .iter()
                .map(|h| DataValue::List(vec![dv_str(&h.id), DataValue::from(h.distance)]))
                .collect(),
        );
        let result = self
            .run_immut_params(
                r#"entry[id, score] <- $seeds
expanded[id] := entry[id, _]
expanded[id] := entry[mid, _], *links{src: mid, dst: id}
expanded[id] := entry[mid, _], *links{src: id, dst: mid}
?[id, score] := expanded[id], entry[id, score]
?[id, score] := expanded[id], not entry[id, _], score = 0.0"#,
                btree_params([("seeds", seed_rows)]),
            )
            .map_err(cozo_err)?;
        let mut hits = Vec::new();
        for row in result.rows.iter() {
            if let (Some(id), Some(dist)) = (row.first(), row.get(1)) {
                if let (Some(id_s), Some(d)) = (id.get_str(), dist.get_float()) {
                    hits.push(VectorHit {
                        id: id_s.to_string(),
                        distance: d,
                    });
                }
            }
        }
        Ok(hits)
    }
}
