//! Embeddings / vector search (Phase G): storing per-node+model vectors,
//! HNSW cosine k-NN search, and GraphRAG (vector hits expanded one hop
//! through the typed link graph).

use super::util::{btree_params, cozo_err, dv_str};
use super::*;

impl CozoKbStore {
    /// Store an embedding vector for a node+model pair.
    pub fn store_embedding(&self, id: &str, model: &str, vec: &[f32]) -> Result<(), KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        self.run_mut_params(
            "?[id, model, vec] <- [[$id, $model, $vec]] :put embeddings {id, model => vec}",
            btree_params([
                ("id", dv_str(id)),
                ("model", dv_str(model)),
                ("vec", DataValue::Vec(Vector::F32(arr))),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
    /// Search for k nearest neighbors by vector similarity (HNSW Cosine).
    pub fn vector_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        let result = self
            .run_immut_params(
                &format!(
                    "?[id, distance] := ~embeddings:semantic{{id, model | query: $vec, k: {k}, ef: 50, bind_distance: distance}}"
                ),
                btree_params([("vec", DataValue::Vec(Vector::F32(arr)))]),
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
    /// with score 0.0 (no vector distance — included via structural proximity).
    pub fn graphrag_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        let query = format!(
            r#"entry[id, score] := ~embeddings:semantic{{id | query: $vec, k: {k}, ef: 50, bind_distance: score}}
expanded[id] := entry[id, _]
expanded[id] := entry[mid, _], *links{{src: mid, dst: id}}
expanded[id] := entry[mid, _], *links{{src: id, dst: mid}}
?[id, score] := expanded[id], entry[id, score]
?[id, score] := expanded[id], not entry[id, _], score = 0.0"#
        );
        let result = self
            .run_immut_params(
                &query,
                btree_params([("vec", DataValue::Vec(Vector::F32(arr)))]),
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
