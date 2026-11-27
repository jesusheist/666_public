//! `shader_history`: what happened to each shader over the whole run, kept
//! as one small summary per hash (the event ring only holds the newest few
//! thousand events, so a shader translated at start-up is gone from it
//! after a while).

use crate::json::Json;
use crate::{KIND_FIRST_BIND, KIND_SHADER_TRANSLATED, KINDS, PipelineEvent};
use std::collections::HashMap;
use std::sync::Mutex;

/// Summaries kept at most (a new hash past this is counted, not kept).
pub const MAX_HASHES: usize = 100_000;
/// Translations remembered for each hash.
const KEEP_TRANSLATIONS: usize = 8;

/// Where a translation came from.
#[derive(Clone, Debug, PartialEq)]
pub struct TranslationSource {
    /// `disk_cache_hit`, `fresh_translation_cached` (translated now and
    /// stored in the cache file), `translated_no_cache`,
    /// `retranslated_after_unreadable_cache_entry` or `translation_failed`.
    pub how: &'static str,
    /// The cache file read or written.
    pub path: Option<String>,
    /// Its size in bytes.
    pub size: Option<u64>,
    /// The BLAKE3 of its bytes (hex).
    pub blake3: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Translation {
    pub frame: u64,
    pub stage: Option<String>,
    pub source: Option<TranslationSource>,
    pub error: Option<String>,
    pub duration_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShaderSummary {
    pub stages: Vec<String>,
    pub first_seen_frame: u64,
    pub last_seen_frame: u64,
    /// Events by kind, in the order of [`KINDS`].
    pub kind_counts: [u64; 5],
    /// Pipelines that bound it for the first time (one event each); a
    /// pipeline bound again is not counted here (`frame_commands` lists
    /// what a frame runs).
    pub first_binds: u64,
    pub first_bind_frame: Option<u64>,
    pub last_bind_frame: Option<u64>,
    pub translations_total: u64,
    /// The newest translations, oldest first.
    pub translations: Vec<Translation>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, ShaderSummary>,
    not_kept: u64,
}

#[derive(Default)]
pub struct History {
    inner: Mutex<Inner>,
}

impl History {
    pub fn new() -> Self {
        History::default()
    }

    pub fn record(&self, e: &PipelineEvent) {
        let mut inner = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        if !inner.map.contains_key(&e.hash) && inner.map.len() >= MAX_HASHES {
            inner.not_kept += 1;
            return;
        }
        let s = inner.map.entry(e.hash.clone()).or_insert_with(|| ShaderSummary { first_seen_frame: e.frame, ..Default::default() });
        s.first_seen_frame = s.first_seen_frame.min(e.frame);
        s.last_seen_frame = s.last_seen_frame.max(e.frame);
        if let Some(i) = KINDS.iter().position(|k| *k == e.kind) {
            s.kind_counts[i] += 1;
        }
        if let Some(stage) = &e.stage
            && !s.stages.contains(stage)
        {
            s.stages.push(stage.clone());
        }
        if e.kind == KIND_FIRST_BIND {
            s.first_binds += 1;
            s.first_bind_frame = Some(s.first_bind_frame.map_or(e.frame, |f| f.min(e.frame)));
            s.last_bind_frame = Some(s.last_bind_frame.map_or(e.frame, |f| f.max(e.frame)));
        }
        if e.kind == KIND_SHADER_TRANSLATED {
            s.translations_total += 1;
            if s.translations.len() >= KEEP_TRANSLATIONS {
                s.translations.remove(0);
            }
            s.translations.push(Translation {
                frame: e.frame,
                stage: e.stage.clone(),
                source: e.source.clone(),
                error: e.detail.clone(),
                duration_ms: e.duration_ms,
            });
        }
    }

    /// The summaries whose hash contains any of `hashes` (all if empty) and
    /// that have an event of `kind` (any if `None`), by hash, at most `n`.
    /// Also the number of hashes kept and the number that did not fit.
    pub fn query(&self, hashes: &[String], kind: Option<&str>, n: usize) -> (Vec<(String, ShaderSummary)>, usize, u64) {
        let inner = self.inner.lock().unwrap_or_else(|x| x.into_inner());
        let wanted: Vec<String> = hashes.iter().map(|h| h.to_ascii_lowercase()).collect();
        let kind_at = kind.and_then(|k| KINDS.iter().position(|x| *x == k));
        let mut out: Vec<(String, ShaderSummary)> = inner
            .map
            .iter()
            .filter(|(h, s)| {
                (wanted.is_empty() || wanted.iter().any(|w| h.to_ascii_lowercase().contains(w.as_str())))
                    && kind_at.is_none_or(|i| s.kind_counts[i] > 0)
            })
            .map(|(h, s)| (h.clone(), s.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.truncate(n);
        (out, inner.map.len(), inner.not_kept)
    }
}

fn opt_str(v: &Option<String>) -> Json {
    v.as_deref().map_or(Json::Null, Json::str)
}

pub fn summary_json(hash: &str, s: &ShaderSummary) -> Json {
    let kinds = Json::Obj(KINDS.iter().zip(s.kind_counts).map(|(k, n)| ((*k).to_owned(), Json::uint(n))).collect());
    let translations = s
        .translations
        .iter()
        .map(|t| {
            let (how, path, size, blake3) = match &t.source {
                Some(src) => (Json::str(src.how), opt_str(&src.path), src.size.map_or(Json::Null, Json::uint), opt_str(&src.blake3)),
                None => (Json::Null, Json::Null, Json::Null, Json::Null),
            };
            Json::obj([
                ("frame", Json::uint(t.frame)),
                ("stage", opt_str(&t.stage)),
                ("source", how),
                ("cache_file", path),
                ("cache_file_bytes", size),
                ("cache_file_blake3", blake3),
                ("error", opt_str(&t.error)),
                ("duration_ms", t.duration_ms.map_or(Json::Null, Json::num)),
            ])
        })
        .collect();
    let frame = |f: Option<u64>| f.map_or(Json::Null, Json::uint);
    Json::obj([
        ("hash", Json::str(hash)),
        ("stages", Json::Arr(s.stages.iter().map(|x| Json::str(x.as_str())).collect())),
        ("first_seen_frame", Json::uint(s.first_seen_frame)),
        ("last_seen_frame", Json::uint(s.last_seen_frame)),
        ("events_by_kind", kinds),
        ("times_first_bound", Json::uint(s.first_binds)),
        ("first_bound_frame", frame(s.first_bind_frame)),
        ("last_bound_frame", frame(s.last_bind_frame)),
        ("translations_total", Json::uint(s.translations_total)),
        ("translations", Json::Arr(translations)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &'static str, hash: &str, frame: u64, source: Option<TranslationSource>, detail: Option<&str>) -> PipelineEvent {
        PipelineEvent {
            kind,
            hash: hash.to_owned(),
            stage: Some("pixel".to_owned()),
            detail: detail.map(str::to_owned),
            frame,
            at_ms: 0.0,
            duration_ms: Some(1.5),
            source,
        }
    }

    fn hit() -> TranslationSource {
        TranslationSource { how: "disk_cache_hit", path: Some("cache/shaders/ab/cd".into()), size: Some(10), blake3: Some("ff".into()) }
    }

    #[test]
    fn summaries_follow_a_hash_across_events() {
        let h = History::new();
        h.record(&ev(KIND_SHADER_TRANSLATED, "1aa247ed3653b616", 5, Some(hit()), None));
        h.record(&ev(KIND_FIRST_BIND, "1aa247ed3653b616", 9, None, None));
        h.record(&ev(KIND_FIRST_BIND, "1aa247ed3653b616", 12, None, None));
        h.record(&ev(KIND_SHADER_TRANSLATED, "5dc3b25986622949", 6, None, Some("boom")));
        let (all, kept, missed) = h.query(&[], None, 10);
        assert_eq!((all.len(), kept, missed), (2, 2, 0));
        let (one, _, _) = h.query(&["1AA247ED".to_owned()], None, 10);
        assert_eq!(one.len(), 1);
        let s = &one[0].1;
        assert_eq!((s.first_seen_frame, s.last_seen_frame), (5, 12));
        assert_eq!((s.first_binds, s.first_bind_frame, s.last_bind_frame), (2, Some(9), Some(12)));
        assert_eq!(s.translations_total, 1);
        assert_eq!(s.translations[0].source.as_ref().map(|x| x.how), Some("disk_cache_hit"));
        let (failed, _, _) = h.query(&["5dc3".to_owned()], Some(KIND_SHADER_TRANSLATED), 10);
        assert_eq!(failed[0].1.translations[0].error.as_deref(), Some("boom"));
        let (none, _, _) = h.query(&["5dc3".to_owned()], Some(KIND_FIRST_BIND), 10);
        assert!(none.is_empty());
    }

    #[test]
    fn translations_kept_are_bounded() {
        let h = History::new();
        for f in 0..20 {
            h.record(&ev(KIND_SHADER_TRANSLATED, "aa", f, None, None));
        }
        let (r, _, _) = h.query(&[], None, 5);
        assert_eq!(r[0].1.translations.len(), KEEP_TRANSLATIONS);
        assert_eq!(r[0].1.translations_total, 20);
        assert_eq!(r[0].1.translations[0].frame, 12);
    }
}
