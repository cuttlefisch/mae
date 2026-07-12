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
