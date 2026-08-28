//! STT model catalog and downloader.
//!
//! Holds the hard-coded catalog of speech-to-text models (whisper.cpp ggml
//! models and sherpa-onnx transducer models, both from Hugging Face), tracks
//! which ones are installed under `data_dir/models/`, and performs
//! checksum-verified downloads. A catalog entry is one or more artifacts: a
//! whisper model is a single `.bin` file, and a model with several artifacts
//! (e.g. a sherpa-onnx transducer's encoder, decoder, joiner and tokens) is
//! installed as a directory holding all of them. Each artifact streams to
//! its own resumable `.part` file while its sha256 is computed incrementally, and a
//! model only becomes visible at its final path once every one of its
//! artifacts has verified. Cancellation and transport failures preserve a
//! partial for the next attempt; invalid ranges and checksum failures remove
//! it. Neither case can expose a half-installed model.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::IntegrityError;

/// Cooperative cancellation shared between the desktop owner and the
/// blocking downloader. The store crate knows nothing about Tauri; it only
/// observes this one-way flag at safe boundaries.
#[derive(Debug, Clone, Default)]
pub struct DownloadCancellation(Arc<AtomicBool>);

impl DownloadCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(DownloadCancelled.into())
        } else {
            Ok(())
        }
    }
}

/// Typed normal outcome used to distinguish a user's cancellation from a
/// network, disk, or integrity failure carried by `anyhow::Error`.
#[derive(Debug, thiserror::Error)]
#[error("model download cancelled")]
pub struct DownloadCancelled;

/// A response was established but stopped yielding body bytes. Keeping this
/// error distinct makes the desktop message actionable while the resumable
/// `.part` remains available for a retry.
#[derive(Debug, thiserror::Error)]
#[error("model download stalled for {seconds} seconds; retry to resume")]
pub struct DownloadStalled {
    seconds: u64,
}

#[derive(Debug, Clone, Copy)]
struct DownloadTimeouts {
    connect: Duration,
    stall: Duration,
}

const DOWNLOAD_TIMEOUTS: DownloadTimeouts = DownloadTimeouts {
    connect: Duration::from_secs(20),
    stall: Duration::from_secs(30),
};

const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Where a model's output is used in the dictation pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Produces the transcript that can be inserted into the focused app.
    Final,
    /// Produces low-latency text shown only in the recording HUD.
    Preview,
}

/// A deliberately coarse relative cost indicator. Actual latency depends on
/// hardware and acceleration, so the catalog avoids promising timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceClass {
    Fast,
    Balanced,
    Heavy,
}

/// A speech-to-text model available for download, with its installed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub engine: String,
    pub label: String,
    pub size_mb: u32,
    pub installed: bool,
    /// BCP-47 language prefixes, or `"*"` for broadly multilingual models.
    pub supported_languages: Vec<String>,
    pub role: ModelRole,
    pub performance_class: PerformanceClass,
    /// Short, user-facing reasons to choose this model. These are qualitative
    /// recommendations, not hardware-independent benchmark promises.
    pub recommendation_tags: Vec<String>,
}

/// One downloadable file that makes up a catalog entry.
///
/// `name` is the file name the artifact is installed under (directly inside
/// `models/` for a single-artifact entry, or inside the entry's model
/// directory for a multi-artifact one) — it is independent of `url`, since a
/// remote file's name is not always the one it should be installed as.
#[derive(Debug, Clone, Copy)]
struct Artifact {
    url: &'static str,
    sha256: &'static str,
    name: &'static str,
    /// The artifact's exact size once fully downloaded, used by
    /// [`ModelManager::verify_installed`] to catch a truncated file before
    /// its path is ever handed to a native engine. This is a size check,
    /// not a re-hash: it catches truncation — the failure mode actually
    /// observed in practice — but not a file of the right length with
    /// corrupted bytes inside it; that class of corruption is what the
    /// download path's sha256 verification already guards against, and
    /// re-hashing hundreds of megabytes on every engine load would trade a
    /// cheap, load-time-only guard for a slow one that mostly re-checks
    /// what already passed once.
    size_bytes: u64,
}

/// Static metadata for one catalog entry.
///
/// `engine` distinguishes the STT engine a model belongs to; installation
/// itself is driven purely by artifact count: a single file directly under
/// `models/` when there is exactly one artifact (e.g. `"whisper"`), or a
/// directory named after the entry's `id` holding every artifact when there
/// is more than one (e.g. a sherpa-onnx transducer's encoder, decoder,
/// joiner and tokens).
#[derive(Debug, Clone, Copy)]
struct CatalogEntry {
    id: &'static str,
    engine: &'static str,
    label: &'static str,
    size_mb: u32,
    supported_languages: &'static [&'static str],
    role: ModelRole,
    performance_class: PerformanceClass,
    recommendation_tags: &'static [&'static str],
    artifacts: &'static [Artifact],
}

/// The hard-coded catalog of downloadable speech-to-text models.
///
/// Official Whisper sha256 and size_bytes values were read from the Hugging
/// Face tree API for `ggerganov/whisper.cpp` (`lfs.oid` and `size` per file).
/// Breeze-ASR-25 uses the exact artifact published by Handy; its checksum was
/// verified from a complete download and its byte length against the origin's
/// `Content-Length`.
/// Sherpa-onnx sha256 and size_bytes values were read from the Hugging Face
/// tree API for each model's repository at the pinned revision in its
/// artifact URLs — except `tokens.txt`, which none of these repositories
/// track as an LFS file, so the tree API only has a git blob hash for it,
/// not its sha256. Every `tokens.txt` entry's sha256 was instead obtained by
/// downloading the file and hashing it directly.
const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "tiny",
        engine: "whisper",
        label: "Whisper Tiny",
        size_mb: 74,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Lowest latency", "Lower accuracy"],
        artifacts: &[Artifact {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
            name: "ggml-tiny.bin",
            size_bytes: 77_691_713,
        }],
    },
    CatalogEntry {
        id: "base",
        engine: "whisper",
        label: "Whisper Base",
        size_mb: 141,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Lightweight", "Lower accuracy"],
        artifacts: &[Artifact {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            name: "ggml-base.bin",
            size_bytes: 147_951_465,
        }],
    },
    CatalogEntry {
        id: "small",
        engine: "whisper",
        label: "Whisper Small",
        size_mb: 465,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Balanced,
        recommendation_tags: &["Balanced multilingual"],
        artifacts: &[Artifact {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            name: "ggml-small.bin",
            size_bytes: 487_601_967,
        }],
    },
    CatalogEntry {
        id: "medium",
        engine: "whisper",
        label: "Whisper Medium",
        size_mb: 1463,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Heavy,
        recommendation_tags: &["Higher accuracy"],
        artifacts: &[Artifact {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
            name: "ggml-medium.bin",
            size_bytes: 1_533_763_059,
        }],
    },
    CatalogEntry {
        id: "large-v3-turbo-q5_0",
        engine: "whisper",
        label: "Whisper Large v3 Turbo (q5_0)",
        size_mb: 547,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Balanced,
        recommendation_tags: &["High accuracy", "Mixed language"],
        artifacts: &[Artifact {
            url:
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            name: "ggml-large-v3-turbo-q5_0.bin",
            size_bytes: 574_041_195,
        }],
    },
    CatalogEntry {
        id: "breeze-asr-25-q5_k",
        engine: "whisper",
        label: "Breeze-ASR-25 (q5_k, Russian + English)",
        size_mb: 1030,
        supported_languages: &["ru", "en"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Heavy,
        recommendation_tags: &["Mixed Russian + English"],
        artifacts: &[Artifact {
            url: "https://blob.handy.computer/breeze-asr-q5_k.bin",
            sha256: "8efbf0ce8a3f50fe332b7617da787fb81354b358c288b008d3bdef8359df64c6",
            name: "breeze-asr-q5_k.bin",
            size_bytes: 1_080_732_108,
        }],
    },
    CatalogEntry {
        id: "large-v2-q5_0",
        engine: "whisper",
        label: "Whisper Large v2 (q5_0)",
        size_mb: 1030,
        supported_languages: &["*"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Heavy,
        recommendation_tags: &["Stable quality"],
        artifacts: &[Artifact {
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/bf8b606c2fcd9173605cdf6bd2ac8a75a8141b6c/ggml-large-v2-q5_0.bin",
            sha256: "3a214837221e4530dbc1fe8d734f302af393eb30bd0ed046042ebf4baf70f6f2",
            name: "ggml-large-v2-q5_0.bin",
            size_bytes: 1_080_732_091,
        }],
    },
    CatalogEntry {
        id: "gigaam-v3-e2e-rnnt",
        engine: "sherpa",
        label: "GigaAM-v3 (Russian)",
        size_mb: 221,
        supported_languages: &["ru"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Recommended for Russian", "Punctuation included"],
        artifacts: &[
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/a6039be7cee829a9044a69ac0ebaf1c191217c97/encoder.int8.onnx",
                sha256: "369f35a71bf288d3b8e0391fabd8dba5f2314088d440bca474056b7b4b6e66bf",
                name: "encoder.int8.onnx",
                size_bytes: 224_570_820,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/a6039be7cee829a9044a69ac0ebaf1c191217c97/decoder.onnx",
                sha256: "38fc7475443ea2a26f63211ca350f73ac50fff824ab7a3876ee2bd610c53bbc4",
                name: "decoder.onnx",
                size_bytes: 4_600_132,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/a6039be7cee829a9044a69ac0ebaf1c191217c97/joiner.onnx",
                sha256: "602ff7017a93311aad34df1437c8d7f49911353c13d6eae7a6ee7b041339465c",
                name: "joiner.onnx",
                size_bytes: 2_712_896,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16/resolve/a6039be7cee829a9044a69ac0ebaf1c191217c97/tokens.txt",
                sha256: "39abae20e692998290c574e606f11a9edef2902a1995463fcff63d1490cf22b7",
                name: "tokens.txt",
                size_bytes: 13_354,
            },
        ],
    },
    CatalogEntry {
        id: "parakeet-tdt-110m-en",
        engine: "sherpa",
        label: "Parakeet TDT (English)",
        size_mb: 455,
        supported_languages: &["en"],
        role: ModelRole::Final,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Recommended for English", "Punctuation included"],
        artifacts: &[
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000/resolve/e9bea5a06247dc3f55319ff23d34b0328f2f5ddf/encoder.onnx",
                sha256: "db260f1073c654c37dd65006885d1ee98ff16c22463b1ef992bbcabc29780a3f",
                name: "encoder.onnx",
                size_bytes: 456_050_698,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000/resolve/e9bea5a06247dc3f55319ff23d34b0328f2f5ddf/decoder.onnx",
                sha256: "3da156bde41a04c94ef783e0bd92928e9974e08645b976a22d0c3e1063510249",
                name: "decoder.onnx",
                size_bytes: 15_753_086,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000/resolve/e9bea5a06247dc3f55319ff23d34b0328f2f5ddf/joiner.onnx",
                sha256: "b603765c0724a0768c378a23326dabbeb9cfea932d260e4fcc14384fa5fd5aff",
                name: "joiner.onnx",
                size_bytes: 5_596_854,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000/resolve/e9bea5a06247dc3f55319ff23d34b0328f2f5ddf/tokens.txt",
                sha256: "450e56bd2f036fe5b6aa821865838cc5aa9d8b0106134ce9a9ba0664abe6cd10",
                name: "tokens.txt",
                size_bytes: 9_953,
            },
        ],
    },
    CatalogEntry {
        id: "zipformer-ru-small",
        engine: "sherpa-streaming",
        label: "Zipformer Small (Russian, streaming)",
        size_mb: 27,
        supported_languages: &["ru"],
        role: ModelRole::Preview,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Live preview only"],
        artifacts: &[
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-small-ru-vosk-int8-2025-08-16/resolve/31fa603e4f31279c6e1f7600fed13dc4312663ab/encoder.int8.onnx",
                sha256: "e0db705e94ec35d803b1df4f40cda23d064e1142977c80ab288430b109777a9d",
                name: "encoder.onnx",
                size_bytes: 26_214_060,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-small-ru-vosk-int8-2025-08-16/resolve/31fa603e4f31279c6e1f7600fed13dc4312663ab/decoder.onnx",
                sha256: "89b3088a9e20e1ef7f2e85ce1a3478afe6a9c4ac57369cabcc4beb8e95328ea0",
                name: "decoder.onnx",
                size_bytes: 2_093_080,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-small-ru-vosk-int8-2025-08-16/resolve/31fa603e4f31279c6e1f7600fed13dc4312663ab/joiner.int8.onnx",
                sha256: "b55784b071ab7512eab4c7c44e4f5478284ef33c83562cc6a249b972515a31e5",
                name: "joiner.onnx",
                size_bytes: 259_417,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-small-ru-vosk-int8-2025-08-16/resolve/31fa603e4f31279c6e1f7600fed13dc4312663ab/tokens.txt",
                sha256: "93bbbc0bae6b78c0bbb743d4aa9fded3bb5ff3aac5f0200e3a769a5a05e0fdf6",
                name: "tokens.txt",
                size_bytes: 6_388,
            },
        ],
    },
    CatalogEntry {
        id: "zipformer-en-small",
        engine: "sherpa-streaming",
        label: "Zipformer Small (English, streaming)",
        size_mb: 43,
        supported_languages: &["en"],
        role: ModelRole::Preview,
        performance_class: PerformanceClass::Fast,
        recommendation_tags: &["Live preview only"],
        artifacts: &[
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/d42f2d9f7ca24806fb667456a18a9f1b60f70d16/encoder-epoch-99-avg-1.int8.onnx",
                sha256: "3810755ce7c3ab26b42a8bcf39d191308fa27fb0f53358823ba46141d03b7eb3",
                name: "encoder.onnx",
                size_bytes: 42_845_182,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/d42f2d9f7ca24806fb667456a18a9f1b60f70d16/decoder-epoch-99-avg-1.onnx",
                sha256: "45a7f940ecfb53d89fa270ad11b88b961e53a317203eb24b1c8e95ed208b0f30",
                name: "decoder.onnx",
                size_bytes: 2_092_272,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/d42f2d9f7ca24806fb667456a18a9f1b60f70d16/joiner-epoch-99-avg-1.int8.onnx",
                sha256: "e085d73b593cf9b0707f370dbd656d58327d3fe36d80d849202ef81df02cb01e",
                name: "joiner.onnx",
                size_bytes: 259_572,
            },
            Artifact {
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/d42f2d9f7ca24806fb667456a18a9f1b60f70d16/tokens.txt",
                sha256: "49e3c2646595fd907228b3c6787069658f67b17377c60aeb8619c4551b2316fb",
                name: "tokens.txt",
                size_bytes: 5_048,
            },
        ],
    },
];

/// Manages the local install state of the speech-to-text model catalog:
/// lists what is available, resolves installed paths, and performs
/// checksum-verified downloads and removal.
pub struct ModelManager {
    data_dir: PathBuf,
    catalog: Vec<CatalogEntry>,
}

impl ModelManager {
    /// Creates a manager whose models live under `data_dir/models/`.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            catalog: CATALOG.to_vec(),
        }
    }

    /// Test-only constructor: builds a manager against a custom catalog
    /// (typically pointing at a mock server) instead of the real, hard-coded
    /// one, so tests never hit the network.
    #[cfg(test)]
    fn with_catalog(data_dir: PathBuf, catalog: Vec<CatalogEntry>) -> Self {
        Self { data_dir, catalog }
    }

    /// Lists every catalog entry, each annotated with whether it is
    /// currently installed under this manager's `data_dir`.
    pub fn catalog(&self) -> Vec<ModelInfo> {
        self.catalog
            .iter()
            .map(|entry| {
                let path = self.install_path(entry);
                ModelInfo {
                    id: entry.id.to_string(),
                    engine: entry.engine.to_string(),
                    label: entry.label.to_string(),
                    size_mb: entry.size_mb,
                    installed: self.is_installed(entry, &path),
                    supported_languages: entry
                        .supported_languages
                        .iter()
                        .map(|language| (*language).to_string())
                        .collect(),
                    role: entry.role,
                    performance_class: entry.performance_class,
                    recommendation_tags: entry
                        .recommendation_tags
                        .iter()
                        .map(|tag| (*tag).to_string())
                        .collect(),
                }
            })
            .collect()
    }

    /// Returns the installed path for `id` (a file for single-artifact
    /// models, a directory for multi-artifact models), or `None` if
    /// it is unknown, or not installed. A multi-artifact model is only
    /// reported as installed once every one of its artifacts is present —
    /// a partially downloaded model must never report as ready.
    pub fn path_for(&self, id: &str) -> Option<PathBuf> {
        let entry = self.find(id)?;
        let path = self.install_path(entry);
        self.is_installed(entry, &path).then_some(path)
    }

    /// The catalog `engine` string `id` is filed under (`"whisper"`,
    /// `"sherpa"`, `"sherpa-streaming"`), or `None` if `id` is not in the
    /// catalog at all.
    ///
    /// This is what lets a caller check a model's *kind* before loading it,
    /// which [`verify_installed`](Self::verify_installed) deliberately does
    /// not: that call answers "are these files intact", not "are these the
    /// files this engine can read". The two questions are independent, and
    /// several entries of different kinds install under the very same
    /// artifact names, so a perfectly intact model of the wrong kind reaches
    /// the loader looking exactly like a right one.
    pub fn engine_of(&self, id: &str) -> Option<&str> {
        self.find(id).map(|entry| entry.engine)
    }

    /// The file names `id`'s artifacts are installed under, in catalog
    /// order, or `None` if `id` is not in the catalog at all.
    ///
    /// The contents of a model directory are a contract with whoever loads
    /// it: an engine opens fixed file names inside the directory this
    /// manager installed, and [`Artifact::name`] is what decides those names
    /// — deliberately independent of the URL, since upstream file names vary
    /// per release (`encoder.int8.onnx`, `encoder-epoch-99-avg-1.int8.onnx`,
    /// ...) and an engine cannot chase them. That contract is otherwise
    /// stated twice, in two crates that cannot see each other, with nothing
    /// checking the copies still agree; this is the accessor that lets a
    /// crate downstream of both hold them against one another.
    pub fn artifact_names(&self, id: &str) -> Option<Vec<&'static str>> {
        self.find(id)
            .map(|entry| entry.artifacts.iter().map(|a| a.name).collect())
    }

    /// Downloads and installs the model identified by `id`.
    ///
    /// Each artifact's response body streams into a resumable `.part` file
    /// inside a staging area while its sha256 is computed incrementally;
    /// `progress(done, total)` is called after every chunk of every artifact,
    /// with `total` the catalog's grand total across every artifact in the
    /// entry (known upfront from each artifact's `size_bytes`, independent of
    /// whatever the server reports as `Content-Length`) and `done` running
    /// cumulatively across artifacts, never resetting to zero partway through
    /// a multi-artifact model. An artifact's digest is checked against its
    /// catalog sha256 as soon as it finishes downloading: on the first
    /// mismatch removes that artifact's partial. Cancellation and transport
    /// failures keep valid bytes for a later Range request. The staging area
    /// is outside [`Self::path_for`], so no half-downloaded model can report
    /// as installed.
    ///
    /// Only once every artifact has verified does the model become visible
    /// at its final path: a single-artifact model (e.g. whisper) is renamed
    /// directly into place as one file, and a multi-artifact model (e.g. a
    /// sherpa-onnx entry) has its whole staging directory renamed into place
    /// at once.
    pub fn download(&self, id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf> {
        self.download_with_cancellation(id, &DownloadCancellation::default(), progress)
    }

    /// [`Self::download`] with cooperative cancellation. Cancellation is
    /// checked before network access, around every response-body read,
    /// between artifacts, and immediately before the atomic install rename.
    pub fn download_with_cancellation(
        &self,
        id: &str,
        cancellation: &DownloadCancellation,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf> {
        let entry = *self
            .find(id)
            .ok_or_else(|| anyhow!("unknown model id: {id}"))?;
        cancellation.check()?;

        let models_dir = self.models_dir();
        fs::create_dir_all(&models_dir)
            .with_context(|| format!("failed to create {}", models_dir.display()))?;

        self.download_artifacts(id, &entry, &models_dir, cancellation, progress)
    }

    /// Downloads and verifies every artifact of `entry` into a staging
    /// directory, then puts it in its final place: a single file when there
    /// is exactly one artifact, or the whole directory when there are
    /// several. See [`Self::download`] for the staging/verification
    /// contract.
    fn download_artifacts(
        &self,
        id: &str,
        entry: &CatalogEntry,
        models_dir: &Path,
        cancellation: &DownloadCancellation,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf> {
        let Some((first, _)) = entry.artifacts.split_first() else {
            bail!("model '{id}' has no artifacts defined");
        };

        cancellation.check()?;
        let staging_dir = models_dir.join(format!("{}.staging", entry.id));
        if staging_dir.is_file() {
            fs::remove_file(&staging_dir).with_context(|| {
                format!(
                    "failed to replace invalid staging file {}",
                    staging_dir.display()
                )
            })?;
        }
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("failed to create {}", staging_dir.display()))?;

        // The catalog already knows every artifact's exact size, so the
        // grand total is fixed up front and does not depend on the server's
        // `Content-Length`. Reporting `completed + done` against that fixed
        // total (rather than passing each artifact's own `stream_to_part`
        // progress straight through) keeps the sequence handed to `progress`
        // monotonic across the whole model, instead of resetting to zero at
        // the start of every artifact.
        let grand_total: u64 = entry.artifacts.iter().map(|a| a.size_bytes).sum();
        let mut completed: u64 = 0;
        for artifact in entry.artifacts {
            cancellation.check()?;
            let mut aggregate = |done: u64, _total: u64| progress(completed + done, grand_total);
            if let Err(error) =
                stage_one_artifact(id, artifact, &staging_dir, cancellation, &mut aggregate)
            {
                // Remove only an empty directory. A transport failure or
                // cancellation leaves a `.part`; previously verified
                // artifacts of a multi-file model also remain resumable.
                let _ = fs::remove_dir(&staging_dir);
                return Err(error);
            }
            completed += artifact.size_bytes;
            // One explicit boundary callback lets an owner cancel after a
            // verified artifact and before the next request. Skip the final
            // boundary because the stream already reported completion and a
            // duplicate 100% event would add no information.
            if completed < grand_total {
                progress(completed, grand_total);
            }
        }

        cancellation.check()?;

        let final_path = self.install_path(entry);
        if entry.artifacts.len() > 1 {
            if final_path.exists() {
                fs::remove_dir_all(&final_path).with_context(|| {
                    format!(
                        "failed to remove previous install at {}",
                        final_path.display()
                    )
                })?;
            }
            fs::rename(&staging_dir, &final_path).with_context(|| {
                format!(
                    "failed to move {} into {}",
                    staging_dir.display(),
                    final_path.display()
                )
            })?;
        } else {
            let staged_file = staging_dir.join(first.name);
            let renamed = fs::rename(&staged_file, &final_path).with_context(|| {
                format!(
                    "failed to move {} into {}",
                    staged_file.display(),
                    final_path.display()
                )
            });
            let _ = fs::remove_dir_all(&staging_dir);
            renamed?;
        }

        Ok(final_path)
    }

    /// Removes the installed model identified by `id`, if present. A no-op
    /// (not an error) if the model is unknown or not installed.
    pub fn remove(&self, id: &str) -> Result<()> {
        let Some(entry) = self.find(id) else {
            return Ok(());
        };

        let path = self.install_path(entry);
        if path.is_dir() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove directory {}", path.display()))?;
        } else if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove file {}", path.display()))?;
        }

        let staging = self.models_dir().join(format!("{}.staging", entry.id));
        if staging.is_dir() {
            fs::remove_dir_all(&staging)
                .with_context(|| format!("failed to remove directory {}", staging.display()))?;
        } else if staging.is_file() {
            fs::remove_file(&staging)
                .with_context(|| format!("failed to remove file {}", staging.display()))?;
        }

        Ok(())
    }

    fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    fn find(&self, id: &str) -> Option<&CatalogEntry> {
        self.catalog.iter().find(|entry| entry.id == id)
    }

    /// The final on-disk path a catalog entry installs to: a directory named
    /// after the entry's `id` for models with more than one artifact, or a
    /// single file (the artifact's `name`) otherwise.
    fn install_path(&self, entry: &CatalogEntry) -> PathBuf {
        if entry.artifacts.len() > 1 {
            self.models_dir().join(entry.id)
        } else {
            let name = entry.artifacts.first().map_or(entry.id, |a| a.name);
            self.models_dir().join(name)
        }
    }

    /// Whether every artifact of `entry` is present at `path`, the value
    /// returned by [`Self::install_path`] for it.
    fn is_installed(&self, entry: &CatalogEntry, path: &Path) -> bool {
        entry
            .artifacts
            .iter()
            .all(|a| artifact_path(entry, path, a).is_file())
    }

    /// Verifies that every artifact of the installed model `id` is present
    /// and has exactly the byte length recorded in the catalog, returning
    /// its installed path (the same one [`Self::path_for`] would) only if
    /// every check passes.
    ///
    /// This exists because a corrupt model file does not fail cleanly once
    /// handed to the native speech engine: sherpa-onnx's token-table parser
    /// calls a compiled-in `exit()` on a malformed `tokens.txt`, and a
    /// malformed `.onnx` file makes onnxruntime throw a C++ exception that
    /// unwinds across the FFI boundary uncaught. Neither is catchable in
    /// Rust — both abort the whole process with nothing logged and no
    /// notice shown. Verifying before the path is ever handed to the native
    /// layer is the only point at which this can be prevented rather than
    /// merely reported.
    ///
    /// The check is file size, not a checksum: hashing every artifact (up
    /// to several hundred megabytes) on every engine load would add real
    /// latency to every app start and every language switch, to guard
    /// against corruption that the download path already checksums
    /// against. A size mismatch is the failure actually observed in
    /// practice — an interrupted download leaving a file of the right name
    /// and the wrong length — but this is not a complete integrity check:
    /// a file of the correct size with corrupted bytes inside it still
    /// passes.
    pub fn verify_installed(&self, id: &str) -> Result<PathBuf, IntegrityError> {
        let entry = self
            .find(id)
            .ok_or_else(|| IntegrityError::UnknownModel(id.to_string()))?;
        let path = self.install_path(entry);
        if !self.is_installed(entry, &path) {
            return Err(IntegrityError::NotInstalled(id.to_string()));
        }

        for artifact in entry.artifacts {
            let artifact_path = artifact_path(entry, &path, artifact);
            let metadata = fs::metadata(&artifact_path).map_err(|source| IntegrityError::Io {
                model: id.to_string(),
                artifact: artifact.name.to_string(),
                path: artifact_path.clone(),
                source,
            })?;
            let actual = metadata.len();
            if actual != artifact.size_bytes {
                return Err(IntegrityError::SizeMismatch {
                    model: id.to_string(),
                    artifact: artifact.name.to_string(),
                    path: artifact_path,
                    expected: artifact.size_bytes,
                    actual,
                });
            }
        }

        Ok(path)
    }
}

/// Resolves the on-disk path of one artifact of `entry`, given the entry's
/// `install_path`: the artifact itself for a single-artifact entry (where
/// `install_path` is already a file), or `install_path` joined with the
/// artifact's name for a multi-artifact entry (where `install_path` is a
/// directory).
fn artifact_path(entry: &CatalogEntry, install_path: &Path, artifact: &Artifact) -> PathBuf {
    if entry.artifacts.len() > 1 {
        install_path.join(artifact.name)
    } else {
        install_path.to_path_buf()
    }
}

/// Downloads and verifies one artifact into `staging_dir`, leaving it at
/// `staging_dir/<artifact.name>` on success. Verified artifacts from a prior
/// interrupted multi-file download are reused without another request.
fn stage_one_artifact(
    id: &str,
    artifact: &Artifact,
    staging_dir: &Path,
    cancellation: &DownloadCancellation,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<()> {
    let part_path = staging_dir.join(format!("{}.part", artifact.name));
    let final_in_staging = staging_dir.join(artifact.name);

    if final_in_staging.is_dir() {
        fs::remove_dir_all(&final_in_staging).with_context(|| {
            format!(
                "failed to remove invalid staged directory {}",
                final_in_staging.display()
            )
        })?;
    } else if final_in_staging.is_file() {
        let (digest, size) = hash_file(&final_in_staging, cancellation)?;
        if size == artifact.size_bytes && digest == artifact.sha256 {
            let _ = fs::remove_file(&part_path);
            progress(size, artifact.size_bytes);
            return Ok(());
        }

        tracing::warn!(
            model = id,
            artifact = artifact.name,
            "discarding an invalid previously staged artifact"
        );
        fs::remove_file(&final_in_staging).with_context(|| {
            format!(
                "failed to remove invalid staged artifact {}",
                final_in_staging.display()
            )
        })?;
    }

    let digest = stream_to_part_with_cancellation(
        artifact.url,
        &part_path,
        artifact.size_bytes,
        cancellation,
        DOWNLOAD_TIMEOUTS,
        progress,
    )?;
    if digest != artifact.sha256 {
        let _ = fs::remove_file(&part_path);
        bail!(
            "checksum mismatch for model '{id}' artifact '{}': expected {}, got {digest}",
            artifact.name,
            artifact.sha256
        );
    }

    fs::rename(&part_path, &final_in_staging).with_context(|| {
        format!(
            "failed to move {} into {}",
            part_path.display(),
            final_in_staging.display()
        )
    })
}

/// Streams the HTTP body at `url` into `part_path`, reporting progress against
/// the catalog's expected size and returning the full file's sha256. Existing
/// valid-length bytes are hashed and continued with a Range request.
#[cfg(test)]
fn stream_to_part(
    url: &str,
    part_path: &Path,
    expected_size: u64,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<String> {
    stream_to_part_with_cancellation(
        url,
        part_path,
        expected_size,
        &DownloadCancellation::default(),
        DOWNLOAD_TIMEOUTS,
        progress,
    )
}

fn stream_to_part_with_cancellation(
    url: &str,
    part_path: &Path,
    expected_size: u64,
    cancellation: &DownloadCancellation,
    timeouts: DownloadTimeouts,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<String> {
    cancellation.check()?;

    if part_path.is_dir() {
        fs::remove_dir_all(part_path)
            .with_context(|| format!("failed to remove invalid partial {}", part_path.display()))?;
    }

    let mut resume_from = match fs::metadata(part_path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect partial {}", part_path.display()));
        }
    };
    if resume_from > expected_size {
        tracing::warn!(
            path = %part_path.display(),
            actual = resume_from,
            expected = expected_size,
            "discarding oversized partial model artifact"
        );
        fs::remove_file(part_path)
            .with_context(|| format!("failed to remove invalid partial {}", part_path.display()))?;
        resume_from = 0;
    }

    let mut hasher = Sha256::new();
    if resume_from > 0 {
        hash_file_into(part_path, &mut hasher, cancellation)?;
        if resume_from == expected_size {
            progress(resume_from, expected_size);
            return Ok(hex::encode(hasher.finalize()));
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create model download runtime")?;
    runtime.block_on(stream_response_to_part(
        url,
        part_path,
        expected_size,
        resume_from,
        hasher,
        cancellation,
        timeouts,
        progress,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn stream_response_to_part(
    url: &str,
    part_path: &Path,
    expected_size: u64,
    resume_from: u64,
    mut hasher: Sha256,
    cancellation: &DownloadCancellation,
    timeouts: DownloadTimeouts,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<String> {
    let client = reqwest::Client::builder()
        .connect_timeout(timeouts.connect)
        .build()
        .context("failed to create model download client")?;
    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header(RANGE, format!("bytes={resume_from}-"));
    }

    let mut send = Box::pin(request.send());
    let response_deadline = tokio::time::sleep(timeouts.connect);
    tokio::pin!(response_deadline);
    let mut cancellation_tick = tokio::time::interval(CANCELLATION_POLL_INTERVAL);
    cancellation_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    cancellation_tick.tick().await;

    let mut response = loop {
        tokio::select! {
            result = &mut send => {
                break result.with_context(|| format!("failed to request {url}"))?;
            }
            _ = &mut response_deadline => {
                bail!("model download server did not respond within {:?}", timeouts.connect);
            }
            _ = cancellation_tick.tick() => cancellation.check()?,
        }
    };
    cancellation.check()?;

    let status = response.status();
    let mut append = resume_from > 0;
    if resume_from == 0 {
        if status != StatusCode::OK {
            bail!("download of {url} failed with status {status}");
        }
    } else if status == StatusCode::PARTIAL_CONTENT {
        let content_range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok());
        let content_length = response.content_length();
        if let Err(error) =
            validate_partial_response(content_range, content_length, resume_from, expected_size)
        {
            let _ = fs::remove_file(part_path);
            return Err(error);
        }
    } else if status == StatusCode::OK {
        tracing::info!(
            url,
            "server ignored Range; restarting artifact from byte zero"
        );
        append = false;
        hasher = Sha256::new();
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let content_range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok());
        match validate_unsatisfied_response(content_range, resume_from, expected_size) {
            Ok(()) => {
                progress(resume_from, expected_size);
                return Ok(hex::encode(hasher.finalize()));
            }
            Err(error) => {
                let _ = fs::remove_file(part_path);
                return Err(error);
            }
        }
    } else {
        bail!("download of {url} failed with status {status}; retry to resume");
    }

    let mut file = if append {
        OpenOptions::new().create(true).append(true).open(part_path)
    } else {
        File::create(part_path)
    }
    .with_context(|| format!("failed to open {}", part_path.display()))?;
    let mut done = if append { resume_from } else { 0 };
    progress(done, expected_size);

    loop {
        cancellation.check()?;
        let mut next_chunk = Box::pin(response.chunk());
        let stall_deadline = tokio::time::sleep(timeouts.stall);
        tokio::pin!(stall_deadline);
        let chunk = loop {
            tokio::select! {
                result = &mut next_chunk => {
                    break result.context("failed reading response body")?;
                }
                _ = &mut stall_deadline => {
                    return Err(DownloadStalled { seconds: timeouts.stall.as_secs() }.into());
                }
                _ = cancellation_tick.tick() => cancellation.check()?,
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        cancellation.check()?;

        let new_done = done
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow!("downloaded byte count overflowed"))?;
        if new_done > expected_size {
            drop(file);
            let _ = fs::remove_file(part_path);
            bail!(
                "downloaded artifact is larger than expected: expected {expected_size} bytes, received more than {new_done}"
            );
        }

        hasher.update(&chunk);
        file.write_all(&chunk)
            .context("failed writing to part file")?;
        done = new_done;
        progress(done, expected_size);
    }

    cancellation.check()?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", part_path.display()))?;
    if done != expected_size {
        bail!("download ended after {done} of {expected_size} bytes; retry to resume");
    }

    Ok(hex::encode(hasher.finalize()))
}

fn hash_file(path: &Path, cancellation: &DownloadCancellation) -> Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let size = hash_file_into(path, &mut hasher, cancellation)?;
    Ok((hex::encode(hasher.finalize()), size))
}

fn hash_file_into(
    path: &Path,
    hasher: &mut Sha256,
    cancellation: &DownloadCancellation,
) -> Result<u64> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open partial {}", path.display()))?;
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        cancellation.check()?;
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read partial {}", path.display()))?;
        if read == 0 {
            return Ok(size);
        }
        cancellation.check()?;
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
}

fn validate_partial_response(
    content_range: Option<&str>,
    content_length: Option<u64>,
    resume_from: u64,
    expected_size: u64,
) -> Result<()> {
    let Some((start, end, total)) = content_range.and_then(parse_partial_content_range) else {
        bail!("server returned an invalid Content-Range; partial download was discarded");
    };
    if start != resume_from || total != expected_size {
        bail!(
            "server returned Content-Range bytes {start}-{end}/{total}, expected start {resume_from} and total {expected_size}; partial download was discarded"
        );
    }
    if let Some(length) = content_length {
        let ranged_length = end - start + 1;
        if length != ranged_length {
            bail!(
                "server returned Content-Length {length} for a {ranged_length}-byte range; partial download was discarded"
            );
        }
    }
    Ok(())
}

fn validate_unsatisfied_response(
    content_range: Option<&str>,
    resume_from: u64,
    expected_size: u64,
) -> Result<()> {
    let Some(total) = content_range.and_then(parse_unsatisfied_content_range) else {
        bail!("server returned 416 without a valid Content-Range; partial download was discarded");
    };
    if total == expected_size && resume_from == expected_size {
        Ok(())
    } else {
        bail!(
            "server rejected resume at byte {resume_from} for a {total}-byte artifact; partial download was discarded"
        )
    }
}

fn parse_partial_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = total.parse().ok()?;
    (start <= end && end < total).then_some((start, end, total))
}

fn parse_unsatisfied_content_range(value: &str) -> Option<u64> {
    value.strip_prefix("bytes */")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Leaks a `Vec<Artifact>` into a `&'static [Artifact]`, mirroring how
    /// the real catalog's entries are `'static` data, so test entries can be
    /// built from owned `String`s (e.g. a mock server's URI).
    fn leak_artifacts(artifacts: Vec<Artifact>) -> &'static [Artifact] {
        Box::leak(artifacts.into_boxed_slice())
    }

    fn whisper_entry(url: String, sha256: String, size_bytes: u64) -> CatalogEntry {
        CatalogEntry {
            id: "test-whisper",
            engine: "whisper",
            label: "Test Whisper",
            size_mb: 1,
            supported_languages: &["*"],
            role: ModelRole::Final,
            performance_class: PerformanceClass::Fast,
            recommendation_tags: &["Test model"],
            artifacts: leak_artifacts(vec![Artifact {
                url: Box::leak(url.into_boxed_str()),
                sha256: Box::leak(sha256.into_boxed_str()),
                name: "ggml-test.bin",
                size_bytes,
            }]),
        }
    }

    /// A two-artifact entry (e.g. mirroring a sherpa-onnx model's encoder
    /// and tokens) whose artifacts are never actually downloaded in the
    /// tests that use it — only `path_for`'s and `verify_installed`'s
    /// "every file present" / "every size matches" logic is exercised, so
    /// the URLs are unused placeholders. `encoder.onnx` is expected to be
    /// 100 bytes and `tokens.txt` 50 once genuinely installed.
    fn two_file_entry() -> CatalogEntry {
        CatalogEntry {
            id: "two-file-model",
            engine: "sherpa",
            label: "Test Two-File Model",
            size_mb: 1,
            supported_languages: &["en"],
            role: ModelRole::Final,
            performance_class: PerformanceClass::Fast,
            recommendation_tags: &["Test model"],
            artifacts: &[
                Artifact {
                    url: "unused",
                    sha256: "unused",
                    name: "encoder.onnx",
                    size_bytes: 100,
                },
                Artifact {
                    url: "unused",
                    sha256: "unused",
                    name: "tokens.txt",
                    size_bytes: 50,
                },
            ],
        }
    }

    fn multi_artifact_entry(
        encoder_url: String,
        encoder_sha256: String,
        encoder_size_bytes: u64,
        tokens_url: String,
        tokens_sha256: String,
        tokens_size_bytes: u64,
    ) -> CatalogEntry {
        CatalogEntry {
            id: "test-multi",
            engine: "sherpa",
            label: "Test Multi-Artifact",
            size_mb: 1,
            supported_languages: &["en"],
            role: ModelRole::Final,
            performance_class: PerformanceClass::Fast,
            recommendation_tags: &["Test model"],
            artifacts: leak_artifacts(vec![
                Artifact {
                    url: Box::leak(encoder_url.into_boxed_str()),
                    sha256: Box::leak(encoder_sha256.into_boxed_str()),
                    name: "encoder.onnx",
                    size_bytes: encoder_size_bytes,
                },
                Artifact {
                    url: Box::leak(tokens_url.into_boxed_str()),
                    sha256: Box::leak(tokens_sha256.into_boxed_str()),
                    name: "tokens.txt",
                    size_bytes: tokens_size_bytes,
                },
            ]),
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    fn write_partial(data_dir: &Path, model: &str, artifact: &str, bytes: &[u8]) -> PathBuf {
        let staging = data_dir.join("models").join(format!("{model}.staging"));
        fs::create_dir_all(&staging).expect("create staging");
        let path = staging.join(format!("{artifact}.part"));
        fs::write(&path, bytes).expect("write partial");
        path
    }

    fn assert_cancelled(result: &Result<PathBuf>) {
        let error = result.as_ref().expect_err("download should be cancelled");
        assert!(
            error.downcast_ref::<DownloadCancelled>().is_some(),
            "cancellation must remain distinguishable from failure, got: {error:#}"
        );
    }

    #[test]
    fn cancellation_before_the_first_byte_performs_no_network_or_disk_work() {
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry(
            "http://127.0.0.1:9/must-not-be-requested".to_string(),
            "unused".to_string(),
            1,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);
        let cancellation = DownloadCancellation::default();
        cancellation.cancel();

        let result =
            manager.download_with_cancellation("test-whisper", &cancellation, &mut |_, _| {});

        assert_cancelled(&result);
        assert!(!dir.path().join("models").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_mid_artifact_preserves_partial_and_retry_installs() {
        let server = MockServer::start().await;
        let body = vec![0x5au8; 200_000];
        let sha256 = sha256_hex(&body);
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256,
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);
        let cancellation = DownloadCancellation::default();
        let callback_cancellation = cancellation.clone();

        let (first, manager) = tokio::task::spawn_blocking(move || {
            let result = manager.download_with_cancellation(
                "test-whisper",
                &cancellation,
                &mut |done, _| {
                    if done > 0 {
                        callback_cancellation.cancel();
                    }
                },
            );
            (result, manager)
        })
        .await
        .expect("blocking task panicked");

        assert_cancelled(&first);
        assert!(!dir.path().join("models/ggml-test.bin").exists());
        let part = dir
            .path()
            .join("models/test-whisper.staging/ggml-test.bin.part");
        let partial_size = fs::metadata(&part).expect("partial must remain").len();
        assert!(partial_size > 0 && partial_size < body.len() as u64);

        let installed =
            tokio::task::spawn_blocking(move || manager.download("test-whisper", &mut |_, _| {}))
                .await
                .expect("blocking task panicked")
                .expect("a new token should allow a clean retry");
        assert_eq!(fs::read(installed).expect("installed bytes"), body);
        let requests = server.received_requests().await.expect("request log");
        assert_eq!(requests.len(), 2);
        let expected_range = format!("bytes={partial_size}-");
        assert_eq!(
            requests[1]
                .headers
                .get("range")
                .and_then(|value| value.to_str().ok()),
            Some(expected_range.as_str())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_between_multi_file_artifacts_never_publishes_the_model() {
        let server = MockServer::start().await;
        let encoder = vec![0x11u8; 1_000];
        let tokens = vec![0x22u8; 500];
        Mock::given(method("GET"))
            .and(path("/encoder"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encoder.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tokens"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tokens.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = multi_artifact_entry(
            format!("{}/encoder", server.uri()),
            sha256_hex(&encoder),
            encoder.len() as u64,
            format!("{}/tokens", server.uri()),
            sha256_hex(&tokens),
            tokens.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);
        let cancellation = DownloadCancellation::default();
        let callback_cancellation = cancellation.clone();
        let first_size = encoder.len() as u64;

        let result = tokio::task::spawn_blocking(move || {
            let mut boundary_seen = false;
            manager.download_with_cancellation("test-multi", &cancellation, &mut |done, _| {
                if done == first_size {
                    if boundary_seen {
                        callback_cancellation.cancel();
                    }
                    boundary_seen = true;
                }
            })
        })
        .await
        .expect("blocking task panicked");

        assert_cancelled(&result);
        assert!(!dir.path().join("models/test-multi").exists());
        assert_eq!(
            fs::read(dir.path().join("models/test-multi.staging/encoder.onnx"))
                .expect("verified first artifact must remain"),
            encoder
        );
        assert!(!dir
            .path()
            .join("models/test-multi.staging/tokens.txt.part")
            .exists());
        let requests = server.received_requests().await.expect("request log");
        assert_eq!(
            requests.len(),
            1,
            "the second artifact must not be requested"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn download_streams_reports_progress_and_verifies_checksum() {
        let server = MockServer::start().await;
        let body = vec![0x42u8; 200_000];
        let sha256 = sha256_hex(&body);

        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256.clone(),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let (result, calls) = tokio::task::spawn_blocking(move || {
            let mut calls: Vec<(u64, u64)> = Vec::new();
            let result =
                manager.download("test-whisper", &mut |done, total| calls.push((done, total)));
            (result, calls)
        })
        .await
        .expect("blocking task panicked");

        let installed_path = result.expect("download should succeed");
        assert_eq!(installed_path, dir.path().join("models/ggml-test.bin"));
        let installed_bytes = fs::read(&installed_path).expect("read installed file");
        assert_eq!(installed_bytes, body);

        // Progress must actually be observable through the public `download`
        // API, not just through the private `stream_to_part` helper.
        assert!(!calls.is_empty(), "expected at least one progress call");
        assert_eq!(calls.first(), Some(&(0, body.len() as u64)));
        assert_eq!(calls.last(), Some(&(body.len() as u64, body.len() as u64)));
        for pair in calls.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0,
                "progress must never decrease: {:?} -> {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn progress_is_monotonically_non_decreasing_and_ends_at_total() {
        let dir = tempfile::tempdir().expect("tempdir");
        let part_path = dir.path().join("progress-test.part");

        // A raw local HTTP/1.1 server: no need for wiremock here since this
        // test only cares about the shape of the progress callback, not the
        // catalog/manager wiring.
        let (base_url, handle) = spawn_fixed_body_server(vec![7u8; 500_000], false);

        let mut calls: Vec<(u64, u64)> = Vec::new();
        let digest = stream_to_part(
            &format!("{base_url}/body"),
            &part_path,
            500_000,
            &mut |done, total| calls.push((done, total)),
        )
        .expect("stream_to_part should succeed");
        handle.join().expect("server thread should not panic");

        assert_eq!(digest, sha256_hex(&vec![7u8; 500_000]));
        assert!(calls.len() >= 2, "expected multiple progress callbacks");
        assert_eq!(calls.first(), Some(&(0, 500_000)));
        assert_eq!(calls.last(), Some(&(500_000, 500_000)));
        for pair in calls.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0,
                "progress must never decrease: {:?} -> {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn range_response_continues_a_partial_and_progress_starts_at_saved_bytes() {
        let server = MockServer::start().await;
        let body = vec![0x31u8; 90_000];
        let split = 27_000usize;
        let content_range = format!(
            "bytes {split}-{}/{total}",
            body.len() - 1,
            total = body.len()
        );
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .and(header("range", format!("bytes={split}-")))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("Content-Range", content_range)
                    .set_body_bytes(body[split..].to_vec()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        write_partial(dir.path(), "test-whisper", "ggml-test.bin", &body[..split]);
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256_hex(&body),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let (installed, calls) = tokio::task::spawn_blocking(move || {
            let mut calls = Vec::new();
            let installed = manager
                .download("test-whisper", &mut |done, total| calls.push((done, total)))
                .expect("resume should install");
            (installed, calls)
        })
        .await
        .expect("blocking task panicked");

        assert_eq!(fs::read(installed).expect("installed bytes"), body);
        assert_eq!(calls.first(), Some(&(split as u64, body.len() as u64)));
        assert_eq!(calls.last(), Some(&(body.len() as u64, body.len() as u64)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn server_ignoring_range_restarts_from_zero_without_duplicating_bytes() {
        let server = MockServer::start().await;
        let body = vec![0x41u8; 24_000];
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .expect(1)
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        write_partial(dir.path(), "test-whisper", "ggml-test.bin", &body[..4_000]);
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256_hex(&body),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let (installed, first_progress) = tokio::task::spawn_blocking(move || {
            let mut first_progress = None;
            let installed = manager
                .download("test-whisper", &mut |done, total| {
                    first_progress.get_or_insert((done, total));
                })
                .expect("full restart should install");
            (installed, first_progress)
        })
        .await
        .expect("blocking task panicked");

        assert_eq!(first_progress, Some((0, body.len() as u64)));
        assert_eq!(fs::read(installed).expect("installed bytes"), body);
        let requests = server.received_requests().await.expect("request log");
        assert!(requests[0].headers.get("range").is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_content_range_discards_the_partial() {
        let server = MockServer::start().await;
        let body = vec![0x51u8; 10_000];
        let split = 2_000usize;
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header(
                        "Content-Range",
                        format!(
                            "bytes {}-{}/{total}",
                            split + 1,
                            body.len() - 1,
                            total = body.len()
                        ),
                    )
                    .set_body_bytes(body[split..].to_vec()),
            )
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let part = write_partial(dir.path(), "test-whisper", "ggml-test.bin", &body[..split]);
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256_hex(&body),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let error = tokio::task::spawn_blocking(move || {
            manager
                .download("test-whisper", &mut |_, _| {})
                .expect_err("wrong range must fail")
        })
        .await
        .expect("blocking task panicked");

        assert!(error.to_string().contains("expected start"));
        assert!(!part.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn range_not_satisfiable_for_incomplete_file_discards_the_partial() {
        let server = MockServer::start().await;
        let body = vec![0x61u8; 10_000];
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(
                ResponseTemplate::new(416)
                    .insert_header("Content-Range", format!("bytes */{}", body.len())),
            )
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let part = write_partial(dir.path(), "test-whisper", "ggml-test.bin", &body[..2_000]);
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256_hex(&body),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let result =
            tokio::task::spawn_blocking(move || manager.download("test-whisper", &mut |_, _| {}))
                .await
                .expect("blocking task panicked");

        assert!(result.is_err());
        assert!(!part.exists());
    }

    #[test]
    fn range_not_satisfiable_accepts_only_a_complete_matching_file() {
        assert!(validate_unsatisfied_response(Some("bytes */100"), 100, 100).is_ok());
        assert!(validate_unsatisfied_response(Some("bytes */100"), 99, 100).is_err());
        assert!(validate_unsatisfied_response(Some("bytes */101"), 100, 100).is_err());
        assert!(validate_unsatisfied_response(None, 100, 100).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_partial_is_discarded_before_a_fresh_request() {
        let server = MockServer::start().await;
        let body = vec![0x71u8; 8_000];
        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        write_partial(
            dir.path(),
            "test-whisper",
            "ggml-test.bin",
            &vec![0u8; body.len() + 1],
        );
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256_hex(&body),
            body.len() as u64,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let installed = tokio::task::spawn_blocking(move || {
            manager
                .download("test-whisper", &mut |_, _| {})
                .expect("fresh request should install")
        })
        .await
        .expect("blocking task panicked");

        assert_eq!(fs::read(installed).expect("installed bytes"), body);
        let requests = server.received_requests().await.expect("request log");
        assert!(requests[0].headers.get("range").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_checksum_errors_and_discards_the_partial() {
        let server = MockServer::start().await;
        let body = vec![0x11u8; 1_000];
        let body_len = body.len() as u64;

        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            body_len,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let result =
            tokio::task::spawn_blocking(move || manager.download("test-whisper", &mut |_, _| {}))
                .await
                .expect("blocking task panicked");

        assert!(result.is_err(), "checksum mismatch should be an error");
        assert!(!dir.path().join("models/ggml-test.bin").exists());
        assert!(!dir
            .path()
            .join("models/test-whisper.staging/ggml-test.bin.part")
            .exists());
    }

    #[test]
    fn interrupted_body_errors_and_preserves_partial_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base_url, handle) = spawn_fixed_body_server(vec![9u8; 500_000], true);
        let entry = whisper_entry(
            format!("{base_url}/body"),
            "irrelevant-because-body-is-truncated".to_string(),
            500_000,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let result = manager.download("test-whisper", &mut |_, _| {});
        let _ = handle.join();

        assert!(result.is_err(), "truncated body should be an error");
        let part = dir
            .path()
            .join("models/test-whisper.staging/ggml-test.bin.part");
        let size = fs::metadata(part).expect("partial must remain").len();
        assert!(size > 0 && size < 500_000);
    }

    #[test]
    fn stalled_body_times_out_and_preserves_received_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let part_path = dir.path().join("stalled.part");
        let prefix = vec![0x81u8; 2_000];
        let expected_size = 10_000;
        let (base_url, handle) =
            spawn_stalling_body_server(prefix.clone(), expected_size, Duration::from_millis(200));

        let result = stream_to_part_with_cancellation(
            &format!("{base_url}/body"),
            &part_path,
            expected_size,
            &DownloadCancellation::default(),
            DownloadTimeouts {
                connect: Duration::from_secs(1),
                stall: Duration::from_millis(50),
            },
            &mut |_, _| {},
        );
        handle.join().expect("server thread should not panic");

        let error = result.expect_err("stalled body must time out");
        assert!(error.downcast_ref::<DownloadStalled>().is_some());
        assert_eq!(fs::read(part_path).expect("partial bytes"), prefix);
    }

    #[test]
    fn response_header_wait_is_bounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let part_path = dir.path().join("no-headers.part");
        let (base_url, handle) = spawn_headerless_server(Duration::from_millis(200));

        let result = stream_to_part_with_cancellation(
            &format!("{base_url}/body"),
            &part_path,
            10_000,
            &DownloadCancellation::default(),
            DownloadTimeouts {
                connect: Duration::from_millis(50),
                stall: Duration::from_secs(1),
            },
            &mut |_, _| {},
        );
        handle.join().expect("server thread should not panic");

        let error = result.expect_err("missing response headers must time out");
        assert!(error.to_string().contains("did not respond"));
        assert!(!part_path.exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catalog_marks_model_installed_after_download() {
        let server = MockServer::start().await;
        let body = vec![0x77u8; 10_000];
        let body_len = body.len() as u64;
        let sha256 = sha256_hex(&body);

        Mock::given(method("GET"))
            .and(path("/ggml-test.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry(
            format!("{}/ggml-test.bin", server.uri()),
            sha256.clone(),
            body_len,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let before = manager.catalog();
        assert_eq!(before.len(), 1);
        assert!(!before[0].installed, "should not be installed yet");

        let manager = tokio::task::spawn_blocking(move || {
            manager
                .download("test-whisper", &mut |_, _| {})
                .expect("download should succeed");
            manager
        })
        .await
        .expect("blocking task panicked");

        let after = manager.catalog();
        assert_eq!(after.len(), 1);
        assert!(after[0].installed, "should be installed after download");

        manager.remove("test-whisper").expect("remove");
        let after_remove = manager.catalog();
        assert!(
            !after_remove[0].installed,
            "should not be installed after removal"
        );
    }

    #[test]
    fn multi_artifact_model_is_installed_only_when_every_file_is_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), vec![two_file_entry()]);

        let model_dir = dir.path().join("models").join("two-file-model");
        fs::create_dir_all(&model_dir).expect("create model dir");
        fs::write(model_dir.join("encoder.onnx"), b"x").expect("write encoder");

        assert!(
            models.path_for("two-file-model").is_none(),
            "a half-downloaded model must not report as installed"
        );

        fs::write(model_dir.join("tokens.txt"), b"x").expect("write tokens");
        assert_eq!(models.path_for("two-file-model"), Some(model_dir));
    }

    #[test]
    fn a_truncated_artifact_is_reported_as_damaged_not_installed() {
        // The failure this guards against is not hypothetical: a 456 MB model
        // download was observed to stall silently partway through, leaving a
        // file of exactly the right name and the wrong length.
        let dir = tempfile::tempdir().expect("tempdir");
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), vec![two_file_entry()]);

        let model_dir = dir.path().join("models").join("two-file-model");
        fs::create_dir_all(&model_dir).expect("create model dir");
        fs::write(model_dir.join("encoder.onnx"), b"truncated").expect("write encoder");
        fs::write(model_dir.join("tokens.txt"), b"x").expect("write tokens");

        let err = models
            .verify_installed("two-file-model")
            .expect_err("a wrong-sized artifact must not pass verification");
        assert!(matches!(err, IntegrityError::SizeMismatch { .. }));
    }

    #[test]
    fn verify_installed_succeeds_when_every_artifact_size_matches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), vec![two_file_entry()]);

        let model_dir = dir.path().join("models").join("two-file-model");
        fs::create_dir_all(&model_dir).expect("create model dir");
        fs::write(model_dir.join("encoder.onnx"), vec![0u8; 100]).expect("write encoder");
        fs::write(model_dir.join("tokens.txt"), vec![0u8; 50]).expect("write tokens");

        let verified = models
            .verify_installed("two-file-model")
            .expect("correctly sized artifacts must verify");
        assert_eq!(verified, model_dir);
    }

    #[test]
    fn verify_installed_checks_the_size_of_a_single_file_model() {
        // Every other `verify_installed` test uses `two_file_entry`, so this
        // is the only one exercising `artifact_path`'s single-artifact
        // branch (whisper's shape, and the reason the function is generic).
        let dir = tempfile::tempdir().expect("tempdir");
        let entry = whisper_entry("unused".to_string(), "unused".to_string(), 100);
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).expect("create models dir");
        let file = models_dir.join("ggml-test.bin");

        fs::write(&file, vec![0u8; 99]).expect("write short file");
        assert!(matches!(
            models.verify_installed("test-whisper"),
            Err(IntegrityError::SizeMismatch { .. })
        ));

        fs::write(&file, vec![0u8; 100]).expect("write correct file");
        assert_eq!(
            models.verify_installed("test-whisper").expect("verifies"),
            file
        );
    }

    #[test]
    fn verify_installed_reports_a_never_downloaded_model_as_not_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), vec![two_file_entry()]);

        let err = models
            .verify_installed("two-file-model")
            .expect_err("a never-downloaded model must not verify");
        assert!(matches!(err, IntegrityError::NotInstalled(id) if id == "two-file-model"));
    }

    #[test]
    fn verify_installed_rejects_an_unknown_model_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = ModelManager::with_catalog(dir.path().to_path_buf(), Vec::new());

        let err = models
            .verify_installed("does-not-exist")
            .expect_err("an unknown id must not verify");
        assert!(matches!(err, IntegrityError::UnknownModel(id) if id == "does-not-exist"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn download_installs_every_artifact_of_a_multi_artifact_model() {
        let server = MockServer::start().await;
        let encoder_body = vec![0xAAu8; 5_000];
        let tokens_body = b"token list".to_vec();
        let encoder_size_bytes = encoder_body.len() as u64;
        let tokens_size_bytes = tokens_body.len() as u64;
        let encoder_sha256 = sha256_hex(&encoder_body);
        let tokens_sha256 = sha256_hex(&tokens_body);

        Mock::given(method("GET"))
            .and(path("/encoder.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encoder_body.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tokens.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tokens_body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = multi_artifact_entry(
            format!("{}/encoder.onnx", server.uri()),
            encoder_sha256,
            encoder_size_bytes,
            format!("{}/tokens.txt", server.uri()),
            tokens_sha256,
            tokens_size_bytes,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let installed_path =
            tokio::task::spawn_blocking(move || manager.download("test-multi", &mut |_, _| {}))
                .await
                .expect("blocking task panicked")
                .expect("download should succeed");

        assert_eq!(installed_path, dir.path().join("models/test-multi"));
        assert_eq!(
            fs::read(installed_path.join("encoder.onnx")).expect("read encoder"),
            encoder_body
        );
        assert_eq!(
            fs::read(installed_path.join("tokens.txt")).expect("read tokens"),
            tokens_body
        );
        assert!(!dir.path().join("models/test-multi.staging").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_artifact_download_reports_progress_without_resetting_between_artifacts() {
        let server = MockServer::start().await;
        let encoder_body = vec![0xCCu8; 5_000];
        let tokens_body = vec![0xDDu8; 2_000];
        let encoder_size_bytes = encoder_body.len() as u64;
        let tokens_size_bytes = tokens_body.len() as u64;
        let encoder_sha256 = sha256_hex(&encoder_body);
        let tokens_sha256 = sha256_hex(&tokens_body);

        Mock::given(method("GET"))
            .and(path("/encoder.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encoder_body.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tokens.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tokens_body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = multi_artifact_entry(
            format!("{}/encoder.onnx", server.uri()),
            encoder_sha256,
            encoder_size_bytes,
            format!("{}/tokens.txt", server.uri()),
            tokens_sha256,
            tokens_size_bytes,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);
        let grand_total = encoder_size_bytes + tokens_size_bytes;

        let calls = tokio::task::spawn_blocking(move || {
            let mut calls: Vec<(u64, u64)> = Vec::new();
            manager
                .download("test-multi", &mut |done, total| calls.push((done, total)))
                .expect("download should succeed");
            calls
        })
        .await
        .expect("blocking task panicked");

        assert!(!calls.is_empty(), "expected at least one progress call");
        assert_eq!(calls.first(), Some(&(0, grand_total)));
        assert_eq!(calls.last(), Some(&(grand_total, grand_total)));
        for pair in calls.windows(2) {
            assert!(
                pair[1].0 >= pair[0].0,
                "progress must never decrease across artifacts: {:?} -> {:?}",
                pair[0],
                pair[1]
            );
            assert_eq!(
                pair[0].1, grand_total,
                "total must stay the grand total across every artifact"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_artifact_checksum_failure_keeps_only_previously_verified_artifacts() {
        let server = MockServer::start().await;
        let encoder_body = vec![0xBBu8; 5_000];
        let tokens_body = b"token list".to_vec();
        let encoder_size_bytes = encoder_body.len() as u64;
        let tokens_size_bytes = tokens_body.len() as u64;
        let encoder_sha256 = sha256_hex(&encoder_body);

        Mock::given(method("GET"))
            .and(path("/encoder.onnx"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(encoder_body.clone()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tokens.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tokens_body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let entry = multi_artifact_entry(
            format!("{}/encoder.onnx", server.uri()),
            encoder_sha256,
            encoder_size_bytes,
            format!("{}/tokens.txt", server.uri()),
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            tokens_size_bytes,
        );
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), vec![entry]);

        let result =
            tokio::task::spawn_blocking(move || manager.download("test-multi", &mut |_, _| {}))
                .await
                .expect("blocking task panicked");

        assert!(
            result.is_err(),
            "a bad artifact checksum should fail the whole download"
        );
        assert!(!dir.path().join("models/test-multi").exists());
        let staging = dir.path().join("models/test-multi.staging");
        assert_eq!(
            fs::read(staging.join("encoder.onnx")).expect("verified encoder remains"),
            encoder_body
        );
        assert!(!staging.join("tokens.txt.part").exists());
    }

    // `ModelInfo` (the type `ModelManager::catalog()` returns) does not carry
    // artifacts, only installed state, so this checks the real hard-coded
    // `CatalogEntry` data directly rather than going through the manager.
    #[test]
    fn catalog_entries_declare_every_artifact_they_need() {
        for entry in CATALOG {
            assert!(
                !entry.artifacts.is_empty(),
                "{} declares no artifacts",
                entry.id
            );
            for artifact in entry.artifacts {
                assert_eq!(
                    artifact.sha256.len(),
                    64,
                    "{}: {} has a malformed sha256",
                    entry.id,
                    artifact.name
                );
            }
        }
    }

    #[test]
    fn catalog_entries_declare_complete_user_facing_capabilities() {
        for entry in CATALOG {
            assert!(
                !entry.supported_languages.is_empty(),
                "{} has no language metadata",
                entry.id
            );
            assert!(
                entry
                    .supported_languages
                    .iter()
                    .all(|language| !language.trim().is_empty()),
                "{} has an empty language tag",
                entry.id
            );
            assert!(
                !entry.recommendation_tags.is_empty(),
                "{} has no recommendation tags",
                entry.id
            );

            let expected_role = if entry.engine == "sherpa-streaming" {
                ModelRole::Preview
            } else {
                ModelRole::Final
            };
            assert_eq!(
                entry.role, expected_role,
                "{} has a role that disagrees with its runtime engine",
                entry.id
            );
        }
    }

    #[test]
    fn breeze_catalog_entry_pins_the_benchmarked_artifact() {
        let entry = CATALOG
            .iter()
            .find(|entry| entry.id == "breeze-asr-25-q5_k")
            .expect("Breeze must be in the catalog");

        assert_eq!(entry.engine, "whisper");
        assert_eq!(entry.artifacts.len(), 1);
        let artifact = &entry.artifacts[0];
        assert_eq!(artifact.name, "breeze-asr-q5_k.bin");
        assert_eq!(artifact.size_bytes, 1_080_732_108);
        assert_eq!(
            artifact.sha256,
            "8efbf0ce8a3f50fe332b7617da787fb81354b358c288b008d3bdef8359df64c6"
        );
    }

    #[test]
    fn large_v2_catalog_entry_pins_the_official_q5_artifact() {
        let entry = CATALOG
            .iter()
            .find(|entry| entry.id == "large-v2-q5_0")
            .expect("Large v2 must be in the catalog");

        assert_eq!(entry.engine, "whisper");
        assert_eq!(entry.artifacts.len(), 1);
        let artifact = &entry.artifacts[0];
        assert!(artifact
            .url
            .contains("/bf8b606c2fcd9173605cdf6bd2ac8a75a8141b6c/"));
        assert_eq!(artifact.name, "ggml-large-v2-q5_0.bin");
        assert_eq!(artifact.size_bytes, 1_080_732_091);
        assert_eq!(
            artifact.sha256,
            "3a214837221e4530dbc1fe8d734f302af393eb30bd0ed046042ebf4baf70f6f2"
        );
    }

    #[test]
    fn vosk_is_gone_from_the_catalog() {
        let models = ModelManager::new(PathBuf::from("/nonexistent"));
        assert!(
            models.catalog().iter().all(|m| m.engine != "vosk"),
            "vosk models must not be offered once the engine is removed"
        );
    }

    /// The desktop UI's final-model picker uses the explicit role rather than
    /// inferring purpose from an engine name. Assert the real catalog keeps
    /// every streaming entry out of that set.
    #[test]
    fn streaming_preview_models_are_excluded_from_the_injected_transcript_engine_set() {
        let models = ModelManager::new(PathBuf::from("/nonexistent")).catalog();
        let streaming_ids = ["zipformer-ru-small", "zipformer-en-small"];

        assert!(
            models
                .iter()
                .filter(|m| m.role == ModelRole::Final)
                .all(|m| !streaming_ids.contains(&m.id.as_str())),
            "a streaming preview model must not appear in the final transcript set"
        );

        for id in streaming_ids {
            let role = models.iter().find(|m| m.id == id).map(|m| m.role);
            assert_eq!(
                role,
                Some(ModelRole::Preview),
                "{id} must be catalogued for preview only"
            );
        }
    }

    /// `engine_of` is what the load path uses to reject a model of the wrong
    /// kind before handing it to a native decoder that would abort the
    /// process on it. Asserted against the real catalog, and specifically on
    /// the pair that collides: `parakeet-tdt-110m-en` (offline) and
    /// `zipformer-ru-small` (streaming) install under the very same four
    /// artifact names, so their catalog `engine` string is the only thing
    /// that tells them apart before load.
    #[test]
    fn engine_of_reports_the_kind_a_model_is_catalogued_under() {
        let models = ModelManager::new(PathBuf::from("/nonexistent"));

        assert_eq!(models.engine_of("parakeet-tdt-110m-en"), Some("sherpa"));
        assert_eq!(
            models.engine_of("zipformer-ru-small"),
            Some("sherpa-streaming")
        );
        assert_eq!(models.engine_of("small"), Some("whisper"));
        assert_eq!(
            models.engine_of("no-such-model"),
            None,
            "an id that is not catalogued at all has no kind, and must be \
             distinguishable from one that has the wrong kind"
        );
    }

    #[test]
    fn unknown_model_id_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), Vec::new());

        let result = manager.download("does-not-exist", &mut |_, _| {});
        assert!(result.is_err());
    }

    #[test]
    fn removing_unknown_or_uninstalled_model_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = ModelManager::with_catalog(dir.path().to_path_buf(), Vec::new());

        assert!(manager.remove("does-not-exist").is_ok());
    }

    /// A minimal HTTP/1.1 server on a background thread that serves `body`
    /// at `GET /body` with an accurate `Content-Length`. When `truncate` is
    /// true, it writes only half of `body` and then closes the connection,
    /// simulating a network interruption mid-download.
    fn spawn_fixed_body_server(
        body: Vec<u8>,
        truncate: bool,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request_buf = [0u8; 4096];
                let _ = stream.read(&mut request_buf);

                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());

                let to_send = if truncate { body.len() / 2 } else { body.len() };
                let _ = stream.write_all(&body[..to_send]);
                let _ = stream.flush();
                // Dropping `stream` here closes the socket; when truncated,
                // the client is left expecting more bytes than were sent.
            }
        });
        (format!("http://{addr}"), handle)
    }

    fn spawn_stalling_body_server(
        prefix: Vec<u8>,
        advertised_size: u64,
        pause: Duration,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request_buf = [0u8; 4096];
                let _ = stream.read(&mut request_buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {advertised_size}\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&prefix);
                let _ = stream.flush();
                std::thread::sleep(pause);
            }
        });
        (format!("http://{addr}"), handle)
    }

    fn spawn_headerless_server(pause: Duration) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request_buf = [0u8; 4096];
                let _ = stream.read(&mut request_buf);
                std::thread::sleep(pause);
            }
        });
        (format!("http://{addr}"), handle)
    }
}
