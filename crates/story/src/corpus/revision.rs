use anyhow::Result;
use sha2::{Digest, Sha256};

use super::Corpus;

/// Bump when matching, ranking, evidence interpretation or alias parsing changes.
/// Release mapping data is hashed separately so edits cannot silently reuse cursors.
const QUERY_CONTRACT: &str = "story-mcp-m0-m2-v1-body-coverage-rank";

pub(super) fn revision(corpus: &Corpus) -> Result<String> {
    let mut hash = Sha256::new();
    fn field(hash: &mut Sha256, value: impl serde::Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(&value)?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
        Ok(())
    }
    field(&mut hash, QUERY_CONTRACT)?;
    field(&mut hash, crate::engine::releases::revision_data())?;
    let meta = &corpus.meta;
    field(
        &mut hash,
        (
            &meta.source_version,
            &meta.language,
            &meta.local_game_version,
            meta.accuracy_first,
        ),
    )?;
    for group in &corpus.groups {
        field(&mut hash, (&group.id, &group.label, &group.docs))?;
    }
    field(
        &mut hash,
        (&corpus.speakers, &corpus.speaker_identity_names),
    )?;
    field(&mut hash, &corpus.evidence)?;
    for doc in &corpus.docs {
        field(&mut hash, doc)?;
        for line in doc.lines() {
            let l = &corpus.lines[line];
            field(
                &mut hash,
                (
                    corpus.line_text(line),
                    l.speaker,
                    l.kind,
                    &corpus.line_sources[line],
                    corpus.line_evidence_ids[line],
                ),
            )?;
        }
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("corpus_v1_{digest}"))
}
